//! mDNS discovery endpoints.
//!
//! The published records show up under the normal zone/record endpoints with
//! `source: "mdns"`. These two show the layer underneath — what the instance
//! has actually heard on the wire, including names that config filtered out,
//! which is what you need when a device is announcing but not resolving.

use crate::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use microdns_core::types::RecordData;
use serde::Serialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mdns/status", get(mdns_status))
        .route("/mdns/discovered", get(mdns_discovered))
}

#[derive(Serialize)]
struct StatusResponse {
    enabled: bool,
    /// Zone discovered names are published into.
    zone: Option<String>,
    /// Entries currently held in the discovery cache.
    cached: usize,
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

    let cache = handle.cache.lock().unwrap();
    let published = microdns_mdns::translate::desired(cache.entries().cloned(), &handle.config).len();
    Json(StatusResponse {
        enabled: true,
        zone: Some(handle.zone.clone()),
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
            zone: None,
            count: 0,
            entries: Vec::new(),
        });
    };

    let now = Utc::now();
    let cache = handle.cache.lock().unwrap();
    let mut entries: Vec<DiscoveredEntry> = cache
        .entries()
        .map(|entry| {
            let published_as = microdns_mdns::translate::translate(entry, &handle.config)
                .map(|record| format!("{}.{}", record.name, handle.zone));
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
        zone: Some(handle.zone.clone()),
        count: entries.len(),
        entries,
    })
}
