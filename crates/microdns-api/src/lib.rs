pub mod dashboard;
pub mod grpc;
pub mod rest;
pub mod security;
pub mod ws;

use axum::extract::DefaultBodyLimit;
use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use microdns_core::config::{IpamPool, PeerConfig};
use microdns_core::db::Db;
use microdns_core::log_buffer::LogBuffer;
use microdns_core::query_tracker::QueryTracker;
use microdns_core::types::ProbeType;
use microdns_federation::heartbeat::HeartbeatTracker;
use microdns_lb::halfopen::HalfOpenManager;
use microdns_lb::monitor::StateChangeLog;
use microdns_lb::{HealthState, StateChange};
use microdns_msg::MessageBus;
use microdns_recursor::cache::DnsCache;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, watch, Mutex};
use tracing::info;

pub use rest::dhcp::DhcpStatusConfig;

/// Maximum request body size (1 MB)
const MAX_BODY_SIZE: usize = 1024 * 1024;

/// Maximum concurrent WebSocket connections
const MAX_WS_CONNECTIONS: usize = 100;

pub struct ApiServer {
    listen_addr: SocketAddr,
    dashboard_addr: Option<SocketAddr>,
    db: Db,
    api_key: Option<String>,
    instance_id: String,
    heartbeat_tracker: Option<Arc<HeartbeatTracker>>,
    ipam_pools: Vec<IpamPool>,
    peers: Vec<PeerConfig>,
    dhcp_status: DhcpStatusConfig,
    log_buffer: Option<Arc<LogBuffer>>,
    message_bus: Option<Arc<dyn MessageBus>>,
    event_tx: broadcast::Sender<DashboardEvent>,
    recursor_cache: Option<Arc<DnsCache>>,
    lb: Option<LbHandles>,
    query_tracker: Option<Arc<QueryTracker>>,
}

/// Hooks the API needs to surface load-balancer state and emit/receive
/// state-change events. Wired up by the binary when the LB monitor is
/// running.
#[derive(Clone)]
pub struct LbHandles {
    pub state: Arc<Mutex<HealthState>>,
    pub events: broadcast::Sender<StateChange>,
    pub check_interval_secs: u64,
    pub default_probe: ProbeType,
    pub halfopen: Option<Arc<HalfOpenManager>>,
    pub log: Option<Arc<Mutex<StateChangeLog>>>,
}

/// Dashboard event for real-time UI updates via broadcast channel
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum DashboardEvent {
    DhcpPoolChanged { action: String, pool_id: String, pool_name: String },
    DhcpReservationChanged { action: String, mac: String, ip: String },
    DnsForwarderChanged { action: String, zone: String },
    LeaseChanged { action: String, ip: String, mac: String },
    ZoneChanged { action: String, zone_id: String, zone_name: String },
    RecordChanged { action: String, zone_id: String, record_name: String },
    /// Load-balancer state-change. Emitted whenever a record's HealthStatus
    /// flips, or whenever failsafe activates on a record.
    LbStateChange {
        record_id: String,
        zone_id: String,
        zone_name: String,
        name: String,
        ip: String,
        record_type: String,
        status: String,
        failsafe: bool,
        probe_type: String,
        detail: String,
        at: String,
    },
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub api_key: Option<Arc<String>>,
    pub instance_id: String,
    pub heartbeat_tracker: Option<Arc<HeartbeatTracker>>,
    pub ipam_pools: Vec<IpamPool>,
    pub peers: Vec<PeerConfig>,
    pub ws_connections: Arc<AtomicUsize>,
    pub dhcp_status: DhcpStatusConfig,
    pub log_buffer: Option<Arc<LogBuffer>>,
    pub message_bus: Option<Arc<dyn MessageBus>>,
    pub event_tx: broadcast::Sender<DashboardEvent>,
    pub recursor_cache: Option<Arc<DnsCache>>,
    pub started_at: Instant,
    pub lb: Option<LbHandles>,
    pub query_tracker: Option<Arc<QueryTracker>>,
}

