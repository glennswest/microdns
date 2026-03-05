use crate::dns_register::DnsRegistrar;
use crate::lease::LeaseManager;
use crate::v4::packet::*;
use crate::v4::pool::{prefix_len_from_subnet, subnet_mask_from_prefix, Ipv4Pool};
use microdns_core::config::{DhcpMode, DhcpV4Config};
use microdns_core::db::Db;
use microdns_core::types::DhcpDbReservation;
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
    /// EFI boot file for UEFI clients (e.g. ipxe.efi)
    boot_file_efi: Option<String>,
    /// HTTP boot script URL served to iPXE clients instead of the TFTP boot file
    ipxe_boot_url: Option<String>,
}

pub struct Dhcpv4Server {
    _config: Option<DhcpV4Config>,
    db: Db,
    mode: DhcpMode,
    pools: Arc<Mutex<Vec<Ipv4Pool>>>,
    /// PXE config per pool index
    pxe_configs: Vec<Option<PxeConfig>>,
    /// MAC → (IP, hostname) from config file reservations (immutable at startup)
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
                    boot_file_efi: pool_cfg.boot_file_efi.clone(),
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

        let lease_manager = Arc::new(LeaseManager::new(db.clone()));

        Ok(Self {
            _config: Some(config.clone()),
            db,
            mode: config.mode,
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

    /// Create a DHCP server that loads pool definitions from the database.
    /// Reservations are read from DB on every request — no in-memory cache.
    pub fn from_db(
        db: Db,
        mode: DhcpMode,
        server_ip: Option<Ipv4Addr>,
    ) -> anyhow::Result<Self> {
        let db_pools = db.list_dhcp_pools().unwrap_or_default();
        let mut pools = Vec::new();
        let mut pxe_configs = Vec::new();

        for pool_cfg in &db_pools {
            let prefix_len = prefix_len_from_subnet(&pool_cfg.subnet).unwrap_or(24);
            let mask = subnet_mask_from_prefix(prefix_len);
            let dns_servers: Vec<Ipv4Addr> = pool_cfg
                .dns_servers
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
                    boot_file_efi: pool_cfg.boot_file_efi.clone(),
                    ipxe_boot_url: pool_cfg.ipxe_boot_url.clone(),
                }),
                _ => None,
            };
            pxe_configs.push(pxe);
        }

        let effective_server_ip = server_ip
            .or_else(|| pools.first().map(|p| p.gateway))
            .unwrap_or(Ipv4Addr::UNSPECIFIED);

        let lease_manager = Arc::new(LeaseManager::new(db.clone()));

