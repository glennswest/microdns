use crate::security::internal_error;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use microdns_core::types::{HealthCheck, HealthStatus, ProbeType, RecordData};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/lb/status", get(lb_status))
        .route("/lb/groups", get(lb_groups))
        .route("/lb/records", get(lb_records))
        .route("/lb/log", get(lb_log))
        .route("/lb/resolutions", get(lb_resolutions))
        .route("/lb/debug", get(lb_debug))
        .route("/lb/probe/{record_id}", post(lb_probe))
        .route(
            "/zones/{zone_id}/records/lb/{name}/{rtype}",
            put(lb_bulk_set),
        )
        .route(
            "/zones/{zone_id}/records/lb/{name}/{rtype}",
            delete(lb_bulk_clear),
        )
}

// ─── Common DTOs ────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    enabled: bool,
    check_interval_secs: u64,
    default_probe: Option<ProbeType>,
    icmp_available: bool,
    aggregate: Aggregate,
    last_cycle: Option<CycleInfo>,
}

#[derive(Serialize, Default)]
struct Aggregate {
    total: usize,
    healthy: usize,
    unhealthy: usize,
    unknown: usize,
    groups: usize,
}

#[derive(Serialize)]
struct CycleInfo {
    /// Most recent `last_checked_at` across all records (proxy for "last
    /// cycle finished at" — exact cycle metadata isn't stored).
    last_check_at: DateTime<Utc>,
    /// Oldest `last_checked_at` — gives an upper bound on cycle staleness.
    oldest_check_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct GroupRow {
    zone_id: Uuid,
    zone_name: String,
    name: String,
    fqdn: String,
    record_type: String,
    members: usize,
    healthy: usize,
    unhealthy: usize,
    unknown: usize,
    failsafe_active: bool,
    probe_type: Option<ProbeType>,
}

#[derive(Serialize)]
struct RecordRow {
    record_id: Uuid,
    zone_id: Uuid,
    zone_name: String,
    name: String,
    fqdn: String,
    record_type: String,
    ip: String,
    enabled: bool,
    status: HealthStatus,
    probe_type: ProbeType,
    last_checked_at: Option<DateTime<Utc>>,
    last_state_change_at: Option<DateTime<Utc>>,
    last_healthy_at: Option<DateTime<Utc>>,
    last_probe_detail: String,
    consecutive_successes: u32,
    consecutive_failures: u32,
    /// `now − last_checked_at` exceeded `2 × check_interval_secs`.
    stale: bool,
    age_seconds: Option<i64>,
}

// ─── GET /lb/status ─────────────────────────────────────────────────────────

async fn lb_status(
    State(state): State<AppState>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let lb = match &state.lb {
        Some(h) => h,
        None => {
            return Ok(Json(StatusResponse {
                enabled: false,
                check_interval_secs: 0,
                default_probe: None,
                icmp_available: false,
                aggregate: Aggregate::default(),
                last_cycle: None,
            }))
        }
    };

    let snap = lb.state.lock().await;
    let agg = snap.aggregate();
    let mut newest: Option<DateTime<Utc>> = None;
    let mut oldest: Option<DateTime<Utc>> = None;
    for (_, h) in snap.iter() {
        if let Some(t) = h.last_checked_at {
            if newest.map(|n| t > n).unwrap_or(true) {
                newest = Some(t);
            }
            if oldest.map(|o| t < o).unwrap_or(true) {
                oldest = Some(t);
            }
        }
    }
    drop(snap);

    let last_cycle = newest.zip(oldest).map(|(n, o)| CycleInfo {
        last_check_at: n,
        oldest_check_at: o,
    });

    let icmp_available = microdns_lb::icmp::icmp_available().await;

    Ok(Json(StatusResponse {
        enabled: true,
        check_interval_secs: lb.check_interval_secs,
        default_probe: Some(lb.default_probe),
        icmp_available,
        aggregate: Aggregate {
            total: agg.total,
            healthy: agg.healthy,
            unhealthy: agg.unhealthy,
            unknown: agg.unknown,
            groups: agg.groups,
        },
        last_cycle,
    }))
}

// ─── GET /lb/groups ─────────────────────────────────────────────────────────

async fn lb_groups(
    State(state): State<AppState>,
) -> Result<Json<Vec<GroupRow>>, (StatusCode, String)> {
    let lb = match &state.lb {
        Some(h) => h,
        None => return Ok(Json(Vec::new())),
    };

    let zones = state.db.list_zones().map_err(internal_error)?;
    let zone_names: HashMap<Uuid, String> =
        zones.iter().map(|z| (z.id, z.name.clone())).collect();

    type Key = (Uuid, String, String);
    #[derive(Default)]
    struct Acc {
        members: usize,
        healthy: usize,
        unhealthy: usize,
        unknown: usize,
        probe_type: Option<ProbeType>,
    }
    let mut acc: HashMap<Key, Acc> = HashMap::new();

    let snap = lb.state.lock().await;
    for (_id, h) in snap.iter() {
        let key = (h.zone_id, h.record_name.clone(), h.record_type.clone());
        let entry = acc.entry(key).or_default();
        entry.members += 1;
        match h.status {
            HealthStatus::Healthy => entry.healthy += 1,
            HealthStatus::Unhealthy => entry.unhealthy += 1,
            HealthStatus::Unknown => entry.unknown += 1,
        }
        if entry.probe_type.is_none() {
            entry.probe_type = Some(h.last_probe_type);
        }
    }
    drop(snap);

    let mut rows: Vec<GroupRow> = acc
        .into_iter()
        .map(|((zone_id, name, rtype), v)| {
            let zone_name = zone_names.get(&zone_id).cloned().unwrap_or_default();
            let fqdn = build_fqdn(&name, &zone_name);
            GroupRow {
                zone_id,
                zone_name,
                name,
                fqdn,
                record_type: rtype,
                members: v.members,
                healthy: v.healthy,
                unhealthy: v.unhealthy,
                unknown: v.unknown,
                failsafe_active: v.members >= 2 && v.healthy == 0 && v.unhealthy == v.members,
                probe_type: v.probe_type,
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        a.zone_name
            .cmp(&b.zone_name)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.record_type.cmp(&b.record_type))
    });

    Ok(Json(rows))
}

// ─── GET /lb/records ────────────────────────────────────────────────────────

async fn lb_records(
    State(state): State<AppState>,
) -> Result<Json<Vec<RecordRow>>, (StatusCode, String)> {
    let lb = match &state.lb {
        Some(h) => h,
        None => return Ok(Json(Vec::new())),
    };

    let zones = state.db.list_zones().map_err(internal_error)?;
    let zone_names: HashMap<Uuid, String> =
        zones.iter().map(|z| (z.id, z.name.clone())).collect();

    let stale_after = Duration::from_secs(lb.check_interval_secs.saturating_mul(2).max(1));
    let now = Utc::now();

    let snap = lb.state.lock().await;
    let mut rows: Vec<RecordRow> = Vec::with_capacity(snap.len());
    for (id, h) in snap.iter() {
        let zone_name = zone_names.get(&h.zone_id).cloned().unwrap_or_default();
        let fqdn = build_fqdn(&h.record_name, &zone_name);

        let ip = match state.db.get_record(id) {
            Ok(Some(r)) => match r.data {
                RecordData::A(a) => a.to_string(),
                RecordData::AAAA(a) => a.to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        };

        let enabled = state
            .db
            .get_record(id)
            .ok()
            .flatten()
            .map(|r| r.enabled)
            .unwrap_or(true);

        let (stale, age_seconds) = match h.last_checked_at {
            Some(t) => {
                let age = now - t;
                let secs = age.num_seconds();
                (
                    secs > stale_after.as_secs() as i64,
                    Some(secs),
                )
            }
            None => (true, None),
        };

        rows.push(RecordRow {
            record_id: *id,
            zone_id: h.zone_id,
            zone_name,
            name: h.record_name.clone(),
            fqdn,
            record_type: h.record_type.clone(),
            ip,
            enabled,
            status: h.status,
            probe_type: h.last_probe_type,
            last_checked_at: h.last_checked_at,
            last_state_change_at: h.last_state_change_at,
            last_healthy_at: h.last_healthy_at,
            last_probe_detail: h.last_probe_detail.clone(),
            consecutive_successes: h.success_count,
            consecutive_failures: h.failure_count,
            stale,
            age_seconds,
        });
    }
    drop(snap);

    rows.sort_by(|a, b| {
        a.zone_name
            .cmp(&b.zone_name)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.ip.cmp(&b.ip))
    });

    Ok(Json(rows))
}

// ─── GET /lb/log ────────────────────────────────────────────────────────────
//
// Last 200 state-change events, most recent first. Each entry has the
// hostname, IP, old → new status, probe type, failsafe flag, and detail.

#[derive(Serialize)]
struct LogEntry {
    at: DateTime<Utc>,
    record_id: Uuid,
    zone_name: String,
    name: String,
    fqdn: String,
    ip: String,
    record_type: String,
    previous_status: Option<HealthStatus>,
    status: HealthStatus,
    failsafe: bool,
    probe_type: ProbeType,
    detail: String,
}

async fn lb_log(
    State(state): State<AppState>,
) -> Result<Json<Vec<LogEntry>>, (StatusCode, String)> {
    let lb = match &state.lb {
        Some(h) => h,
        None => return Ok(Json(Vec::new())),
    };
    let log = match &lb.log {
        Some(l) => l,
        None => return Ok(Json(Vec::new())),
    };
    let snap = log.lock().await.snapshot();
    let rows: Vec<LogEntry> = snap
        .into_iter()
        .map(|c| LogEntry {
            at: c.at,
            record_id: c.record_id,
            zone_name: c.zone_name.clone(),
            name: c.name.clone(),
            fqdn: build_fqdn(&c.name, &c.zone_name),
            ip: c.ip,
            record_type: c.record_type,
            previous_status: c.previous_status,
            status: c.status,
            failsafe: c.failsafe,
            probe_type: c.probe_type,
            detail: c.detail,
        })
        .collect();
    Ok(Json(rows))
}

// ─── GET /lb/resolutions ────────────────────────────────────────────────────
//
// For every monitored (zone, name, type) group, list the IPs that the
// authoritative DNS server would return *right now* — i.e. the members
// whose `enabled` bit is set. Equivalent of ploadb's "Current DNS
// Resolution" panel.

#[derive(Serialize)]
struct ResolutionRow {
    zone_id: Uuid,
    zone_name: String,
    name: String,
    fqdn: String,
    record_type: String,
    answers: Vec<ResolutionAnswer>,
    /// Total members of the group (enabled + disabled).
    total_members: usize,
}

#[derive(Serialize)]
struct ResolutionAnswer {
    ip: String,
    status: HealthStatus,
    /// True if this answer is being returned only because of failsafe.
    failsafe: bool,
    /// Record TTL in seconds — what clients will cache the answer for.
    ttl: u32,
}

async fn lb_resolutions(
    State(state): State<AppState>,
) -> Result<Json<Vec<ResolutionRow>>, (StatusCode, String)> {
    let lb = match &state.lb {
        Some(h) => h,
        None => return Ok(Json(Vec::new())),
    };

    let zones = state.db.list_zones().map_err(internal_error)?;
    let zone_names: HashMap<Uuid, String> =
        zones.iter().map(|z| (z.id, z.name.clone())).collect();

    type GroupKey = (Uuid, String, String);
    #[derive(Default)]
    struct GroupAcc {
        members: Vec<(String, HealthStatus, bool, u32)>, // ip, status, enabled, ttl
    }
    let mut groups: HashMap<GroupKey, GroupAcc> = HashMap::new();

    let snap = lb.state.lock().await;
    for (id, h) in snap.iter() {
        let key = (h.zone_id, h.record_name.clone(), h.record_type.clone());
        let entry = groups.entry(key).or_default();
        let rec = match state.db.get_record(id) {
            Ok(Some(r)) => r,
            _ => continue,
        };
        let ip = match rec.data {
            RecordData::A(a) => a.to_string(),
            RecordData::AAAA(a) => a.to_string(),
            _ => continue,
        };
        entry.members.push((ip, h.status, rec.enabled, rec.ttl));
    }
    drop(snap);

    let mut out: Vec<ResolutionRow> = Vec::with_capacity(groups.len());
    for ((zone_id, name, rtype), acc) in groups {
        let zone_name = zone_names.get(&zone_id).cloned().unwrap_or_default();
        let fqdn = build_fqdn(&name, &zone_name);
        let total_members = acc.members.len();
        let any_unhealthy_enabled = acc
            .members
            .iter()
            .any(|(_, s, en, _)| *en && matches!(s, HealthStatus::Unhealthy));
        let answers: Vec<ResolutionAnswer> = acc
            .members
            .into_iter()
            .filter_map(|(ip, status, enabled, ttl)| {
                if !enabled {
                    return None;
                }
                let failsafe = matches!(status, HealthStatus::Unhealthy) && any_unhealthy_enabled;
                Some(ResolutionAnswer {
                    ip,
                    status,
                    failsafe,
                    ttl,
                })
            })
            .collect();
        out.push(ResolutionRow {
            zone_id,
            zone_name,
            name,
            fqdn,
            record_type: rtype,
            answers,
            total_members,
        });
    }

    out.sort_by(|a, b| {
        a.zone_name
            .cmp(&b.zone_name)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.record_type.cmp(&b.record_type))
    });