impl ApiServer {
    pub fn new(listen_addr: SocketAddr, db: Db, api_key: Option<String>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            listen_addr,
            dashboard_addr: None,
            db,
            api_key,
            instance_id: String::new(),
            heartbeat_tracker: None,
            ipam_pools: Vec::new(),
            peers: Vec::new(),
            dhcp_status: DhcpStatusConfig::default(),
            log_buffer: None,
            message_bus: None,
            event_tx,
            recursor_cache: None,
            lb: None,
            query_tracker: None,
        }
    }

    pub fn with_lb(mut self, handles: LbHandles) -> Self {
        self.lb = Some(handles);
        self
    }

    pub fn with_query_tracker(mut self, tracker: Arc<QueryTracker>) -> Self {
        self.query_tracker = Some(tracker);
        self
    }

    pub fn with_recursor_cache(mut self, cache: Arc<DnsCache>) -> Self {
        self.recursor_cache = Some(cache);
        self
    }

    pub fn with_message_bus(mut self, bus: Arc<dyn MessageBus>) -> Self {
        self.message_bus = Some(bus);
        self
    }

    pub fn event_rx(&self) -> broadcast::Receiver<DashboardEvent> {
        self.event_tx.subscribe()
    }

    pub fn event_tx(&self) -> broadcast::Sender<DashboardEvent> {
        self.event_tx.clone()
    }

    pub fn with_dashboard_addr(mut self, addr: SocketAddr) -> Self {
        self.dashboard_addr = Some(addr);
        self
    }

    pub fn with_instance_id(mut self, id: &str) -> Self {
        self.instance_id = id.to_string();
        self
    }

    pub fn with_heartbeat_tracker(mut self, tracker: Arc<HeartbeatTracker>) -> Self {
        self.heartbeat_tracker = Some(tracker);
        self
    }

    pub fn with_ipam_pools(mut self, pools: Vec<IpamPool>) -> Self {
        self.ipam_pools = pools;
        self
    }

    pub fn with_peers(mut self, peers: Vec<PeerConfig>) -> Self {
        self.peers = peers;
        self
    }

    pub fn with_dhcp_status(mut self, status: DhcpStatusConfig) -> Self {
        self.dhcp_status = status;
        self
    }

    pub fn with_log_buffer(mut self, buffer: Arc<LogBuffer>) -> Self {
        self.log_buffer = Some(buffer);
        self
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let state = AppState {
            db: self.db,
            api_key: self.api_key.map(Arc::new),
            instance_id: self.instance_id,
            heartbeat_tracker: self.heartbeat_tracker,
            ipam_pools: self.ipam_pools,
            peers: self.peers,
            ws_connections: Arc::new(AtomicUsize::new(0)),
            dhcp_status: self.dhcp_status,
            log_buffer: self.log_buffer,
            message_bus: self.message_bus,
            event_tx: self.event_tx,
            recursor_cache: self.recursor_cache,
            started_at: Instant::now(),
            lb: self.lb,
            query_tracker: self.query_tracker,
        };

        // Bridge LB state-changes onto the dashboard broadcast so the
        // existing WS multiplexer carries them too.
        if let Some(lb) = state.lb.clone() {
            let dashboard_tx = state.event_tx.clone();
            let mut lb_rx = lb.events.subscribe();
            let mut bridge_shutdown = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        result = lb_rx.recv() => {
                            match result {
                                Ok(change) => {
                                    let _ = dashboard_tx.send(DashboardEvent::LbStateChange {
                                        record_id: change.record_id.to_string(),
                                        zone_id: change.zone_id.to_string(),
                                        zone_name: change.zone_name,
                                        name: change.name,
                                        ip: change.ip,
                                        record_type: change.record_type,
                                        status: change.status.to_string(),
                                        failsafe: change.failsafe,
                                        probe_type: change.probe_type.to_string(),
                                        detail: change.detail,
                                        at: change.at.to_rfc3339(),
                                    });
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(_) => break,
                            }
                        }
                        _ = bridge_shutdown.changed() => {
                            if *bridge_shutdown.borrow() { break; }
                        }
                    }
                }
            });
        }

        // API router: /api/v1 routes with body limit + api_key auth + CORS
        let api_app = Router::new()
            .nest("/api/v1", rest::router())
            .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security::api_key_auth,
            ))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            )
            .with_state(state.clone());

        let api_listener = tokio::net::TcpListener::bind(self.listen_addr).await?;
        info!("REST API listening on {}", self.listen_addr);

        let mut api_shutdown = shutdown.clone();
        let api_task = tokio::spawn(async move {
            axum::serve(api_listener, api_app)
                .with_graceful_shutdown(async move {
                    let _ = api_shutdown.changed().await;
                })
                .await
        });

        // Dashboard router: /dashboard + /ws (no api_key auth)
        let dashboard_task = if let Some(dashboard_addr) = self.dashboard_addr {
            let dashboard_app = Router::new()
                .route("/", get(|| async { Redirect::permanent("/dashboard") }))
                .route("/dashboard", get(dashboard::dashboard_page))
                .route("/ws", get(ws::ws_handler))
                .with_state(state);

            let dashboard_listener = tokio::net::TcpListener::bind(dashboard_addr).await?;
            info!("Dashboard listening on {}", dashboard_addr);

            let mut dash_shutdown = shutdown;
            Some(tokio::spawn(async move {
                axum::serve(dashboard_listener, dashboard_app)
                    .with_graceful_shutdown(async move {
                        let _ = dash_shutdown.changed().await;
                    })
                    .await
            }))
        } else {
            None
        };

        // Wait for both to complete, but don't block shutdown forever.
        // Graceful shutdown waits for in-flight connections (WebSocket, SSE)
        // which may never close. Give them 5s then abort.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            if let Err(e) = api_task.await {
                tracing::error!("API server task error: {e}");
            }
            if let Some(task) = dashboard_task {
                if let Err(e) = task.await {
                    tracing::error!("Dashboard task error: {e}");
                }
            }
        })
        .await;

        if result.is_err() {
            info!("API shutdown timed out after 5s, forcing close");
        }

        Ok(())
    }
}