        Ok(Self {
            _config: None,
            db,
            mode,
            pools: Arc::new(Mutex::new(pools)),
            pxe_configs,
            reservations: HashMap::new(), // DB-only mode — no config file reservations
            server_ip: effective_server_ip,
            lease_manager,
            dns_registrar: None,
            message_bus: None,
            instance_id: String::new(),
        })
    }

    /// Look up a reservation by MAC. Checks database first, then falls back
    /// to config-file reservations.
    fn get_reservation(&self, mac: &str) -> Option<(Ipv4Addr, Option<String>)> {
        // Database is the live source of truth
        if let Ok(Some(res)) = self.db.get_dhcp_reservation(mac) {
            if let Ok(ip) = res.ip.parse() {
                return Some((ip, res.hostname.clone()));
            }
        }
        // Fall back to static config-file reservations
        self.reservations.get(mac).cloned()
    }

    /// Get full reservation details from DB (for per-reservation option overrides).
    fn get_db_reservation(&self, mac: &str) -> Option<DhcpDbReservation> {
        self.db.get_dhcp_reservation(mac).ok().flatten()
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
        let primary_port = self
            ._config
            .as_ref()
            .and_then(|c| c.listen_ports.first().copied())
            .unwrap_or(67);

        info!(
            "DHCPv4 server listening on 0.0.0.0:{primary_port} (mode: {:?})",
            self.mode
        );

        // Restore existing leases into pools
        self.restore_leases().await?;

        match self.mode {
            DhcpMode::Normal => self.run_normal(primary_port, shutdown).await,
            DhcpMode::Gateway => self.run_gateway(primary_port, shutdown).await,
        }
    }

    /// Normal mode: accept all DHCP packets (broadcast and relay).
    /// Single persistent socket, no deadman timer, no veth workarounds.
    async fn run_normal(
        &self,
        port: u16,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let socket = loop {
            match bind_recv_socket(port) {
                Ok(s) => break s,
                Err(e) => {
                    warn!("DHCP bind failed on port {port}: {e}, retrying in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        };
        // Enable broadcast so we can send replies to 255.255.255.255
        socket.set_broadcast(true)?;

        let mut buf = vec![0u8; 1500];
        let mut pool_sync = tokio::time::interval(Duration::from_secs(60));

        loop {
            let recv_result = tokio::select! {
                r = socket.recv_from(&mut buf) => r,
                _ = pool_sync.tick() => {
                    if let Err(e) = self.sync_pool().await {
                        warn!("pool sync error: {e}");
                    }
                    continue;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("DHCPv4 server shutting down");
                        return Ok(());
                    }
                    continue;
                }
            };

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

            let response = match self.handle_packet(&packet).await {
                Ok(Some(resp)) => resp,
                Ok(None) => continue,
                Err(e) => {
                    warn!("error handling DHCP packet: {e}");
                    continue;
                }
            };

            // Determine response destination.  When the client has no IP
            // yet (ciaddr==0) we MUST broadcast — unicast to yiaddr fails
            // because ARP resolution for an IP the client doesn't have yet
            // is dropped by the network stack.
            let dest = if packet.giaddr != Ipv4Addr::UNSPECIFIED {
                SocketAddr::new(packet.giaddr.into(), 67)
            } else if packet.ciaddr == Ipv4Addr::UNSPECIFIED
                || packet.flags & 0x8000 != 0
            {
                SocketAddr::new(Ipv4Addr::BROADCAST.into(), 68)
            } else {
                SocketAddr::new(packet.ciaddr.into(), 68)
            };

            let resp_bytes = response.to_bytes();
            info!("sending DHCP response ({} bytes) to {dest}", resp_bytes.len());
            match socket.send_to(&resp_bytes, dest).await {
                Ok(n) => debug!("sent DHCP response to {dest} ({n} bytes)"),
                Err(e) => error!("failed to send DHCP response to {dest}: {e}"),
            }
        }
    }

    /// Gateway mode: only accept relay-forwarded packets (giaddr != 0).
    /// Includes veth deadman timer and socket recycling to work around
    /// RouterOS container networking bugs.
    async fn run_gateway(
        &self,
        port: u16,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let mut buf = vec![0u8; 1500];
        let mut last_relay_packet = tokio::time::Instant::now();
        let mut pool_sync = tokio::time::interval(Duration::from_secs(60));

        'outer: loop {
            let recv_socket = match bind_recv_socket(port) {
                Ok(s) => s,
                Err(e) => {
                    warn!("DHCP bind failed on port {port}: {e}, retrying in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            loop {
                let deadline = last_relay_packet + Duration::from_secs(30);

                let recv_result = tokio::select! {
                    r = recv_socket.recv_from(&mut buf) => r,
                    _ = pool_sync.tick() => {
                        if let Err(e) = self.sync_pool().await {
                            warn!("pool sync error: {e}");
                        }
                        continue;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        info!("DHCP deadman: no relay packets for 30s, recycling socket");
                        drop(recv_socket);
                        last_relay_packet = tokio::time::Instant::now();
                        continue 'outer;
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!("DHCPv4 server shutting down");
                            break 'outer;
                        }
                        continue;
                    }
                };

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

                if packet.giaddr == Ipv4Addr::UNSPECIFIED {
                    debug!("ignoring raw broadcast DHCP from {src} (gateway mode, no relay giaddr)");
                    continue;
                }

                last_relay_packet = tokio::time::Instant::now();
                drop(recv_socket);

                let response = match self.handle_packet(&packet).await {
                    Ok(Some(resp)) => resp,
                    Ok(None) => continue 'outer,
                    Err(e) => {
                        warn!("error handling DHCP packet: {e}");
                        continue 'outer;
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
                info!("sending DHCP response ({} bytes) to {dest}", resp_bytes.len());
                match send_one_shot(&resp_bytes, dest) {
                    Ok(n) => debug!("sent DHCP response to {dest} ({n} bytes)"),
                    Err(e) => error!("failed to send DHCP response to {dest}: {e}"),
                }
                continue 'outer;
            }
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

        // Check reservations (reads from database, falls back to config)
        if let Some((reserved_ip, _hostname)) = self.get_reservation(&mac) {
            let ip = reserved_ip;
            // Mark as allocated in pool so it's not given to someone else
            {
                let mut pools = self.pools.lock().await;
                for pool in pools.iter_mut() {
                    pool.mark_allocated(ip);
                }
            } // drop lock before build_offer (which also locks pools)
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
            let allocated = {
                let mut pools = self.pools.lock().await;
                pools.iter_mut().any(|pool| pool.allocate_specific(requested))
            }; // drop lock before build_offer
            if allocated {
                info!("offering requested IP {requested} to {mac}");
                return Ok(Some(self.build_offer(request, requested).await));
            }
        }

        // Allocate from pool
        let allocated_ip = {
            let mut pools = self.pools.lock().await;
            pools.iter_mut().find_map(|pool| pool.allocate())
        }; // drop lock before build_offer
        if let Some(ip) = allocated_ip {
            info!("offering {ip} to {mac}");
            return Ok(Some(self.build_offer(request, ip).await));
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
        if let Some((reserved_ip, _)) = self.get_reservation(&mac) {
            if reserved_ip != ip {
                warn!("client {mac} requested {ip} but has reservation for {reserved_ip}");
                return Ok(Some(self.build_nak(request).await));
            }
        }

        // Use reservation hostname if client didn't provide one
        let hostname = request.hostname().or_else(|| {
            self.get_reservation(&mac).and_then(|(_, h)| h)
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
                if self.get_reservation(&mac).is_some() {
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
        if self.get_reservation(&mac).is_none() {
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
        let mac = request.mac_address();
        // Read per-reservation overrides directly from database
        let db_res = self.get_db_reservation(&mac);

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

            // Gateway: per-reservation override or pool default
            let gw = db_res
                .as_ref()
                .and_then(|r| r.gateway.as_ref())
                .and_then(|s| s.parse().ok())
                .unwrap_or(pool.gateway);
            options.push(ip_option(OPT_ROUTER, gw));

            // Lease time: per-reservation override or pool default
            let lease_time = db_res
                .as_ref()
                .and_then(|r| r.lease_time_secs)
                .map(|v| v as u32)
                .unwrap_or(pool.lease_time_secs);
            options.push(u32_option(OPT_LEASE_TIME, lease_time));

            // DNS servers
            let override_dns: Option<Vec<Ipv4Addr>> = db_res.as_ref().and_then(|r| {
                r.dns_servers.as_ref().map(|v| {
                    v.iter().filter_map(|s| s.parse().ok()).collect()
                })
            });
            let dns = override_dns.as_deref().unwrap_or(&pool.dns_servers);
            if !dns.is_empty() {
                options.push(ip_list_option(OPT_DNS_SERVER, dns));
            }

            // Domain name
            let domain = db_res
                .as_ref()
                .and_then(|r| r.domain.as_deref())
                .unwrap_or(&pool.domain);
            if !domain.is_empty() {
                options.push(string_option(OPT_DOMAIN_NAME, domain));
            }
        }

        // Extended options from per-reservation overrides (read from DB)
        if let Some(ref res) = db_res {
            if let Some(ref ntp) = res.ntp_servers {
                let addrs: Vec<Ipv4Addr> = ntp.iter().filter_map(|s| s.parse().ok()).collect();
                if !addrs.is_empty() {
                    options.push(ip_list_option(OPT_NTP_SERVERS, &addrs));
                }
            }
            if let Some(mtu) = res.mtu {
                options.push(u16_option(OPT_MTU, mtu));
            }
            if let Some(ref domains) = res.domain_search {
                if !domains.is_empty() {
                    options.push(domain_search_option(domains));
                }
            }
            if let Some(ref routes) = res.static_routes {
                let parsed: Vec<(String, Ipv4Addr)> = routes
                    .iter()
                    .filter_map(|r| r.gateway.parse().ok().map(|gw| (r.destination.clone(), gw)))
                    .collect();
                if !parsed.is_empty() {
                    options.push(classless_static_routes_option(&parsed));
                }
            }
            if let Some(ref log_srv) = res.log_server {
                if let Ok(addr) = log_srv.parse() {
                    options.push(ip_option(OPT_LOG_SERVER, addr));
                }
            }
            if let Some(offset) = res.time_offset {
                options.push(i32_option(OPT_TIME_OFFSET, offset));
            }
            if let Some(ref wpad) = res.wpad_url {
                options.push(string_option(OPT_WPAD, wpad));
            }
        }

        // PXE boot options — detect iPXE clients via option 175 (iPXE
        // encapsulated options, always sent by iPXE) or user-class "iPXE".
        // Per-reservation PXE overrides take precedence over pool PXE config.
        let pxe_idx = pool_idx.unwrap_or(0);
        let pool_pxe = self.pxe_configs.get(pxe_idx).and_then(|p| p.as_ref());

        // Build effective PXE config from per-reservation overrides or pool config
        let effective_next_server = db_res
            .as_ref()
            .and_then(|r| r.next_server.as_ref())
            .and_then(|s| s.parse().ok())
            .or_else(|| pool_pxe.map(|p| p.next_server));
        let effective_boot_file = db_res
            .as_ref()
            .and_then(|r| r.boot_file.clone())
            .or_else(|| pool_pxe.map(|p| p.boot_file.clone()));
        let effective_boot_file_efi = db_res
            .as_ref()
            .and_then(|r| r.boot_file_efi.clone())
            .or_else(|| pool_pxe.and_then(|p| p.boot_file_efi.clone()));
        let effective_ipxe_url = db_res
            .as_ref()
            .and_then(|r| r.ipxe_boot_url.clone())
            .or_else(|| pool_pxe.and_then(|p| p.ipxe_boot_url.clone()));

        if let (Some(next_srv), Some(ref bf)) = (effective_next_server, &effective_boot_file) {
            let is_ipxe = request.get_option(OPT_IPXE_ENCAP).is_some()
                || request
                    .get_option(OPT_USER_CLASS)
                    .map(|d| d.windows(4).any(|w| w == b"iPXE"))
                    .unwrap_or(false);

            let is_efi = request
                .get_option(OPT_CLIENT_ARCH)
                .map(|d| {
                    if d.len() >= 2 {
                        let arch = u16::from_be_bytes([d[0], d[1]]);
                        matches!(arch, 6 | 7 | 9 | 10 | 11)
                    } else {
                        false
                    }
                })
                .unwrap_or(false);

            let boot_file = if is_ipxe {
                if let Some(ref url) = effective_ipxe_url {
                    info!("iPXE client detected, serving boot URL: {}", url);
                    url.as_str()
                } else {
                    bf.as_str()
                }
            } else if is_efi {
                if let Some(ref efi_file) = effective_boot_file_efi {
                    info!("UEFI client detected, serving EFI boot file: {}", efi_file);
                    efi_file.as_str()
                } else {
                    warn!("UEFI client detected but no boot_file_efi configured, falling back to BIOS boot file");
                    bf.as_str()
                }
            } else {
                bf.as_str()
            };

            if !is_ipxe {
                siaddr = next_srv;
                let ns_str = next_srv.to_string();
                let ns_bytes = ns_str.as_bytes();
                let len = ns_bytes.len().min(63);
                sname[..len].copy_from_slice(&ns_bytes[..len]);
                options.push(string_option(OPT_TFTP_SERVER, &next_srv.to_string()));
            }

            options.push(string_option(OPT_BOOTFILE, boot_file));

            let bf_bytes = boot_file.as_bytes();
            let len = bf_bytes.len().min(127);
            file[..len].copy_from_slice(&bf_bytes[..len]);
        }

        options.push(DhcpOption {
            code: OPT_END,
            data: Vec::new(),
        });

        // When responding via relay (giaddr set), force broadcast flag
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
        let db_reservations = self.db.list_dhcp_reservations().unwrap_or_default();
        let mut pools = self.pools.lock().await;

        for lease in &leases {
            if let Ok(ip) = lease.ip_addr.parse::<Ipv4Addr>() {
                for pool in pools.iter_mut() {
                    pool.mark_allocated(ip);
                }
            }
        }

        // Pre-allocate all reservation IPs so they're never given to other clients
        // (both DB reservations and config-file reservations)
        for res in &db_reservations {
            if let Ok(ip) = res.ip.parse::<Ipv4Addr>() {
                for pool in pools.iter_mut() {
                    pool.mark_allocated(ip);
                }
            }
        }
        for (_mac, (ip, _hostname)) in &self.reservations {
            for pool in pools.iter_mut() {
                pool.mark_allocated(*ip);
            }
        }

        info!(
            "restored {} active leases, {} DB reservations, {} config reservations",
            leases.len(),
            db_reservations.len(),
            self.reservations.len()
        );
        Ok(())
    }

    /// Re-synchronise the pool's in-memory allocated set with the database.
    /// This frees IPs from expired/released leases that were never explicitly
    /// released by the client (most clients skip DHCP Release).
    async fn sync_pool(&self) -> anyhow::Result<()> {
        let active_leases = self.lease_manager.list_active_leases()?;
        let db_reservations = self.db.list_dhcp_reservations().unwrap_or_default();
        let mut pools = self.pools.lock().await;

        for pool in pools.iter_mut() {
            pool.clear_allocated();
        }

        // Re-mark active leases
        for lease in &active_leases {
            if let Ok(ip) = lease.ip_addr.parse::<Ipv4Addr>() {
                for pool in pools.iter_mut() {
                    pool.mark_allocated(ip);
                }
            }
        }

        // Re-mark reservations from DB + config (must never be handed out to other clients)
        for res in &db_reservations {
            if let Ok(ip) = res.ip.parse::<Ipv4Addr>() {
                for pool in pools.iter_mut() {
                    pool.mark_allocated(ip);
                }
            }
        }
        for (_mac, (ip, _hostname)) in &self.reservations {
            for pool in pools.iter_mut() {
                pool.mark_allocated(*ip);
            }
        }

        let avail: u32 = pools.iter().map(|p| p.available_count()).sum();
        debug!(
            "pool sync: {} active leases, {} reservations, {} available",
            active_leases.len(),
            db_reservations.len() + self.reservations.len(),
            avail
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
