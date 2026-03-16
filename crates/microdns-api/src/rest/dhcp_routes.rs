use crate::security::internal_error;
use crate::{AppState, DashboardEvent};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::StaticRoute;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/dhcp/pools/{pool_id}/routes",
        get(list_routes).post(add_route),
    ).route(
        "/dhcp/pools/{pool_id}/routes/{route_id}",
        axum::routing::delete(delete_route),
    )
}

#[derive(Serialize)]
struct RoutesResponse {
    routes: Vec<StaticRoute>,
}

#[derive(Deserialize)]
struct AddRouteRequest {
    destination: String,
    gateway: String,
    #[serde(default)]
    managed_by: Option<String>,
}

async fn list_routes(
    State(state): State<AppState>,
    Path(pool_id): Path<Uuid>,
) -> Result<Json<RoutesResponse>, (StatusCode, String)> {
    let pool = state
        .db
        .get_dhcp_pool(&pool_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "pool not found".to_string()))?;

    let routes = pool.static_routes.unwrap_or_default();
    Ok(Json(RoutesResponse { routes }))
}

async fn add_route(
    State(state): State<AppState>,
    Path(pool_id): Path<Uuid>,
    Json(req): Json<AddRouteRequest>,
) -> Result<(StatusCode, Json<StaticRoute>), (StatusCode, String)> {
    let mut pool = state
        .db
        .get_dhcp_pool(&pool_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "pool not found".to_string()))?;

    let routes = pool.static_routes.get_or_insert_with(Vec::new);

    // Duplicate detection: same destination + gateway returns existing
    if let Some(existing) = routes.iter().find(|r| r.destination == req.destination && r.gateway == req.gateway) {
        return Ok((StatusCode::OK, Json(existing.clone())));
    }

    let route = StaticRoute {
        id: Uuid::new_v4(),
        destination: req.destination,
        gateway: req.gateway,
        managed_by: req.managed_by,
    };

    routes.push(route.clone());
    pool.updated_at = Utc::now();

    state.db.update_dhcp_pool(&pool).map_err(internal_error)?;

    let _ = state.event_tx.send(DashboardEvent::DhcpPoolChanged {
        action: "MODIFIED".to_string(),
        pool_id: pool.id.to_string(),
        pool_name: pool.name.clone(),
    });

    Ok((StatusCode::CREATED, Json(route)))
}

async fn delete_route(
    State(state): State<AppState>,
    Path((pool_id, route_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let mut pool = state
        .db
        .get_dhcp_pool(&pool_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "pool not found".to_string()))?;

    let routes = pool.static_routes.get_or_insert_with(Vec::new);
    let before_len = routes.len();
    routes.retain(|r| r.id != route_id);

    if routes.len() == before_len {
        return Err((StatusCode::NOT_FOUND, "route not found".to_string()));
    }

    if routes.is_empty() {
        pool.static_routes = None;
    }
    pool.updated_at = Utc::now();

    state.db.update_dhcp_pool(&pool).map_err(internal_error)?;

    let _ = state.event_tx.send(DashboardEvent::DhcpPoolChanged {
        action: "MODIFIED".to_string(),
        pool_id: pool.id.to_string(),
        pool_name: pool.name.clone(),
    });

    Ok(StatusCode::NO_CONTENT)
}
