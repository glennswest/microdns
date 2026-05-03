//! In-memory tracker for "last queried" / "total queries" per (fqdn, type).
//!
//! Bumped on the auth server's hot path; flushed periodically to the
//! `query_stats` redb table by the runtime so a quick restart preserves
//! the recent view. Hydrated on startup so the dashboard never shows an
//! empty "last queried" column right after boot.

use crate::db::Db;
use crate::types::{QueryStat, RecordType};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::warn;

/// One row of in-memory observation state. Kept lock-free by storing the
/// timestamp inside a `parking_lot`-style cell (we use `parking_lot::Mutex`
/// via `dashmap`'s entry API to avoid pulling another dep — single
/// writer per shard already protects us).
#[derive(Debug)]
pub struct TrackedRow {
    /// Most recent query observed.
    pub last_queried_at: parking_lot_lite::Cell<DateTime<Utc>>,
    /// Cumulative total since the row was hydrated/created.
    pub total_count: AtomicU64,
    /// Whether the row has been bumped since the last flush — saves a
    /// round trip if nothing changed.
    pub dirty: std::sync::atomic::AtomicBool,
}

mod parking_lot_lite {
    //! Tiny `Mutex<T>` that supports `Copy` values without pulling
    //! `parking_lot`. Used for `DateTime<Utc>` which is `Copy`.
    use std::sync::Mutex;

    pub struct Cell<T: Copy>(Mutex<T>);

    impl<T: Copy> Cell<T> {
        pub fn new(initial: T) -> Self {
            Self(Mutex::new(initial))
        }
        pub fn get(&self) -> T {
            *self.0.lock().expect("cell poisoned")
        }
        pub fn set(&self, v: T) {
            *self.0.lock().expect("cell poisoned") = v;
        }
    }

    impl<T: Copy + std::fmt::Debug> std::fmt::Debug for Cell<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.get().fmt(f)
        }
    }
}

/// Map key. Lower-cased FQDN (no trailing dot) + record type.
type Key = (String, RecordType);

#[derive(Debug, Default)]
pub struct QueryTracker {
    rows: DashMap<Key, Arc<TrackedRow>>,
}

impl QueryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hydrate from `query_stats` table. Safe to call exactly once at startup.
    pub fn hydrate(&self, db: &Db) {
        let rows = match db.list_query_stats() {
            Ok(r) => r,
            Err(e) => {
                warn!("query_tracker hydrate failed: {e}");
                return;
            }
        };
        for row in rows {
            let key = (normalize_fqdn(&row.fqdn), row.record_type);
            self.rows.insert(
                key,
                Arc::new(TrackedRow {
                    last_queried_at: parking_lot_lite::Cell::new(row.last_queried_at),
                    total_count: AtomicU64::new(row.total_count),
                    dirty: std::sync::atomic::AtomicBool::new(false),
                }),
            );
        }
    }

    /// Bump on every query. Cheap: locks one DashMap shard, increments,
    /// updates timestamp.
    pub fn bump(&self, fqdn: &str, rtype: RecordType, now: DateTime<Utc>) {
        let key = (normalize_fqdn(fqdn), rtype);
        let row = self
            .rows
            .entry(key)
            .or_insert_with(|| {
                Arc::new(TrackedRow {
                    last_queried_at: parking_lot_lite::Cell::new(now),
                    total_count: AtomicU64::new(0),
                    dirty: std::sync::atomic::AtomicBool::new(false),
                })
            })
            .clone();
        row.total_count.fetch_add(1, Ordering::Relaxed);
        row.last_queried_at.set(now);
        row.dirty.store(true, Ordering::Relaxed);
    }

    /// Look up one row.
    pub fn get(&self, fqdn: &str, rtype: RecordType) -> Option<QueryStat> {
        let key = (normalize_fqdn(fqdn), rtype);
        self.rows.get(&key).map(|r| QueryStat {
            fqdn: key.0.clone(),
            record_type: rtype,
            last_queried_at: r.last_queried_at.get(),
            total_count: r.total_count.load(Ordering::Relaxed),
        })
    }

    /// Flush every dirty row to redb in one transaction. Returns the
    /// number of rows written.
    pub fn flush(&self, db: &Db) -> usize {
        let mut to_write: Vec<QueryStat> = Vec::new();
        for entry in self.rows.iter() {
            let row = entry.value();
            if !row.dirty.load(Ordering::Relaxed) {
                continue;
            }
            let (fqdn, rtype) = entry.key();
            to_write.push(QueryStat {
                fqdn: fqdn.clone(),
                record_type: *rtype,
                last_queried_at: row.last_queried_at.get(),
                total_count: row.total_count.load(Ordering::Relaxed),
            });
            row.dirty.store(false, Ordering::Relaxed);
        }
        let n = to_write.len();
        if let Err(e) = db.upsert_query_stats_batch(&to_write) {
            warn!("query_tracker flush failed: {e}");
        }
        n
    }
}

fn normalize_fqdn(fqdn: &str) -> String {
    fqdn.trim_end_matches('.').to_lowercase()
}
