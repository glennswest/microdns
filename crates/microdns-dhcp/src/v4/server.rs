use crate::dns_register::DnsRegistrar;
use crate::lease::LeaseManager;
use crate::v4::packet::*;
use crate::v4::pool::{prefix_len_from_subnet, subnet_mask_from_prefix, Ipv4Pool};
use microdns_core::config::DhcpV4Config;
use microdns_core::db::Db;
use microdns_msg::events::Event;
use microdns_msg::MessageBus;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{watch, Mutex};
use tracing::{debug, error, info, warn};

/// PXE boot config for a pool
#[derive(Debug, Clone)]
struct PxeConfig {
    next_server: Ipv4Addr,
    boot_file: String,
    /// HTTP boot script URL served to iPXE clients instead of the TFTP boot file
    ipxe_boot_url: Option<String>,
}

pub struct Dhcpv4Server {
    _config: DhcpV4Config,
    pools: Arc<Mutex<Vec<Ipv4Pool>>>,
    /// PXE config per pool index
    pxe_configs: Vec<Option<PxeConfig>>,
    /// MAC → (IP, hostname) reservations
    reservations: HashMap<String, (Ipv4Addr, Option<String>)>,
    server_ip: Ipv4Addr,
    lease_manager: Arc<LeaseManager>,
    dns_registrar: Option<Arc<DnsRegistrar>>,
    message_bus: Option<Arc<dyn MessageBus>>,
    instance_id: String,
}

impl Dhcpv4Server {
    pub fn new(config: &DhcpV4Config, db: Db) -> anyhow::Result<Self> {
        let mut pools = Vec::new();
        let mut pxe_configs = Vec::new();

        for pool_cfg in &config.pools {
            let prefix_len = prefix_len_from_subnet(&pool_cfg.subnet).unwrap_or(24);
            let mask = subnet_mask_from_prefix(prefix_len);
            let dns_servers: Vec<Ipv4Addr> = pool_cfg
                .dns
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect();

            pools.push(Ipv4Pool::new(
                pool_cfg.range_start.parse()?,
                pool_cfg.range_end.parse()?,
                mask,
                pool_cfg.gateway.parse()?,
                dns_servers,
                pool_cfg.domain.clone(),
                pool_cfg.lease_time_secs as u32,
            ));

            let pxe = match (&pool_cfg.next_server, &pool_cfg.boot_file) {
                (Some(ns), Some(bf)) => Some(PxeConfig {
                    next_server: ns.parse()?,
                    boot_file: bf.clone(),
                    ipxe_boot_url: pool_cfg.ipxe_boot_url.clone(),
                }),
                _ => None,
            };
            pxe_configs.push(pxe);
        }

        // Parse reservations
        let mut reservations = HashMap::new();
        for res in &config.reservations {
            let mac = res.mac.to_lowercase();
            let ip: Ipv4Addr = res.ip.parse()?;
            reservations.insert(mac, (ip, res.hostname.clone()));
        }

        // Use configured server_ip if provided, otherwise fall back to first
        // pool's gateway. The server_ip is used for siaddr and option 54 (server
        // identifier) — it must be the DHCP server's own IP, NOT the gateway,
        // otherwise DHCP relays that use the gateway as their local-address will
        // confuse the OFFER with their own traffic and stop forwarding.
        let server_ip = config
            .server_ip
            .as_deref()
            .and_then(|s| s.parse::<Ipv4Addr>().ok())
            .or_else(|| pools.first().map(|p| p.gateway))
            .unwrap_or(Ipv4Addr::UNSPECIFIED);

        let lease_manager = Arc::new(LeaseManager::new(db));

        Ok(Self {
            _config: config.clone(),
            pools: Arc::new(Mutex::new(pools)),
            pxe_configs,
            reservations,
            server_ip,
            lease_manager,
            dns_registrar: None,
            message_bus: None,
            instance_id: String::new(),
        })
    }

    pub fn with_dns_registrar(mut self, registrar: Arc<DnsRegistrar>) -> Self {
        self.dns_registrar = Some(registrar);
        self
    }

