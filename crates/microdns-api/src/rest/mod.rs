pub mod cluster;
pub mod connectivity;
pub mod dhcp;
pub mod dhcp_config;
pub mod dhcp_pools;
pub mod dhcp_reservations;
pub mod dns_forwarders;
pub mod health;
pub mod ipam;
pub mod leases;
pub mod logs;
pub mod records;
pub mod watch;
pub mod zones;

use crate::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(zones::router())
        .merge(records::router())
        .merge(health::router())
        .merge(leases::router())
        .merge(cluster::router())
        .merge(ipam::router())
        .merge(connectivity::router())
        .merge(dhcp::router())
        .merge(logs::router())
        .merge(dhcp_pools::router())
        .merge(dhcp_reservations::router())
        .merge(dhcp_config::router())
        .merge(dns_forwarders::router())
        .merge(watch::router())
}
