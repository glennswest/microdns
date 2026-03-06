mod log_layer;

use anyhow::Result;
use clap::Parser;
use microdns_api::ApiServer;
use microdns_auth::server::AuthServer;
use microdns_core::config::Config;
use microdns_core::db::Db;
use microdns_core::log_buffer::LogBuffer;
use microdns_core::types::{DhcpDbReservation, DhcpPool, DnsForwarder, InstanceMode};
use microdns_federation::heartbeat::HeartbeatTracker;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "microdns", about = "MicroDNS - Authoritative DNS, Recursive DNS, Load Balancer, and DHCP")]
struct Cli {
    /// Path to configuration file (optional — if omitted, runs with database-only config)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// DNS listen address (e.g. "0.0.0.0:53")
    #[arg(long)]
    listen_dns: Option<String>,

    /// REST API listen address (e.g. "0.0.0.0:8080")
    #[arg(long)]
    listen_api: Option<String>,

    /// Data directory for database and state
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// NATS URL for messaging
    #[arg(long)]
    nats_url: Option<String>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Instance mode (standalone, leaf, coordinator, gateway)
    #[arg(long)]
    mode: Option<String>,

    /// DHCP network interface
    #[arg(long)]
    dhcp_interface: Option<String>,

    /// Instance ID (unique per microdns instance)
    #[arg(long)]
    instance_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut config = if let Some(ref config_path) = cli.config {
        Config::from_file(config_path)?
    } else {
        Config::default()
    };

    // CLI flag overrides
    if let Some(ref id) = cli.instance_id {
        config.instance.id = id.clone();
    }
    if let Some(ref mode) = cli.mode {
        config.instance.mode = match mode.as_str() {
            "standalone" => InstanceMode::Standalone,
            "leaf" => InstanceMode::Leaf,
            "coordinator" => InstanceMode::Coordinator,
            "gateway" => InstanceMode::Gateway,
            _ => {
                anyhow::bail!("invalid mode: {mode} (expected standalone, leaf, coordinator, gateway)");
            }
        };
    }
    if let Some(ref level) = Some(&cli.log_level) {
        config.logging.level = level.to_string();
    }
    if let Some(ref listen_dns) = cli.listen_dns {
        // Override auth + recursor listen addresses
        if let Some(ref mut auth) = config.dns.auth {
            auth.listen = listen_dns.clone();
            auth.enabled = true;
        }
        if let Some(ref mut recursor) = config.dns.recursor {
            recursor.listen = listen_dns.clone();
            recursor.enabled = true;
        }
    }
    if let Some(ref listen_api) = cli.listen_api {
        if let Some(ref mut rest) = config.api.rest {
            rest.listen = listen_api.clone();
            rest.enabled = true;
        }
    }
    if let Some(ref data_dir) = cli.data_dir {
        config.database.path = data_dir.join("microdns.db");
    }
    if let Some(ref nats_url) = cli.nats_url {
        if config.messaging.is_none() {
            config.messaging = Some(microdns_core::config::MessagingConfig {
                backend: "nats".to_string(),
                topic_prefix: "microdns".to_string(),
                brokers: vec![],
                url: Some(nats_url.clone()),
            });
        } else if let Some(ref mut msg) = config.messaging {
            msg.url = Some(nats_url.clone());
            msg.backend = "nats".to_string();
        }
    }

    // Initialize logging
    let log_buffer = init_logging(&config.logging);

    info!(
        instance_id = %config.instance.id,
        mode = ?config.instance.mode,
        "starting microdns"
    );

    // Open database
    let db = Db::open(&config.database.path)?;
    info!(path = %config.database.path.display(), "database opened");

    // TOML → database migration: if config file was provided and DB tables are empty,
    // auto-import pools, reservations, and forwarders to the database (one-time).
    if cli.config.is_some() {
        migrate_toml_to_db(&config, &db);
    }

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut tasks = Vec::new();