    pub fn with_message_bus(mut self, bus: Arc<dyn MessageBus>, instance_id: &str) -> Self {
        self.message_bus = Some(bus);
        self.instance_id = instance_id.to_string();
        self
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let ports = &self._config.listen_ports;
        let primary_port = ports.first().copied().unwrap_or(67);

        info!("DHCPv4 server listening on 0.0.0.0:{primary_port}");

        // Restore existing leases into pools
        self.restore_leases().await?;

        let mut buf = vec![0u8; 1500];
        let mut shutdown = shutdown;

        loop {
            // Open a fresh recv socket each iteration. RouterOS container
            // networking bug: any send_to() from inside the container corrupts
            // the veth receive path. Closing and reopening both the recv and
            // send sockets between each transaction resets the veth state.
            let recv_socket = match bind_recv_socket(primary_port) {
                Ok(s) => s,
                Err(e) => {
                    warn!("DHCP bind failed on port {primary_port}: {e}, retrying in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            let recv_result = tokio::select! {
                r = recv_socket.recv_from(&mut buf) => r,
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    info!("DHCP recv timeout, recycling socket");
                    drop(recv_socket);
                    continue;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("DHCPv4 server shutting down");
                        break;
                    }
                    continue;
                }
            };

            // Drop recv socket BEFORE sending — frees the veth state
            drop(recv_socket);

            let (len, src) = match recv_result {
                Ok(v) => v,
                Err(e) => {
                    warn!("DHCP recv error: {e}");
                    continue;
                }
            };

            let packet = match DhcpPacket::parse(&buf[..len]) {
                Some(p) => p,
                None => {
                    debug!("invalid DHCP packet from {src}");
                    continue;
                }
            };

            if packet.op != 1 {
                continue;
            }

            // Skip raw broadcast packets (giaddr=0) — only process relay
            // unicasts. RouterOS bridges flood DHCP broadcasts to all ports
            // including container veths. Sending a response to a broadcast
            // destination corrupts the veth receive path. Relay packets have
            // giaddr set and responses go unicast to the relay, which is safe.
            if packet.giaddr == Ipv4Addr::UNSPECIFIED {
                debug!("ignoring raw broadcast DHCP from {src} (no relay giaddr)");
                continue;
            }

            let response = match self.handle_packet(&packet).await {
                Ok(Some(resp)) => resp,
                Ok(None) => continue,
                Err(e) => {
                    warn!("error handling DHCP packet: {e}");
                    continue;
                }
            };

            let dest = if packet.giaddr != Ipv4Addr::UNSPECIFIED {
                SocketAddr::new(packet.giaddr.into(), 67)
            } else if packet.flags & 0x8000 != 0 {
                SocketAddr::new(Ipv4Addr::BROADCAST.into(), 68)
            } else if response.yiaddr != Ipv4Addr::UNSPECIFIED {
                SocketAddr::new(response.yiaddr.into(), 68)
            } else {
                SocketAddr::new(Ipv4Addr::BROADCAST.into(), 68)
            };

            let resp_bytes = response.to_bytes();
            // Fresh send socket — created, used once, then dropped
            info!("sending DHCP response ({} bytes) to {dest}", resp_bytes.len());
            match send_one_shot(&resp_bytes, dest) {
                Ok(n) => debug!("sent DHCP response to {dest} ({n} bytes)"),
                Err(e) => error!("failed to send DHCP response to {dest}: {e}"),
            }
            // send socket dropped here, recv socket reopens at top of loop
        }

        Ok(())
    }

    async fn handle_packet(
        &self,
        request: &DhcpPacket,
    ) -> anyhow::Result<Option<DhcpPacket>> {
        let msg_type = match request.message_type() {
            Some(t) => t,
            None => return Ok(None),
        };

        let mac = request.mac_address();
        info!("DHCP {msg_type:?} from {mac} (xid: {:08x})", request.xid);

        match msg_type {
            DhcpMessageType::Discover => self.handle_discover(request).await,
            DhcpMessageType::Request => self.handle_request(request).await,
            DhcpMessageType::Release => {
                self.handle_release(request).await?;
                Ok(None) // No response for Release
            }
            _ => Ok(None),
        }
    }

