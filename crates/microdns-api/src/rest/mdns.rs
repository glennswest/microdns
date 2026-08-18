//! mDNS discovery endpoints.
//!
//! The published records show up under the normal zone/record endpoints with
//! `source: "mdns"`. These show the layer underneath — what the instance has
//! actually heard on the wire, including names that config filtered out, which
//! is what you need when a device is announcing but not resolving.
//!
//! `/mdns/discovered` reports what *this* instance heard on its own segment.
//! The zone those names land in may well be held by another instance — see
//! `holder` in `/mdns/status` — since there is one shared `mdns.lo` for the
//! whole network rather than one per segment.

use crate::security::internal_error;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use microdns_core::config::MdnsSourceConfig;
use microdns_core::types::RecordData;
use serde::Serialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mdns/status", get(mdns_status))
        .route("/mdns/discovered", get(mdns_discovered))
        .route("/mdns/config", get(get_config))
        .route("/mdns/config", put(put_config))
        .route("/mdns/config", delete(delete_config))
}

#[derive(Serialize)]
struct StatusResponse {
    enabled: bool,
    /// Zone discovered names are published into.
    zone: Option<String>,
    /// Entries currently held in the discovery cache.
    cached: usize,
    /// Where the zone lives: this instance, or the address holding it.
    holder: Option<String>,
    /// Entries that pass the allow/deny filters and are published.
    published: usize,
    /// DNS-SD service types seen on the segment.
    service_types: Vec<String>,
    packets: u64,
    records_learned: u64,
    goodbyes: u64,
    expired: u64,
    queries_sent: u64,
    last_packet_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct DiscoveredResponse {
    /// Which instance answered. A sibling polling this endpoint uses it to
    /// recognise — and skip — itself.
    instance_id: String,
    zone: Option<String>,
    count: usize,
    entries: Vec<DiscoveredEntry>,
}

#[derive(Serialize)]
struct DiscoveredEntry {
    /// Name as announced, e.g. `teslatracker-52c4.local`.
    name: String,
    #[serde(rename = "type")]
    record_type: String,
    data: RecordData,
    ttl: u32,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    expires_in: i64,
    /// Address the announcement arrived from.
    from: String,
    /// Where this is published, or `null` when config filters it out.
    published_as: Option<String>,
}

async fn mdns_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let Some(handle) = state.mdns.as_ref() else {
        return Json(StatusResponse {
            enabled: false,
            zone: None,
            cached: 0,
            holder: None,
            published: 0,
            service_types: Vec::new(),
            packets: 0,
            records_learned: 0,
            goodbyes: 0,
            expired: 0,
            queries_sent: 0,
            last_packet_at: None,
        });
    };

    let config = handle.config();
    let zone = handle.zone();
    let cache = handle.cache.lock().unwrap();
    let published = microdns_mdns::translate::desired(cache.entries().cloned(), &config).len();
    Json(StatusResponse {
        enabled: zone.is_some(),
        zone,
        holder: if config.is_holder() {
            None
        } else {
            Some(config.holder.clone())
        },
        cached: cache.len(),
        published,
        service_types: cache.service_types(),
        packets: cache.stats.packets,
        records_learned: cache.stats.records_learned,
        goodbyes: cache.stats.goodbyes,
        expired: cache.stats.expired,
        queries_sent: cache.stats.queries_sent,
        last_packet_at: cache.last_packet_at,
    })
}

async fn mdns_discovered(State(state): State<AppState>) -> Json<DiscoveredResponse> {
    let Some(handle) = state.mdns.as_ref() else {
        return Json(DiscoveredResponse {
            instance_id: state.instance_id.clone(),
            zone: None,
            count: 0,
            entries: Vec::new(),
        });
    };

    let now = Utc::now();
    let config = handle.config();
    let zone = handle.zone();
    let cache = handle.cache.lock().unwrap();
    let mut entries: Vec<DiscoveredEntry> = cache
        .entries()
        .map(|entry| {
            let published_as = zone.as_ref().and_then(|zone| {
                microdns_mdns::translate::translate(entry, &config)
                    .map(|record| format!("{}.{}", record.name, zone))
            });
            DiscoveredEntry {
                name: entry.name.clone(),
                record_type: entry.record_type().to_string(),
                data: entry.data.clone(),
                ttl: entry.ttl,
                first_seen: entry.first_seen,
                last_seen: entry.last_seen,
                expires_at: entry.expires_at,
                expires_in: entry.expires_in(now),
                from: entry.from.to_string(),
                published_as,
            }
        })
        .collect();
    entries.sort_by(|a, b| (&a.name, &a.record_type).cmp(&(&b.name, &b.record_type)));

    Json(DiscoveredResponse {
        instance_id: state.instance_id.clone(),
        zone,
        count: entries.len(),
        entries,
    })
}

/// The stored configuration, or 404 when the source has never been configured.
///
/// This is deliberately the *stored* config rather than the running one: it is
/// what an operator edits, and it is what survives a TOML regeneration on
/// deployments where the config file is generated for them.
async fn get_config(
    State(state): State<AppState>,
) -> Result<Json<MdnsSourceConfig>, (StatusCode, String)> {
    match state
        .db
        .get_runtime_section::<MdnsSourceConfig>(microdns_mdns::CONFIG_SECTION)
        .map_err(internal_error)?
    {
        Some(config) => Ok(Json(config)),
        None => Err((
            StatusCode::NOT_FOUND,
            "mDNS ingest has not been configured on this instance".to_string(),
        )),
    }
}

/// Store the configuration. The running source picks it up within seconds —
/// starting, stopping or re-homing its zone without a restart.
async fn put_config(
    State(state): State<AppState>,
    Json(config): Json<MdnsSourceConfig>,
) -> Result<Json<MdnsSourceConfig>, (StatusCode, String)> {
    let zone = config.zone.trim().trim_end_matches('.').to_string();
    if config.enabled && zone.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "zone is required when mDNS ingest is enabled".to_string(),
        ));
    }
    if microdns_core::reverse::is_reverse_zone(&zone) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("{zone} is a reverse zone; discovered names cannot be published into it"),
        ));
    }
    if config.ttl_min > config.ttl_max {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "ttl_min ({}) is above ttl_max ({})",
                config.ttl_min, config.ttl_max
            ),
        ));
    }

    let stored = MdnsSourceConfig { zone, ..config };
    state
        .db
        .set_runtime_section(microdns_mdns::CONFIG_SECTION, &stored)
        .map_err(internal_error)?;
    Ok(Json(stored))
}

/// Forget the configuration entirely, which also stops the source and
/// withdraws the names it published.
async fn delete_config(State(state): State<AppState>) -> Result<StatusCode, (StatusCode, String)> {
    state
        .db
        .delete_runtime_section(microdns_mdns::CONFIG_SECTION)
        .map_err(internal_error)?;
    Ok(StatusCode::NO_CONTENT)
}
