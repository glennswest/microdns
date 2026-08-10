use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use tracing::error;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health_check))
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    zones: u64,
    uptime_seconds: u64,
    uptime: String,
}

async fn health_check(State(state): State<AppState>) -> Response {
    // 200 must mean "this instance can actually serve records" — consumers
    // (mkube) gate all DNS operations on this endpoint. A database that is
    // missing, locked, or corrupt returns 503 naming the failing check.
    // An empty database (zero zones) is healthy.
    let zones = match state.db.zone_count() {
        Ok(n) => n,
        Err(e) => {
            error!("health check: database read failed: {e}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "status": "unhealthy",
                    "check": "database",
                    "error": e.to_string(),
                })),
            )
                .into_response();
        }
    };

    let elapsed = state.started_at.elapsed();
    let secs = elapsed.as_secs();
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    let uptime = if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    };

    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        zones,
        uptime_seconds: secs,
        uptime,
    })
    .into_response()
}