    /// Handle DHCP DISCOVER: allocate an IP and send OFFER.
    async fn handle_discover(
        &self,
        request: &DhcpPacket,
    ) -> anyhow::Result<Option<DhcpPacket>> {
        let mac = request.mac_address();

        // Check static reservations first
        if let Some((reserved_ip, _hostname)) = self.reservations.get(&mac) {
            let ip = *reserved_ip;
            // Mark as allocated in pool so it's not given to someone else
            let mut pools = self.pools.lock().await;
            for pool in pools.iter_mut() {
                pool.mark_allocated(ip);
            }
            info!("offering reserved IP {ip} to {mac}");
            return Ok(Some(self.build_offer(request, ip).await));
        }

        // Check if client already has a lease
        if let Some(existing) = self.lease_manager.find_lease_by_mac(&mac)? {
            let ip: Ipv4Addr = existing.ip_addr.parse()?;
            info!("offering existing lease {ip} to {mac}");
            return Ok(Some(self.build_offer(request, ip).await));
        }

        // Try requested IP first
        if let Some(requested) = request.requested_ip() {
            let mut pools = self.pools.lock().await;
            for pool in pools.iter_mut() {
                if pool.allocate_specific(requested) {
                    info!("offering requested IP {requested} to {mac}");
                    return Ok(Some(self.build_offer(request, requested).await));
                }
            }
        }

        // Allocate from pool
        let mut pools = self.pools.lock().await;
        for pool in pools.iter_mut() {
            if let Some(ip) = pool.allocate() {
                info!("offering {ip} to {mac}");
                return Ok(Some(self.build_offer(request, ip).await));
            }
        }

        warn!("no available IPs for {mac}");
        Ok(None)
    }

    /// Handle DHCP REQUEST: confirm allocation and send ACK.
    async fn handle_request(
        &self,
        request: &DhcpPacket,
    ) -> anyhow::Result<Option<DhcpPacket>> {
        let mac = request.mac_address();
        let requested_ip = request
            .requested_ip()
            .or(if request.ciaddr != Ipv4Addr::UNSPECIFIED {
                Some(request.ciaddr)
            } else {
                None
            });

        let ip = match requested_ip {
            Some(ip) => ip,
            None => return Ok(Some(self.build_nak(request).await)),
        };

        // Validate against reservation if one exists
        if let Some((reserved_ip, _)) = self.reservations.get(&mac) {
            if *reserved_ip != ip {
                warn!("client {mac} requested {ip} but has reservation for {reserved_ip}");
                return Ok(Some(self.build_nak(request).await));
            }
        }

        // Use reservation hostname if client didn't provide one
        let hostname = request.hostname().or_else(|| {
            self.reservations
                .get(&mac)
                .and_then(|(_, h)| h.clone())
        });

        let pool_info = {
            let pools = self.pools.lock().await;
            pools.iter().find(|p| p.contains(ip)).map(|p| {
                (p.lease_time_secs, p.domain.clone())
            })
        };

        // For reserved IPs outside pool range, use first pool's info
        let (lease_time, domain) = match pool_info {
            Some(info) => info,
            None => {
                // Check if this is a reservation — allow it even outside pool range
                if self.reservations.get(&mac).is_some() {
                    let pools = self.pools.lock().await;
                    pools
                        .first()
                        .map(|p| (p.lease_time_secs, p.domain.clone()))
                        .unwrap_or((3600, String::new()))
                } else {
                    return Ok(Some(self.build_nak(request).await));
                }
            }
        };

        // Create lease
        self.lease_manager.create_lease(
            &ip.to_string(),
            &mac,
            hostname.as_deref(),
            lease_time,
            &domain,
        )?;

        info!("ACK: assigned {ip} to {mac} (lease: {lease_time}s)");

        // Publish lease event
        if let Some(ref bus) = self.message_bus {
            let event = Event::LeaseCreated {
                instance_id: self.instance_id.clone(),
                ip_addr: ip.to_string(),
                mac_addr: mac.clone(),
                hostname: hostname.clone(),
                pool_id: domain.clone(),
                timestamp: chrono::Utc::now(),
            };
            if let Err(e) = bus.publish(&event).await {
                warn!("failed to publish LeaseCreated event: {e}");
            }
        }

        // DNS auto-registration
        if let (Some(ref registrar), Some(ref name)) = (&self.dns_registrar, &hostname) {
            if let Err(e) = registrar.register_v4(name, ip) {
                warn!("DNS registration failed for {name}/{ip}: {e}");
            }
        }

        Ok(Some(self.build_ack(request, ip).await))
    }