    Ok(Json(out))
}

// ─── GET /lb/debug ──────────────────────────────────────────────────────────
//
// Dumps the raw in-memory HealthState plus persisted view for every
// monitored record. Equivalent of ploadb's /debug endpoint — meant for
// ops triage, not for programmatic consumption.

#[derive(Serialize)]
struct DebugDump {
    enabled: bool,
    config: DebugConfig,
    icmp_available: bool,
    halfopen_watcher_count: usize,
    in_memory: Vec<DebugInMemoryRow>,
    persisted: Vec<DebugPersistedRow>,
}

#[derive(Serialize)]
struct DebugConfig {
    check_interval_secs: u64,
    default_probe: ProbeType,
}

#[derive(Serialize)]
struct DebugInMemoryRow {
    record_id: Uuid,
    zone_id: Uuid,
    zone_name: String,
    name: String,
    record_type: String,
    status: HealthStatus,
    probe_type_last: ProbeType,
    success_count: u32,
    failure_count: u32,
    healthy_threshold: u32,
    unhealthy_threshold: u32,
    last_checked_at: Option<DateTime<Utc>>,
    last_state_change_at: Option<DateTime<Utc>>,
    last_healthy_at: Option<DateTime<Utc>>,
    last_probe_detail: String,
    record_enabled: Option<bool>,
    record_ip: Option<String>,
    record_health_check: Option<HealthCheck>,
}

