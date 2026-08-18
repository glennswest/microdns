//! Zone-transfer settings: the AXFR ACL, NOTIFY targets, and mirrored zones.
//!
//! Stored in the database and applied live, for the same reason the mDNS config
//! is: on instances whose `microdns.toml` is generated from a Network CRD, a
//! `[dns.auth]` edit is discarded the next time it regenerates.

use crate::security::internal_error;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use microdns_core::config::ZoneTransferConfig;

/// Database section the settings live under. Shared with the binary, which
/// seeds it from the config file on first run.
pub const CONFIG_SECTION: &str = "zone_transfer";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/zone-transfer/config", get(get_config))
        .route("/zone-transfer/config", put(put_config))
        .route("/zone-transfer/config", delete(delete_config))
}

/// The stored settings, or 404 when none have been stored on this instance.
async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<ZoneTransferConfig>, (StatusCode, String)> {
    match state
        .db
        .get_runtime_section::<ZoneTransferConfig>(CONFIG_SECTION)
        .map_err(internal_error)?
    {
        Some(config) => Ok(Json(config)),
        None => Err((
            StatusCode::NOT_FOUND,
            "zone transfer has not been configured on this instance".to_string(),
        )),
    }
}

/// Store the settings. The listener, the announcer and the mirror agent all
/// pick them up within seconds, without a restart.
async fn put_config(
    State(state): State<AppState>,
    Json(config): Json<ZoneTransferConfig>,
) -> Result<Json<ZoneTransferConfig>, (StatusCode, String)> {
    // Reject what would silently do nothing, rather than storing it and leaving
    // an operator to wonder why transfers still fail.
    for cidr in &config.allow_transfer {
        if microdns_auth::server::IpNet::parse(cidr).is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("'{cidr}' is not an address or CIDR block"),
            ));
        }
    }
    for target in &config.notify {
        if microdns_auth::runtime::parse_addr(target).is_none() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("notify target '{target}' is not an address"),
            ));
        }
    }
    for secondary in &config.secondary {
        if microdns_auth::secondary::SecondaryZone::parse(
            &secondary.zone,
            &secondary.primary,
            secondary.refresh_secs,
        )
        .is_none()
        {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "secondary zone '{}' is invalid: primary '{}' must be an address, \
                     and the zone name must not be empty",
                    secondary.zone, secondary.primary
                ),
            ));
        }
    }

    state
        .db
        .set_runtime_section(CONFIG_SECTION, &config)
        .map_err(internal_error)?;
    Ok(Json(config))
}

/// Forget the settings: transfers are refused, nothing is announced, and no
/// zone is mirrored.
async fn delete_config(State(state): State<AppState>) -> Result<StatusCode, (StatusCode, String)> {
    state
        .db
        .delete_runtime_section(CONFIG_SECTION)
        .map_err(internal_error)?;
    Ok(StatusCode::NO_CONTENT)
}
