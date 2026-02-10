use anyhow::Result;
use clap::Parser;
use microdns_api::ApiServer;
use microdns_auth::server::AuthServer;
use microdns_core::config::Config;
use microdns_core::db::Db;
use microdns_core::types::InstanceMode;
use microdns_federation::heartbeat::HeartbeatTracker;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "microdns", about = "MicroDNS - Authoritative DNS, Recursive DNS, Load Balancer, and DHCP")]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "/etc/microdns/microdns.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = Config::from_file(&cli.config)?;

    // Initialize logging
    init_logging(&config.logging);

    info!(
        instance_id = %config.instance.id,
        mode = ?config.instance.mode,
        "starting microdns"
    );

    // Open database
    let db = Db::open(&config.database.path)?;
    info!(path = %config.database.path.display(), "database opened");

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut tasks = Vec::new();

    // Initialize message bus
    let (backend, topic_prefix, brokers) = if let Some(ref msg_config) = config.messaging {
        (
            msg_config.backend.as_str().to_string(),
            msg_config.topic_prefix.clone(),
            msg_config.brokers.clone(),
        )
    } else {
        ("noop".to_string(), "microdns".to_string(), vec![])
    };

    let message_bus: Arc<dyn microdns_msg::MessageBus> = Arc::from(
        microdns_msg::create_message_bus(
            &backend,
            &config.instance.id,
            &topic_prefix,
            &brokers,
        )?,
    );
    info!(backend = %backend, "message bus initialized");

    // Heartbeat tracker (used by coordinator mode and API)
    let heartbeat_tracker = Arc::new(HeartbeatTracker::new(
        config
            .coordinator
            .as_ref()
            .map(|c| c.heartbeat_interval_secs * 3)
            .unwrap_or(90),
    ));

    // Start federation agents based on mode
    match config.instance.mode {
        InstanceMode::Leaf => {
            let leaf = Arc::new(microdns_federation::leaf::LeafAgent::new(
                &config.instance.id,
                message_bus.clone(),
                config
                    .coordinator
                    .as_ref()
                    .map(|c| c.heartbeat_interval_secs)
                    .unwrap_or(10),
            ));

            let rx = shutdown_rx.clone();
            let active_leases_fn: Arc<dyn Fn() -> u64 + Send + Sync> =
                Arc::new(|| 0); // TODO: wire to lease manager
            let zones_fn: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(|| 0);
            tasks.push(tokio::spawn(async move {
                if let Err(e) = leaf.run(active_leases_fn, zones_fn, rx).await {
                    error!("leaf agent error: {e}");
                }
            }));

            // Start config sync agent
            let sync_agent = microdns_federation::sync::ConfigSyncAgent::new(
                &config.instance.id,
                message_bus.clone(),
                db.clone(),
                &topic_prefix,
            );
            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = sync_agent.run(rx).await {
                    error!("config sync agent error: {e}");
                }
            }));

            info!("leaf federation agents started");
        }
        InstanceMode::Coordinator => {
            let coordinator = microdns_federation::coordinator::CoordinatorAgent::new(
                &config.instance.id,
                message_bus.clone(),
                heartbeat_tracker.clone(),
                &topic_prefix,
            );
            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = coordinator.run(rx).await {
                    error!("coordinator agent error: {e}");
                }
            }));

            info!("coordinator federation agent started");
        }
        InstanceMode::Standalone => {
            info!("standalone mode, federation disabled");
        }
    }

    // Start auth DNS server
    if let Some(ref auth_config) = config.dns.auth {
        if auth_config.enabled {
            let addr: SocketAddr = auth_config.listen.parse()?;
            let server = AuthServer::new(addr, db.clone());
            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = server.run(rx).await {
                    error!("auth DNS server error: {e}");
                }
            }));
        }
    }

    // Start recursive DNS server
    if let Some(ref recursor_config) = config.dns.recursor {
        if recursor_config.enabled {
            let server = microdns_recursor::RecursorServer::new(
                recursor_config,
                Some(db.clone()),
            )?;
            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = server.run(rx).await {
                    error!("recursive DNS server error: {e}");
                }
            }));
        }
    }

    // Start load balancer health monitor
    if let Some(ref lb_config) = config.dns.loadbalancer {
        if lb_config.enabled {
            use microdns_core::types::ProbeType;
            let default_probe = match lb_config.default_probe.as_str() {
                "http" => ProbeType::Http,
                "https" => ProbeType::Https,
                "tcp" => ProbeType::Tcp,
                _ => ProbeType::Ping,
            };
            let monitor = microdns_lb::HealthMonitor::new(
                db.clone(),
                std::time::Duration::from_secs(lb_config.check_interval_secs),
                default_probe,
            );
            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = monitor.run(rx).await {
                    error!("health monitor error: {e}");
                }
            }));
        }
    }

    // Start DHCP servers
    if let Some(ref dhcp_config) = config.dhcp {
        // Create DNS registrar if configured
        let dns_registrar = dhcp_config
            .dns_registration
            .as_ref()
            .filter(|r| r.enabled)
            .map(|r| {
                Arc::new(microdns_dhcp::dns_register::DnsRegistrar::new(
                    db.clone(),
                    &r.forward_zone,
                    &r.reverse_zone_v4,
                    &r.reverse_zone_v6,
                    r.default_ttl,
                ))
            });

        // DHCPv4
        if let Some(ref v4_config) = dhcp_config.v4 {
            if v4_config.enabled {
                let mut server =
                    microdns_dhcp::v4::server::Dhcpv4Server::new(v4_config, db.clone())?;
                if let Some(ref registrar) = dns_registrar {
                    server = server.with_dns_registrar(registrar.clone());
                }
                let rx = shutdown_rx.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = server.run(rx).await {
                        error!("DHCPv4 server error: {e}");
                    }
                }));
            }
        }

        // DHCPv6
        if let Some(ref v6_config) = dhcp_config.v6 {
            if v6_config.enabled {
                let server = microdns_dhcp::v6::server::Dhcpv6Server::new(v6_config, db.clone())?;
                let rx = shutdown_rx.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = server.run(rx).await {
                        error!("DHCPv6 server error: {e}");
                    }
                }));
            }
        }

        // SLAAC Router Advertisements
        if let Some(ref slaac_config) = dhcp_config.slaac {
            if slaac_config.enabled {
                let daemon = microdns_dhcp::slaac::ra::RaDaemon::new(slaac_config)?;
                let rx = shutdown_rx.clone();
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = daemon.run(rx).await {
                        error!("SLAAC RA daemon error: {e}");
                    }
                }));
            }
        }
    }

    // Start lease expiry cleanup task
    {
        let db_cleanup = db.clone();
        let rx = shutdown_rx.clone();
        tasks.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            let mut rx = rx;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let mgr = microdns_dhcp::lease::LeaseManager::new(db_cleanup.clone());
                        match mgr.purge_expired_leases(chrono::Duration::hours(24)) {
                            Ok(0) => {}
                            Ok(n) => info!("purged {n} expired leases"),
                            Err(e) => error!("lease cleanup error: {e}"),
                        }
                    }
                    _ = rx.changed() => {
                        if *rx.borrow() { break; }
                    }
                }
            }
        }));
    }

    // Start REST API
    if let Some(ref rest_config) = config.api.rest {
        if rest_config.enabled {
            let addr: SocketAddr = rest_config.listen.parse()?;
            let ipam_pools = config
                .ipam
                .as_ref()
                .filter(|c| c.enabled)
                .map(|c| c.pools.clone())
                .unwrap_or_default();

            let mut api = ApiServer::new(addr, db.clone(), rest_config.api_key.clone())
                .with_instance_id(&config.instance.id)
                .with_ipam_pools(ipam_pools)
                .with_peers(config.instance.peers.clone());

            if config.instance.mode == InstanceMode::Coordinator {
                api = api.with_heartbeat_tracker(heartbeat_tracker.clone());
            }

            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = api.run(rx).await {
                    error!("REST API error: {e}");
                }
            }));
        }
    }

    // Start gRPC server
    if let Some(ref grpc_config) = config.api.grpc {
        if grpc_config.enabled {
            let addr: SocketAddr = grpc_config.listen.parse()?;
            let mut grpc = microdns_api::GrpcServer::new(addr, db.clone())
                .with_instance_id(&config.instance.id);

            if config.instance.mode == InstanceMode::Coordinator {
                grpc = grpc.with_heartbeat_tracker(heartbeat_tracker.clone());
            }

            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = grpc.run(rx).await {
                    error!("gRPC server error: {e}");
                }
            }));
        }
    }

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("shutdown signal received, stopping services...");
    let _ = shutdown_tx.send(true);

    // Shut down message bus
    if let Err(e) = message_bus.shutdown().await {
        error!("message bus shutdown error: {e}");
    }

    // Wait for all tasks to finish
    for task in tasks {
        let _ = task.await;
    }

    info!("microdns stopped");
    Ok(())
}

fn init_logging(config: &microdns_core::config::LoggingConfig) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    match config.format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .init();
        }
    }
}