#[derive(Serialize)]
struct DebugPersistedRow {
    record_id: Uuid,
    status: HealthStatus,
    probe_type: ProbeType,
    last_checked_at: DateTime<Utc>,
    last_state_change_at: DateTime<Utc>,
    last_healthy_at: Option<DateTime<Utc>>,
    last_probe_detail: String,
    consecutive_successes: u32,
    consecutive_failures: u32,
}

async fn lb_debug(
    State(state): State<AppState>,
) -> Result<Json<DebugDump>, (StatusCode, String)> {
    let lb = match &state.lb {
        Some(h) => h,
        None => {
            return Ok(Json(DebugDump {
                enabled: false,
                config: DebugConfig {
                    check_interval_secs: 0,
                    default_probe: ProbeType::Ping,
                },
                icmp_available: false,
                halfopen_watcher_count: 0,
                in_memory: Vec::new(),
                persisted: Vec::new(),
            }))
        }
    };

    let zones = state.db.list_zones().map_err(internal_error)?;
    let zone_names: HashMap<Uuid, String> =
        zones.iter().map(|z| (z.id, z.name.clone())).collect();

    let snap = lb.state.lock().await;
    let mut in_memory: Vec<DebugInMemoryRow> = Vec::with_capacity(snap.len());
    for (id, h) in snap.iter() {
        let (record_enabled, record_ip, record_hc) = match state.db.get_record(id) {
            Ok(Some(r)) => {
                let ip = match r.data {
                    RecordData::A(a) => Some(a.to_string()),
                    RecordData::AAAA(a) => Some(a.to_string()),
                    _ => None,
                };
                (Some(r.enabled), ip, r.health_check)
            }
            _ => (None, None, None),
        };
        in_memory.push(DebugInMemoryRow {
            record_id: *id,
            zone_id: h.zone_id,
            zone_name: zone_names.get(&h.zone_id).cloned().unwrap_or_default(),
            name: h.record_name.clone(),
            record_type: h.record_type.clone(),
            status: h.status,
            probe_type_last: h.last_probe_type,
            success_count: h.success_count,
            failure_count: h.failure_count,
            healthy_threshold: h.healthy_threshold,
            unhealthy_threshold: h.unhealthy_threshold,
            last_checked_at: h.last_checked_at,
            last_state_change_at: h.last_state_change_at,
            last_healthy_at: h.last_healthy_at,
            last_probe_detail: h.last_probe_detail.clone(),
            record_enabled,
            record_ip,
            record_health_check: record_hc,
        });
    }
    drop(snap);

    let persisted: Vec<DebugPersistedRow> = state
        .db
        .list_lb_health()
        .map_err(internal_error)?
        .into_iter()
        .map(|p| DebugPersistedRow {
            record_id: p.record_id,
            status: p.status,
            probe_type: p.last_probe_type,
            last_checked_at: p.last_checked_at,
            last_state_change_at: p.last_state_change_at,
            last_healthy_at: p.last_healthy_at,
            last_probe_detail: p.last_probe_detail,
            consecutive_successes: p.consecutive_successes,
            consecutive_failures: p.consecutive_failures,
        })
        .collect();

    let icmp_available = microdns_lb::icmp::icmp_available().await;
    let halfopen_count = match &lb.halfopen {
        Some(h) => h.watcher_count().await,
        None => 0,
    };

    Ok(Json(DebugDump {
        enabled: true,
        config: DebugConfig {
            check_interval_secs: lb.check_interval_secs,
            default_probe: lb.default_probe,
        },
        icmp_available,
        halfopen_watcher_count: halfopen_count,
        in_memory,
        persisted,
    }))
}

