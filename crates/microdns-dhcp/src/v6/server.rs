use crate::lease::LeaseManager;
use crate::v6::packet::*;
use microdns_core::config::DhcpV6Config;
use microdns_core::db::Db;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

pub struct Dhcpv6Server {
    _config: DhcpV6Config,
    prefix: Ipv6Addr,
    _prefix_len: u8,
    dns_servers: Vec<Ipv6Addr>,
    lease_time_secs: u32,
    lease_manager: Arc<LeaseManager>,
    /// Counter for address allocation within the prefix
    addr_counter: AtomicU64,
    server_duid: Dhcpv6Option,
}

impl Dhcpv6Server {
    pub fn new(config: &DhcpV6Config, db: Db) -> anyhow::Result<Self> {
        let pool = config.pools.first().ok_or_else(|| {
            anyhow::anyhow!("DHCPv6 requires at least one pool")
        })?;

        let prefix: Ipv6Addr = pool.prefix.parse()?;
        let dns_servers: Vec<Ipv6Addr> = pool
            .dns
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        // Use a simple MAC for server DUID
        let server_mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let server_duid = build_server_id(&server_mac);

        Ok(Self {
            _config: config.clone(),
            prefix,
            _prefix_len: pool.prefix_len,
            dns_servers,
            lease_time_secs: pool.lease_time_secs as u32,
            lease_manager: Arc::new(LeaseManager::new(db)),
            addr_counter: AtomicU64::new(0x100),
            server_duid,
        })
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let socket = UdpSocket::bind("[::]:547").await?;
        info!("DHCPv6 server listening on [::]:547");

        let mut buf = vec![0u8; 1500];
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    let (len, src) = result?;

                    let packet = match Dhcpv6Packet::parse(&buf[..len]) {
                        Some(p) => p,
                        None => continue,
                    };

                    if let Some(response) = self.handle_packet(&packet, &src).await {
                        let resp_bytes = response.to_bytes();
                        if let Err(e) = socket.send_to(&resp_bytes, src).await {
                            error!("failed to send DHCPv6 response: {e}");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("DHCPv6 server shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_packet(
        &self,
        request: &Dhcpv6Packet,
        src: &SocketAddr,
    ) -> Option<Dhcpv6Packet> {
        let msg_type = request.message_type()?;
        debug!("DHCPv6 {msg_type:?} from {src}");

        match msg_type {
            Dhcpv6MessageType::Solicit => self.handle_solicit(request).await,
            Dhcpv6MessageType::Request => self.handle_request(request).await,
            Dhcpv6MessageType::Release => {
                self.handle_release(request).await;
                None
            }
            _ => None,
        }
    }

    async fn handle_solicit(&self, request: &Dhcpv6Packet) -> Option<Dhcpv6Packet> {
        let client_id = request.get_option(OPT_CLIENTID)?;
        let addr = self.allocate_address();

        let mut options = vec![
            client_id.clone(),
            self.server_duid.clone(),
            build_ia_na(1, addr, self.lease_time_secs, self.lease_time_secs),
        ];

        if !self.dns_servers.is_empty() {
            options.push(build_dns_option(&self.dns_servers));
        }

        Some(Dhcpv6Packet {
            msg_type: Dhcpv6MessageType::Advertise as u8,
            transaction_id: request.transaction_id,
            options,
        })
    }

    async fn handle_request(&self, request: &Dhcpv6Packet) -> Option<Dhcpv6Packet> {
        let client_id = request.get_option(OPT_CLIENTID)?;
        let addr = self.allocate_address();

        // Create lease
        let client_duid = hex::encode(&client_id.data);
        if let Err(e) = self.lease_manager.create_lease(
            &addr.to_string(),
            &client_duid,
            None,
            self.lease_time_secs,
            "dhcpv6",
        ) {
            warn!("failed to create DHCPv6 lease: {e}");
        }

        info!("DHCPv6: assigned {addr} to {client_duid}");

        let mut options = vec![
            client_id.clone(),
            self.server_duid.clone(),
            build_ia_na(1, addr, self.lease_time_secs, self.lease_time_secs),
        ];

        if !self.dns_servers.is_empty() {
            options.push(build_dns_option(&self.dns_servers));
        }

        Some(Dhcpv6Packet {
            msg_type: Dhcpv6MessageType::Reply as u8,
            transaction_id: request.transaction_id,
            options,
        })
    }

    async fn handle_release(&self, request: &Dhcpv6Packet) {
        if let Some(client_id) = request.get_option(OPT_CLIENTID) {
            let client_duid = hex::encode(&client_id.data);
            if let Err(e) = self.lease_manager.release_lease_by_mac(&client_duid) {
                warn!("failed to release DHCPv6 lease: {e}");
            }
        }
    }

    /// Allocate the next IPv6 address from the prefix.
    fn allocate_address(&self) -> Ipv6Addr {
        let counter = self.addr_counter.fetch_add(1, Ordering::Relaxed);
        let prefix_bits = u128::from(self.prefix);
        let addr = prefix_bits | (counter as u128);
        Ipv6Addr::from(addr)
    }

    pub fn lease_manager(&self) -> &LeaseManager {
        &self.lease_manager
    }
}

/// Simple hex encoding utility (avoid adding a dependency for this)
mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect()
    }
}
