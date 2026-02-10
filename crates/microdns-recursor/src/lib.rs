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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

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
        let tcp_listener = TcpListener::bind(self.listen_addr).await?;
        info!(
            "recursive DNS server listening on {} (UDP+TCP)",
            self.listen_addr
        );

        let mut buf = vec![0u8; 4096];
        let mut shutdown_udp = shutdown.clone();
        let mut shutdown_tcp = shutdown;

        let resolver_tcp = self.resolver.clone();

        // TCP accept loop
        let tcp_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = tcp_listener.accept() => {
                        match result {
                            Ok((stream, src)) => {
                                debug!("recursor TCP connection from {src}");
                                let resolver = resolver_tcp.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_tcp_query(stream, &resolver).await {
                                        warn!("recursor TCP handler error from {src}: {e}");
                                    }
                                });
                            }
                            Err(e) => {
                                error!("recursor TCP accept error: {e}");
                            }
                        }
                    }
                    _ = shutdown_tcp.changed() => {
                        if *shutdown_tcp.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // UDP recv loop
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
                _ = shutdown_udp.changed() => {
                    if *shutdown_udp.borrow() {
                        info!("recursive DNS server shutting down");
                        break;
                    }
                }
            }
        }

        tcp_handle.abort();
        Ok(())
    }

    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }
}

async fn handle_tcp_query(
    mut stream: tokio::net::TcpStream,
    resolver: &Resolver,
) -> anyhow::Result<()> {
    // DNS over TCP: 2-byte length prefix, then DNS message
    let msg_len = stream.read_u16().await? as usize;
    if msg_len == 0 || msg_len > 65535 {
        return Ok(());
    }

    let mut buf = vec![0u8; msg_len];
    stream.read_exact(&mut buf).await?;

    let response = resolver.resolve(&buf).await?;
    let len = response.len() as u16;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&response).await?;
    stream.flush().await?;

    Ok(())
}
