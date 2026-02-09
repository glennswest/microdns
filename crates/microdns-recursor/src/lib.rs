pub mod cache;
pub mod forward;
pub mod resolver;

use cache::DnsCache;
use forward::ForwardTable;
use microdns_core::config::DnsRecursorConfig;
use microdns_core::db::Db;
use resolver::Resolver;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{error, info, warn};

pub struct RecursorServer {
    listen_addr: SocketAddr,
    resolver: Arc<Resolver>,
}

impl RecursorServer {
    pub fn new(config: &DnsRecursorConfig, db: Option<Db>) -> anyhow::Result<Self> {
        let listen_addr: SocketAddr = config.listen.parse()?;

        let cache = Arc::new(DnsCache::new(config.cache_size));
        let forward_table = Arc::new(ForwardTable::from_config(&config.forward_zones));

        let resolver = Arc::new(Resolver::new(cache, forward_table, db));

        Ok(Self {
            listen_addr,
            resolver,
        })
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let socket = Arc::new(UdpSocket::bind(self.listen_addr).await?);
        info!("recursive DNS server listening on {}", self.listen_addr);

        let mut buf = vec![0u8; 4096];
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    let (len, src) = result?;
                    let data = buf[..len].to_vec();
                    let resolver = self.resolver.clone();
                    let socket = socket.clone();

                    // Spawn a task per query for concurrency
                    tokio::spawn(async move {
                        match resolver.resolve(&data).await {
                            Ok(response) => {
                                if let Err(e) = socket.send_to(&response, src).await {
                                    error!("failed to send response to {src}: {e}");
                                }
                            }
                            Err(e) => {
                                warn!("failed to resolve query from {src}: {e}");
                            }
                        }
                    });
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("recursive DNS server shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }
}
