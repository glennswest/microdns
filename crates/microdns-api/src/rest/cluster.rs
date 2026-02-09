use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/cluster/status", get(cluster_status))
}

#[derive(Serialize)]
struct ClusterStatusResponse {
    instance_id: String,
    instances: Vec<InstanceInfo>,
}

#[derive(Serialize)]
struct InstanceInfo {
    instance_id: String,
    mode: String,
    uptime_secs: u64,
    active_leases: u64,
    zones_served: u64,
    last_seen: String,
    healthy: bool,
}

async fn cluster_status(
    State(state): State<AppState>,
) -> Result<Json<ClusterStatusResponse>, (StatusCode, String)> {
    // If we have a heartbeat tracker (coordinator mode), return all instance status
    if let Some(tracker) = &state.heartbeat_tracker {
        let statuses = tracker.get_all_status().await;
        let instances: Vec<InstanceInfo> = statuses
            .into_iter()
            .map(|s| InstanceInfo {
                instance_id: s.instance_id,
                mode: s.mode,
                uptime_secs: s.uptime_secs,
                active_leases: s.active_leases,
                zones_served: s.zones_served,
                last_seen: s.last_seen.to_rfc3339(),
                healthy: s.healthy,
            })
            .collect();

        Ok(Json(ClusterStatusResponse {
            instance_id: state.instance_id.clone(),
            instances,
        }))
    } else {
        // Non-coordinator: just report self
        Ok(Json(ClusterStatusResponse {
            instance_id: state.instance_id.clone(),
            instances: vec![],
        }))
    }
}
