use microdns_core::config::SlaacConfig;
use std::net::Ipv6Addr;
use tokio::sync::watch;
use tracing::info;

/// Router Advertisement daemon for SLAAC.
/// Periodically sends Router Advertisement messages with prefix information
/// so that hosts can auto-configure their IPv6 addresses.
///
/// Note: Full RA implementation requires raw sockets (ICMPv6) which need
/// CAP_NET_RAW. This is a stub that will be completed when running in
/// a container with appropriate capabilities.
pub struct RaDaemon {
    prefix: Ipv6Addr,
    prefix_len: u8,
    interface: String,
}

impl RaDaemon {
    pub fn new(config: &SlaacConfig) -> anyhow::Result<Self> {
        let prefix: Ipv6Addr = config.prefix.parse()?;

        Ok(Self {
            prefix,
            prefix_len: config.prefix_len,
            interface: config.interface.clone(),
        })
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            "SLAAC RA daemon started: prefix {}/{} on {}",
            self.prefix, self.prefix_len, self.interface
        );

        // RA requires raw ICMPv6 sockets (CAP_NET_RAW).
        // In production, this would:
        // 1. Open a raw ICMPv6 socket
        // 2. Join the all-routers multicast group (ff02::2)
        // 3. Listen for Router Solicitation messages
        // 4. Periodically send Router Advertisements with:
        //    - Prefix Information Option (the configured prefix)
        //    - Router Lifetime
        //    - Reachable Time / Retrans Timer
        //    - MTU option

        let mut shutdown = shutdown;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Would send RA here with raw sockets
                    tracing::debug!(
                        "RA: would advertise {}/{} on {}",
                        self.prefix, self.prefix_len, self.interface
                    );
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("SLAAC RA daemon shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}