    /// Handle DHCP RELEASE: release the lease.
    async fn handle_release(&self, request: &DhcpPacket) -> anyhow::Result<()> {
        let mac = request.mac_address();
        let ip = request.ciaddr;

        if ip == Ipv4Addr::UNSPECIFIED {
            return Ok(());
        }

        // DNS unregistration
        if let Some(ref registrar) = self.dns_registrar {
            if let Some(existing) = self.lease_manager.find_lease_by_mac(&mac)? {
                if let Some(ref hostname) = existing.hostname {
                    if let Err(e) = registrar.unregister(hostname) {
                        warn!("DNS unregistration failed for {hostname}: {e}");
                    }
                }
            }
        }

        // Release from pool (skip for reservations — they stay allocated)
        if !self.reservations.contains_key(&mac) {
            let mut pools = self.pools.lock().await;
            for pool in pools.iter_mut() {
                pool.release(&ip);
            }
        }

        // Release lease in DB
        self.lease_manager.release_lease_by_mac(&mac)?;
        info!("released {ip} from {mac}");

        // Publish release event
        if let Some(ref bus) = self.message_bus {
            let event = Event::LeaseReleased {
                instance_id: self.instance_id.clone(),
                ip_addr: ip.to_string(),
                mac_addr: mac.clone(),
                timestamp: chrono::Utc::now(),
            };
            if let Err(e) = bus.publish(&event).await {
                warn!("failed to publish LeaseReleased event: {e}");
            }
        }

        Ok(())
    }

    async fn build_offer(&self, request: &DhcpPacket, ip: Ipv4Addr) -> DhcpPacket {
        self.build_response(request, ip, DhcpMessageType::Offer).await
    }

    async fn build_ack(&self, request: &DhcpPacket, ip: Ipv4Addr) -> DhcpPacket {
        self.build_response(request, ip, DhcpMessageType::Ack).await
    }

    async fn build_nak(&self, request: &DhcpPacket) -> DhcpPacket {
        DhcpPacket {
            op: 2, // BOOTREPLY
            htype: request.htype,
            hlen: request.hlen,
            hops: 0,
            xid: request.xid,
            secs: 0,
            flags: request.flags,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: self.server_ip,
            giaddr: request.giaddr,
            chaddr: request.chaddr,
            sname: [0u8; 64],
            file: [0u8; 128],
            options: vec![
                message_type_option(DhcpMessageType::Nak),
                ip_option(OPT_SERVER_ID, self.server_ip),
                DhcpOption {
                    code: OPT_END,
                    data: Vec::new(),
                },
            ],
        }
    }