    // Initialize message bus
    let (backend, topic_prefix, brokers, nats_url) = if let Some(ref msg_config) = config.messaging
    {
        (
            msg_config.backend.as_str().to_string(),
            msg_config.topic_prefix.clone(),
            msg_config.brokers.clone(),
            msg_config.url.clone(),
        )
    } else {
        ("noop".to_string(), "microdns".to_string(), vec![], None)
    };

    let message_bus: Arc<dyn microdns_msg::MessageBus> = match microdns_msg::create_message_bus(
        &backend,
        &config.instance.id,
        &topic_prefix,
        &brokers,
        nats_url.as_deref(),
    )
    .await
    {
        Ok(bus) => {
            info!(backend = %backend, "message bus initialized");
            Arc::from(bus)
        }
        Err(e) => {
            tracing::warn!(backend = %backend, error = %e, "message bus connection failed, falling back to noop — DNS/DHCP will work but events won't publish");
            Arc::from(
                microdns_msg::create_message_bus("noop", &config.instance.id, &topic_prefix, &[], None)
                    .await
                    .expect("noop message bus should never fail"),
            )
        }
    };

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
        InstanceMode::Gateway => {
            info!("gateway mode (RouterOS/rose), federation disabled");
        }
    }

    // Start replication agent if configured
    if let Some(ref repl_config) = config.replication {
        if repl_config.enabled {
            let agent = microdns_federation::replication::ReplicationAgent::new(
                &config.instance.id,
                db.clone(),
                config.instance.peers.clone(),
                repl_config.clone(),
            );
            let rx = shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = agent.run(rx).await {
                    error!("replication agent error: {e}");
                }
            }));
            info!("replication agent started");
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
    let mut recursor_cache = None;
    if let Some(ref recursor_config) = config.dns.recursor {
        if recursor_config.enabled {
            let server = microdns_recursor::RecursorServer::new(
                recursor_config,
                Some(db.clone()),
            )?;
            // Share the recursor cache with the REST API so mutations can invalidate it
            recursor_cache = Some(server.resolver().cache_arc());
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
    // In gateway mode (RouterOS/rose), default DHCP to relay-only unless
    // the config explicitly sets a different mode.
    if config.instance.mode == InstanceMode::Gateway {
        if let Some(ref mut dhcp_config) = config.dhcp {
            if let Some(ref mut v4) = dhcp_config.v4 {
                if v4.mode == microdns_core::config::DhcpMode::default() {
                    v4.mode = microdns_core::config::DhcpMode::Gateway;
                    info!("gateway instance mode: DHCP defaulting to relay-only");
                }
            }
        }
    }
    let mut dhcp_lease_event_tx: Option<tokio::sync::broadcast::Sender<String>> = None;
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
                let (lease_event_tx, _) = tokio::sync::broadcast::channel::<String>(256);
                let mut server =
                    microdns_dhcp::v4::server::Dhcpv4Server::new(v4_config, db.clone())?;
                if let Some(ref registrar) = dns_registrar {
                    server = server.with_dns_registrar(registrar.clone());
                }
                server = server.with_message_bus(message_bus.clone(), &config.instance.id);
                server = server.with_lease_event_tx(lease_event_tx.clone());

                // Store the lease_event_tx for bridging to dashboard API later
                dhcp_lease_event_tx = Some(lease_event_tx);

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
        let purge_bus = message_bus.clone();
        let purge_instance_id = config.instance.id.clone();
        tasks.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            let mut rx = rx;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let mgr = microdns_dhcp::lease::LeaseManager::new(db_cleanup.clone());
                        // Remove orphaned leases (old code leftovers)
                        match mgr.purge_orphaned_leases() {
                            Ok(0) => {}
                            Ok(n) => info!("purged {n} orphaned leases"),
                            Err(e) => error!("orphan lease cleanup error: {e}"),
                        }
                        // Keep expired leases for 4x lease time (4 * 600s = 2400s = 40 min)
                        match mgr.purge_expired_leases_with_details(chrono::Duration::seconds(2400)) {
                            Ok(purged) if purged.is_empty() => {}
                            Ok(purged) => {
                                info!("purged {} expired leases", purged.len());
                                for (ip, mac) in &purged {
                                    let event = microdns_msg::events::Event::LeaseReleased {
                                        instance_id: purge_instance_id.clone(),
                                        ip_addr: ip.clone(),
                                        mac_addr: mac.clone(),
                                        timestamp: chrono::Utc::now(),
                                    };
                                    if let Err(e) = purge_bus.publish(&event).await {
                                        error!("failed to publish purge LeaseReleased: {e}");
                                    }
                                }
                            }
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

            // Build DHCP status summary for API
            let dhcp_status = if let Some(ref dhcp_cfg) = config.dhcp {
                if let Some(ref v4) = dhcp_cfg.v4 {
                    microdns_api::DhcpStatusConfig {
                        enabled: v4.enabled,
                        interface: v4.interface.clone(),
                        pools: v4
                            .pools
                            .iter()
                            .map(|p| microdns_api::rest::dhcp::DhcpPoolSummary {
                                range_start: p.range_start.clone(),
                                range_end: p.range_end.clone(),
                                subnet: p.subnet.clone(),
                                gateway: p.gateway.clone(),
                                domain: p.domain.clone(),
                                lease_time_secs: p.lease_time_secs,
                                pxe_enabled: p.next_server.is_some(),
                            })
                            .collect(),
                        reservation_count: v4.reservations.len(),
                    }
                } else {
                    microdns_api::DhcpStatusConfig::default()
                }
            } else {
                microdns_api::DhcpStatusConfig::default()
            };

            let dashboard_addr: SocketAddr = rest_config.dashboard_listen.parse()?;

            let mut api = ApiServer::new(addr, db.clone(), rest_config.api_key.clone())
                .with_instance_id(&config.instance.id)
                .with_ipam_pools(ipam_pools)
                .with_peers(config.instance.peers.clone())
                .with_dhcp_status(dhcp_status)
                .with_log_buffer(log_buffer.clone())
                .with_dashboard_addr(dashboard_addr);

            if let Some(cache) = recursor_cache.clone() {
                api = api.with_recursor_cache(cache);
            }

            if config.instance.mode == InstanceMode::Coordinator {
                api = api.with_heartbeat_tracker(heartbeat_tracker.clone());
            }

            // Bridge DHCP lease events to dashboard
            if let Some(lease_rx_sender) = dhcp_lease_event_tx.take() {
                let dashboard_tx = api.event_tx();
                let mut lease_rx = lease_rx_sender.subscribe();
                let bridge_shutdown = shutdown_rx.clone();
                tasks.push(tokio::spawn(async move {
                    let mut rx = bridge_shutdown;
                    loop {
                        tokio::select! {
                            result = lease_rx.recv() => {
                                match result {
                                    Ok(json) => {
                                        // Parse the JSON to extract action, ip, mac
                                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) {
                                            let action = parsed["action"].as_str().unwrap_or("unknown").to_string();
                                            let ip = parsed["ip"].as_str().unwrap_or("").to_string();
                                            let mac = parsed["mac"].as_str().unwrap_or("").to_string();
                                            let _ = dashboard_tx.send(microdns_api::DashboardEvent::LeaseChanged {
                                                action, ip, mac,
                                            });
                                        }
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                        warn!("lease event bridge lagged by {n} messages");
                                    }
                                    Err(_) => break,
                                }
                            }
                            _ = rx.changed() => {
                                if *rx.borrow() { break; }
                            }
                        }
                    }
                }));
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

    // Wait for shutdown signal (SIGINT or SIGTERM)
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;
        tokio::select! {
            _ = sigterm.recv() => info!("SIGTERM received, stopping services..."),
            _ = sigint.recv() => info!("SIGINT received, stopping services..."),
        }
    }
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

/// Migrate DHCP pools, reservations, and DNS forwarders from TOML config into
/// the database. Only runs once — if the DB tables already have data, this is a no-op.
fn migrate_toml_to_db(config: &Config, db: &Db) {
    let empty = match db.dhcp_tables_empty() {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to check if DHCP tables are empty: {e}");
            return;
        }
    };
    if !empty {
        return;
    }

    info!("TOML → database migration: importing DHCP pools, reservations, and forwarders");

    // Import pools
    if let Some(ref dhcp) = config.dhcp {
        if let Some(ref v4) = dhcp.v4 {
            for pool_cfg in &v4.pools {
                let pool = DhcpPool {
                    id: uuid::Uuid::new_v4(),
                    name: format!("{} ({})", pool_cfg.domain, pool_cfg.subnet),
                    range_start: pool_cfg.range_start.clone(),
                    range_end: pool_cfg.range_end.clone(),
                    subnet: pool_cfg.subnet.clone(),
                    gateway: pool_cfg.gateway.clone(),
                    dns_servers: pool_cfg.dns.clone(),
                    domain: pool_cfg.domain.clone(),
                    lease_time_secs: pool_cfg.lease_time_secs,
                    next_server: pool_cfg.next_server.clone(),
                    boot_file: pool_cfg.boot_file.clone(),
                    boot_file_efi: pool_cfg.boot_file_efi.clone(),
                    ipxe_boot_url: pool_cfg.ipxe_boot_url.clone(),
                    root_path: pool_cfg.root_path.clone(),
                    ntp_servers: None,
                    domain_search: None,
                    mtu: None,
                    static_routes: None,
                    log_server: None,
                    time_offset: None,
                    wpad_url: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };
                match db.create_dhcp_pool(&pool) {
                    Ok(_) => info!("migrated pool: {} ({})", pool.name, pool.subnet),
                    Err(e) => warn!("failed to migrate pool {}: {e}", pool.name),
                }
            }

            // Import reservations
            for res in &v4.reservations {
                let reservation = DhcpDbReservation {
                    mac: res.mac.clone(),
                    ip: res.ip.clone(),
                    hostname: res.hostname.clone(),
                    gateway: None,
                    dns_servers: None,
                    domain: None,
                    next_server: None,
                    boot_file: None,
                    boot_file_efi: None,
                    ipxe_boot_url: None,
                    root_path: None,
                    ntp_servers: None,
                    domain_search: None,
                    mtu: None,
                    static_routes: None,
                    log_server: None,
                    time_offset: None,
                    wpad_url: None,
                    lease_time_secs: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };
                match db.create_dhcp_reservation(&reservation) {
                    Ok(_) => info!(
                        "migrated reservation: {} → {}",
                        res.mac,
                        res.ip
                    ),
                    Err(e) => warn!("failed to migrate reservation {}: {e}", res.mac),
                }
            }
        }
    }

    // Import forward zones
    if let Some(ref recursor) = config.dns.recursor {
        for (zone, servers) in &recursor.forward_zones {
            let fwd = DnsForwarder {
                zone: zone.trim_end_matches('.').to_lowercase(),
                servers: servers.clone(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            match db.create_dns_forwarder(&fwd) {
                Ok(_) => info!("migrated forward zone: {} → {:?}", fwd.zone, fwd.servers),
                Err(e) => warn!("failed to migrate forward zone {}: {e}", zone),
            }
        }
    }

    info!("TOML → database migration complete");
}

fn init_logging(config: &microdns_core::config::LoggingConfig) -> Arc<LogBuffer> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    let log_buffer = Arc::new(LogBuffer::new(1000));
    let buffer_layer = log_layer::LogBufferLayer::new(log_buffer.clone());

    match config.format.as_str() {
        "json" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().json())
                .with(buffer_layer)
                .init();
        }
        _ => {
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer())
                .with(buffer_layer)
                .init();
        }
    }

    log_buffer
}
