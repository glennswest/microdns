use crate::security::internal_error;
use crate::{AppState, DashboardEvent};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::DnsForwarder;
use microdns_msg::events::{ChangeAction, Event};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dns/forwarders", get(list_forwarders).post(create_forwarder))
        .route("/dns/forwarders/{zone}", get(get_forwarder).delete(delete_forwarder))
}

#[derive(Deserialize)]
struct CreateForwarderRequest {
    zone: String,
    servers: Vec<String>,
}

async fn list_forwarders(
    State(state): State<AppState>,
) -> Result<Json<Vec<DnsForwarder>>, (StatusCode, String)> {
    let forwarders = state.db.list_dns_forwarders().map_err(internal_error)?;
    Ok(Json(forwarders))
}

async fn get_forwarder(
    State(state): State<AppState>,
    Path(zone): Path<String>,
) -> Result<Json<DnsForwarder>, (StatusCode, String)> {
    let fwd = state
        .db
        .get_dns_forwarder(&zone)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "forwarder not found".to_string()))?;
    Ok(Json(fwd))
}

async fn create_forwarder(
    State(state): State<AppState>,
    Json(req): Json<CreateForwarderRequest>,
) -> Result<(StatusCode, Json<DnsForwarder>), (StatusCode, String)> {
    if req.servers.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "servers list cannot be empty".to_string()));
    }

    let now = Utc::now();
    let fwd = DnsForwarder {
        zone: req.zone.trim_end_matches('.').to_lowercase(),
        servers: req.servers,
        created_at: now,
        updated_at: now,
    };

    state.db.create_dns_forwarder(&fwd).map_err(internal_error)?;

    // Invalidate recursor cache — forwarder changes affect resolution path
    if let Some(ref cache) = state.recursor_cache {
        cache.clear();
    }

    let _ = state.event_tx.send(DashboardEvent::DnsForwarderChanged {
        action: "ADDED".to_string(),
        zone: fwd.zone.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DnsForwarderChanged {
            instance_id: state.instance_id.clone(),
            zone: fwd.zone.clone(),
            action: ChangeAction::Created,
            timestamp: now,
        };
        let _ = bus.publish(&event).await;
    }

    Ok((StatusCode::CREATED, Json(fwd)))
}

async fn delete_forwarder(
    State(state): State<AppState>,
    Path(zone): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .db
        .delete_dns_forwarder(&zone)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    // Invalidate recursor cache — forwarder deletion changes resolution path
    if let Some(ref cache) = state.recursor_cache {
        cache.clear();
    }

    let _ = state.event_tx.send(DashboardEvent::DnsForwarderChanged {
        action: "DELETED".to_string(),
        zone: zone.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DnsForwarderChanged {
            instance_id: state.instance_id.clone(),
            zone,
            action: ChangeAction::Deleted,
            timestamp: Utc::now(),
        };
        let _ = bus.publish(&event).await;
    }

    Ok(StatusCode::NO_CONTENT)
}
