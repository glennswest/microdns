use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct LogQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub level: Option<String>,
    pub module: Option<String>,
}

fn default_limit() -> usize {
    100
}

async fn get_logs(
    State(state): State<AppState>,
    Query(params): Query<LogQuery>,
) -> Json<serde_json::Value> {
    let limit = params.limit.min(1000);

    match &state.log_buffer {
        Some(buf) => {
            let entries = buf.query(limit, params.level.as_deref(), params.module.as_deref());
            Json(serde_json::json!({ "entries": entries }))
        }
        None => Json(serde_json::json!({ "entries": [], "error": "log buffer not configured" })),
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/logs", get(get_logs))
}