// ─── POST /lb/probe/{record_id} ─────────────────────────────────────────────

async fn lb_probe(
    State(state): State<AppState>,
    Path(record_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let record = state
        .db
        .get_record(&record_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "record not found".to_string()))?;
    let hc = record
        .health_check
        .clone()
        .ok_or((StatusCode::BAD_REQUEST, "record has no health_check".to_string()))?;

    let target = match record.data {
        RecordData::A(a) => std::net::IpAddr::V4(a),
        RecordData::AAAA(a) => std::net::IpAddr::V6(a),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "record is not A/AAAA".to_string(),
            ))
        }
    };

    let timeout = Duration::from_secs(if hc.timeout_secs > 0 {
        hc.timeout_secs as u64
    } else {
        5
    });
    let ping_count: u8 = 3;

    let result = microdns_lb::probe::run_probe(
        hc.probe_type,
        target,
        timeout,
        hc.endpoint.as_deref(),
        ping_count,
    )
    .await;

    Ok(Json(serde_json::json!({
        "record_id": record_id,
        "ip": target.to_string(),
        "probe_type": hc.probe_type,
        "success": result.success,
        "detail": result.detail,
        "latency_ms": result.latency.as_millis(),
    })))
}

// ─── PUT /zones/{zone_id}/records/lb/{name}/{rtype} ─────────────────────────

