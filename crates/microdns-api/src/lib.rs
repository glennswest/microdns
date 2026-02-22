pub mod dashboard;
pub mod grpc;
pub mod rest;
pub mod security;
pub mod ws;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::Router;
use microdns_core::config::{IpamPool, PeerConfig};
use microdns_core::db::Db;
use microdns_federation::heartbeat::HeartbeatTracker;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::info;

pub use rest::dhcp::DhcpStatusConfig;

/// Maximum request body size (1 MB)
const MAX_BODY_SIZE: usize = 1024 * 1024;

/// Maximum concurrent WebSocket connections
const MAX_WS_CONNECTIONS: usize = 100;

pub struct ApiServer {
    listen_addr: SocketAddr,
    db: Db,
    api_key: Option<String>,
    instance_id: String,
    heartbeat_tracker: Option<Arc<HeartbeatTracker>>,
    ipam_pools: Vec<IpamPool>,
    peers: Vec<PeerConfig>,
    dhcp_status: DhcpStatusConfig,
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
}

impl ApiServer {
    pub fn new(listen_addr: SocketAddr, db: Db, api_key: Option<String>) -> Self {
        Self {
            listen_addr,
            db,
            api_key,
            instance_id: String::new(),
            heartbeat_tracker: None,
            ipam_pools: Vec::new(),
            peers: Vec::new(),
            dhcp_status: DhcpStatusConfig::default(),
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
        };

        let app = Router::new()
            .nest("/api/v1", rest::router())
            .route("/dashboard", get(dashboard::dashboard_page))
            .route("/ws", get(ws::ws_handler))
            .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security::api_key_auth,
            ))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(self.listen_addr).await?;
        info!("REST API listening on {}", self.listen_addr);

        let mut shutdown = shutdown;
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await?;

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