    async fn build_response(
        &self,
        request: &DhcpPacket,
        ip: Ipv4Addr,
        msg_type: DhcpMessageType,
    ) -> DhcpPacket {
        let pools = self.pools.lock().await;
        let pool_idx = pools.iter().position(|p| p.contains(ip));
        let pool = pool_idx.map(|i| &pools[i]);

        let mut options = vec![message_type_option(msg_type)];
        options.push(ip_option(OPT_SERVER_ID, self.server_ip));

        // For reserved IPs outside pool range, use first pool's options
        let effective_pool = pool.or(pools.first());

        let mut siaddr = self.server_ip;
        let mut sname = [0u8; 64];
        let mut file = [0u8; 128];

        if let Some(pool) = effective_pool {
            options.push(ip_option(OPT_SUBNET_MASK, pool.subnet_mask));
            options.push(ip_option(OPT_ROUTER, pool.gateway));
            options.push(u32_option(OPT_LEASE_TIME, pool.lease_time_secs));

            if !pool.dns_servers.is_empty() {
                options.push(ip_list_option(OPT_DNS_SERVER, &pool.dns_servers));
            }

            if !pool.domain.is_empty() {
                options.push(string_option(OPT_DOMAIN_NAME, &pool.domain));
            }
        }

        // PXE boot options — detect iPXE clients via option 175 (iPXE
        // encapsulated options, always sent by iPXE) or user-class "iPXE".
        let pxe_idx = pool_idx.unwrap_or(0);
        if let Some(Some(ref pxe)) = self.pxe_configs.get(pxe_idx) {
            let is_ipxe = request.get_option(OPT_IPXE_ENCAP).is_some()
                || request
                    .get_option(OPT_USER_CLASS)
                    .map(|d| d.windows(4).any(|w| w == b"iPXE"))
                    .unwrap_or(false);

            let boot_file = if is_ipxe {
                if let Some(ref url) = pxe.ipxe_boot_url {
                    info!("iPXE client detected, serving boot URL: {}", url);
                    url.as_str()
                } else {
                    &pxe.boot_file
                }
            } else {
                &pxe.boot_file
            };

            // Only set siaddr/sname for TFTP boots (non-iPXE)
            if !is_ipxe {
                siaddr = pxe.next_server;
                let ns_str = pxe.next_server.to_string();
                let ns_bytes = ns_str.as_bytes();
                let len = ns_bytes.len().min(63);
                sname[..len].copy_from_slice(&ns_bytes[..len]);
                options.push(string_option(OPT_TFTP_SERVER, &pxe.next_server.to_string()));
            }

            options.push(string_option(OPT_BOOTFILE, boot_file));

            // Populate file field with boot filename
            let bf_bytes = boot_file.as_bytes();
            let len = bf_bytes.len().min(127);
            file[..len].copy_from_slice(&bf_bytes[..len]);
        }

        options.push(DhcpOption {
            code: OPT_END,
            data: Vec::new(),
        });

        // When responding via relay (giaddr set), force broadcast flag so the
        // relay broadcasts the response to the client. Without this, the relay
        // tries to unicast to yiaddr which fails because the client has no ARP
        // entry for that IP yet.
        let flags = if request.giaddr != Ipv4Addr::UNSPECIFIED {
            request.flags | 0x8000
        } else {
            request.flags
        };

        DhcpPacket {
            op: 2, // BOOTREPLY
            htype: request.htype,
            hlen: request.hlen,
            hops: 0,
            xid: request.xid,
            secs: 0,
            flags,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: ip,
            siaddr,
            giaddr: request.giaddr,
            chaddr: request.chaddr,
            sname,
            file,
            options,
        }
    }

    /// Restore active leases and mark reservation IPs as allocated on startup.
    async fn restore_leases(&self) -> anyhow::Result<()> {
        let leases = self.lease_manager.list_active_leases()?;
        let mut pools = self.pools.lock().await;

        for lease in &leases {
            if let Ok(ip) = lease.ip_addr.parse::<Ipv4Addr>() {
                for pool in pools.iter_mut() {
                    pool.mark_allocated(ip);
                }
            }
        }

        // Pre-allocate all reservation IPs so they're never given to other clients
        for (_mac, (ip, _hostname)) in &self.reservations {
            for pool in pools.iter_mut() {
                pool.mark_allocated(*ip);
            }
        }

        info!(
            "restored {} active leases, {} reservations",
            leases.len(),
            self.reservations.len()
        );
        Ok(())
    }

    pub fn lease_manager(&self) -> &LeaseManager {
        &self.lease_manager
    }
}

/// Create a fresh recv socket bound to the given port with SO_REUSEADDR.
fn bind_recv_socket(port: u16) -> anyhow::Result<UdpSocket> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
    Ok(UdpSocket::from_std(sock.into())?)
}

/// Create a one-shot UDP socket, send the data, then let it drop.
/// Binds to port 67 (DHCP server port) as required by RFC 2131 — relays
/// expect responses from the server's well-known port.
/// Uses a 2-second send timeout to prevent blocking the async runtime
/// if the send stalls (e.g. ARP resolution hangs on RouterOS veths).
fn send_one_shot(data: &[u8], dest: SocketAddr) -> std::io::Result<usize> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    sock.set_reuse_address(true)?;
    sock.set_broadcast(true)?;
    sock.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
    sock.bind(&SocketAddr::from(([0, 0, 0, 0], 67)).into())?;
    sock.send_to(data, &dest.into())
}
