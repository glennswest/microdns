pub mod cluster;
pub mod connectivity;
pub mod dhcp;
pub mod health;
pub mod ipam;
pub mod leases;
pub mod records;
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
}
