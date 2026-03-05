use crate::security::internal_error;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::DbInstanceConfig;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/dhcp/config", get(get_config).patch(patch_config))
}

#[derive(Deserialize)]
struct PatchConfigRequest {
    listen_dns: Option<Option<String>>,
    listen_api: Option<Option<String>>,
    dhcp_interface: Option<Option<String>>,
    dhcp_mode: Option<Option<String>>,
    server_ip: Option<Option<String>>,
}

async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<DbInstanceConfig>, (StatusCode, String)> {
    let config = state
        .db
        .get_instance_config()
        .map_err(internal_error)?
        .unwrap_or(DbInstanceConfig {
            listen_dns: None,
            listen_api: None,
            dhcp_interface: None,
            dhcp_mode: None,
            server_ip: None,
            updated_at: Utc::now(),
        });
    Ok(Json(config))
}

async fn patch_config(
    State(state): State<AppState>,
    Json(req): Json<PatchConfigRequest>,
) -> Result<Json<DbInstanceConfig>, (StatusCode, String)> {
    let mut config = state
        .db
        .get_instance_config()
        .map_err(internal_error)?
        .unwrap_or(DbInstanceConfig {
            listen_dns: None,
            listen_api: None,
            dhcp_interface: None,
            dhcp_mode: None,
            server_ip: None,
            updated_at: Utc::now(),
        });

    if let Some(v) = req.listen_dns { config.listen_dns = v; }
    if let Some(v) = req.listen_api { config.listen_api = v; }
    if let Some(v) = req.dhcp_interface { config.dhcp_interface = v; }
    if let Some(v) = req.dhcp_mode { config.dhcp_mode = v; }
    if let Some(v) = req.server_ip { config.server_ip = v; }
    config.updated_at = Utc::now();

    state.db.set_instance_config(&config).map_err(internal_error)?;

    Ok(Json(config))
}