/// Standalone gRPC server that can be started alongside the REST API.
pub struct GrpcServer {
    listen_addr: SocketAddr,
    db: Db,
    instance_id: String,
    heartbeat_tracker: Option<Arc<HeartbeatTracker>>,
}

impl GrpcServer {
    pub fn new(listen_addr: SocketAddr, db: Db) -> Self {
        Self {
            listen_addr,
            db,
            instance_id: String::new(),
            heartbeat_tracker: None,
        }
    }

    pub fn with_instance_id(mut self, id: &str) -> Self {
        self.instance_id = id.to_string();
        self
    }

    pub fn with_heartbeat_tracker(mut self, tracker: Arc<HeartbeatTracker>) -> Self {
        self.heartbeat_tracker = Some(tracker);
        self
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        use grpc::proto::{
            cluster_service_server::ClusterServiceServer,
            health_service_server::HealthServiceServer,
            lease_service_server::LeaseServiceServer,
            record_service_server::RecordServiceServer,
            zone_service_server::ZoneServiceServer,
        };

        let svc = grpc::service::MicroDnsGrpcService::new(
            self.db,
            &self.instance_id,
            self.heartbeat_tracker,
        );

        // tonic requires separate service instances since they get moved
        // We use Arc to share the underlying state
        let svc = Arc::new(svc);

        info!("gRPC server listening on {}", self.listen_addr);

        let mut shutdown = shutdown;
        tonic::transport::Server::builder()
            .add_service(ZoneServiceServer::from_arc(svc.clone()).max_decoding_message_size(1024 * 1024))
            .add_service(RecordServiceServer::from_arc(svc.clone()).max_decoding_message_size(1024 * 1024))
            .add_service(LeaseServiceServer::from_arc(svc.clone()).max_decoding_message_size(1024 * 1024))
            .add_service(ClusterServiceServer::from_arc(svc.clone()).max_decoding_message_size(1024 * 1024))
            .add_service(HealthServiceServer::from_arc(svc).max_decoding_message_size(1024 * 1024))
            .serve_with_shutdown(self.listen_addr, async move {
                let _ = shutdown.changed().await;
            })
            .await?;

        Ok(())
    }
}