#[derive(Serialize)]
struct BulkResponse {
    matched: usize,
    updated: usize,
}

async fn lb_bulk_set(
    State(state): State<AppState>,
    Path((zone_id, name, rtype)): Path<(Uuid, String, String)>,
    Json(hc): Json<HealthCheck>,
) -> Result<Json<BulkResponse>, (StatusCode, String)> {
    let rtype_norm = rtype.to_uppercase();
    let mut matched = 0usize;
    let mut updated = 0usize;

    let records = state.db.list_records(&zone_id).map_err(internal_error)?;
    for mut r in records {
        if r.name != name || r.data.record_type().to_string() != rtype_norm {
            continue;
        }
        matched += 1;
        let needs_update = match &r.health_check {
            Some(existing) => !health_check_eq(existing, &hc),
            None => true,
        };
        if needs_update {
            r.health_check = Some(hc.clone());
            state.db.update_record(&r).map_err(internal_error)?;
            updated += 1;
        }
    }

    Ok(Json(BulkResponse { matched, updated }))
}

async fn lb_bulk_clear(
    State(state): State<AppState>,
    Path((zone_id, name, rtype)): Path<(Uuid, String, String)>,
) -> Result<Json<BulkResponse>, (StatusCode, String)> {
    let rtype_norm = rtype.to_uppercase();
    let mut matched = 0usize;
    let mut updated = 0usize;

    let records = state.db.list_records(&zone_id).map_err(internal_error)?;
    for mut r in records {
        if r.name != name || r.data.record_type().to_string() != rtype_norm {
            continue;
        }
        matched += 1;
        if r.health_check.is_some() {
            r.health_check = None;
            state.db.update_record(&r).map_err(internal_error)?;
            updated += 1;
        }
    }

    Ok(Json(BulkResponse { matched, updated }))
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn build_fqdn(name: &str, zone_name: &str) -> String {
    let zone = zone_name.trim_end_matches('.');
    if zone.is_empty() {
        return name.to_string();
    }
    if name == "@" || name.is_empty() {
        zone.to_string()
    } else {
        format!("{name}.{zone}")
    }
}

fn health_check_eq(a: &HealthCheck, b: &HealthCheck) -> bool {
    a.probe_type == b.probe_type
        && a.interval_secs == b.interval_secs
        && a.timeout_secs == b.timeout_secs
        && a.unhealthy_threshold == b.unhealthy_threshold
        && a.healthy_threshold == b.healthy_threshold
        && a.endpoint == b.endpoint
}
