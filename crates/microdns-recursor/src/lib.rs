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
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{watch, RwLock, Semaphore};
use tracing::{debug, error, info, warn};

/// Maximum concurrent TCP connections
const MAX_TCP_CONNECTIONS: usize = 1000;

/// Maximum concurrent UDP query tasks
const MAX_UDP_QUERIES: usize = 10_000;

/// Timeout for TCP connection handling
const TCP_TIMEOUT: Duration = Duration::from_secs(30);

pub struct RecursorServer {
    listen_addr: SocketAddr,
    resolver: Arc<Resolver>,
    db: Option<Db>,
    reload_rx: Option<watch::Receiver<()>>,
}

impl RecursorServer {
    pub fn new(config: &DnsRecursorConfig, db: Option<Db>) -> anyhow::Result<Self> {
        let listen_addr: SocketAddr = config.listen.parse()?;

        let cache = Arc::new(DnsCache::new(config.cache_size));
        let forward_table = Arc::new(RwLock::new(ForwardTable::from_config(&config.forward_zones)));

        let resolver = Arc::new(Resolver::new(cache, forward_table, db.clone()));

        Ok(Self {
            listen_addr,
            resolver,
            db,
            reload_rx: None,
        })
    }

    /// Create a recursor that loads forward zones from the database.
    pub fn from_db(listen_addr: SocketAddr, db: Db, cache_size: usize) -> anyhow::Result<Self> {
        let cache = Arc::new(DnsCache::new(cache_size));
        let forwarders = db.list_dns_forwarders().unwrap_or_default();
        let forward_table = Arc::new(RwLock::new(ForwardTable::from_forwarders(&forwarders)));
        info!(
            "recursor loaded {} forward zones from database",
            forwarders.len()
        );

        let resolver = Arc::new(Resolver::new(cache, forward_table, Some(db.clone())));

        Ok(Self {
            listen_addr,
            resolver,
            db: Some(db),
            reload_rx: None,
        })
    }

    /// Set a reload channel — when signaled, forward zones are reloaded from the database.
    pub fn with_reload(mut self, rx: watch::Receiver<()>) -> Self {
        self.reload_rx = Some(rx);
        self
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
        let shutdown_reload = shutdown.clone();
        let mut shutdown_tcp = shutdown;

        let resolver_tcp = self.resolver.clone();

        // TCP accept loop with connection limit
        let tcp_semaphore = Arc::new(Semaphore::new(MAX_TCP_CONNECTIONS));
        let tcp_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = tcp_listener.accept() => {
                        match result {
                            Ok((stream, src)) => {
                                let permit = match tcp_semaphore.clone().try_acquire_owned() {
                                    Ok(p) => p,
                                    Err(_) => {
                                        warn!("recursor TCP connection limit reached, rejecting {src}");
                                        continue;
                                    }
                                };
                                debug!("recursor TCP connection from {src}");
                                let resolver = resolver_tcp.clone();
                                tokio::spawn(async move {
                                    let result = tokio::time::timeout(
                                        TCP_TIMEOUT,
                                        handle_tcp_query(stream, &resolver),
                                    ).await;
                                    match result {
                                        Ok(Err(e)) => warn!("recursor TCP handler error from {src}: {e}"),
                                        Err(_) => warn!("recursor TCP handler timeout from {src}"),
                                        _ => {}
                                    }
                                    drop(permit);
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

        // Spawn forward-zone reload task if db + reload channel available
        let reload_handle = if let (Some(db), Some(mut reload_rx)) = (self.db.clone(), self.reload_rx) {
            let resolver = self.resolver.clone();
            let mut shutdown_reload = shutdown_reload;
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = reload_rx.changed() => {
                            match db.list_dns_forwarders() {
                                Ok(forwarders) => {
                                    let table = ForwardTable::from_forwarders(&forwarders);
                                    resolver.update_forward_table(table).await;
                                    info!("recursor reloaded {} forward zones from database", forwarders.len());
                                }
                                Err(e) => {
                                    error!("failed to reload forward zones: {e}");
                                }
                            }
                        }
                        _ = shutdown_reload.changed() => {
                            if *shutdown_reload.borrow() { break; }
                        }
                    }
                }
            }))
        } else {
            None
        };

        // UDP recv loop with concurrency limit
        let udp_semaphore = Arc::new(Semaphore::new(MAX_UDP_QUERIES));
        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    let (len, src) = result?;
                    let data = buf[..len].to_vec();
                    let resolver = self.resolver.clone();
                    let socket = socket.clone();

                    let permit = match udp_semaphore.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            warn!("UDP query limit reached, dropping query from {src}");
                            continue;
                        }
                    };

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
                        drop(permit);
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
        if let Some(h) = reload_handle {
            h.abort();
        }
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
