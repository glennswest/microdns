//! Half-open TCP keepalive monitor.
//!
//! For each record using `ProbeType::TcpHalfOpen`, this module maintains a
//! single long-running TCP connection to the backend. The connection
//! carries no application data — `SO_KEEPALIVE` is enabled and the
//! `TCP_KEEPIDLE` / `TCP_KEEPINTVL` / `TCP_KEEPCNT` knobs are tuned from
//! the record's `HealthCheck`. The only packets on the wire are kernel
//! keepalive probes.
//!
//! When the backend dies (RST received, keepalive timeout reached, route
//! drops), the kernel notifies the per-record task via the long-pending
//! `read()` returning EOF or an error. The task flips the record to
//! `Unhealthy` immediately, then enters a reconnect loop. On successful
//! reconnect it flips back to `Healthy`.
//!
//! State updates flow into the same `HealthState` the probe cycle reads
//! from — so persistence, dashboard, and failsafe all keep working
//! identically. The probe cycle skips half-open records when collecting
//! probe targets.

use crate::monitor::StateChange;
use crate::state::HealthState;
use chrono::Utc;
use microdns_core::types::{HealthStatus, ProbeType};
use socket2::TcpKeepalive;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// One half-open watcher per record. The map stores the JoinHandle plus a
/// shutdown signal so we can cleanly stop the task when the record is
/// removed or its config changes (probe type, port).
pub struct HalfOpenManager {
    inner: Mutex<Inner>,
    state: Arc<Mutex<HealthState>>,
    events: broadcast::Sender<StateChange>,
}

struct Inner {
    watchers: HashMap<Uuid, Watcher>,
}

struct Watcher {
    /// What we're connected to — used to detect config changes that need
    /// a teardown+rebuild.
    target: SocketAddr,
    keepalive_secs: u64,
    handle: JoinHandle<()>,
    stop: watch::Sender<bool>,
}

/// Parameters for one half-open watcher.
pub struct WatcherSpec {
    pub record_id: Uuid,
    pub zone_id: Uuid,
    pub zone_name: String,
    pub name: String,
    pub record_type: String,
    pub target: SocketAddr,
    /// Idle interval (no data) before the kernel sends the first keepalive.
    /// Total failure-detection latency ≈ idle + 3 × (idle / 3) ≈ 2 × idle.
    pub keepalive_secs: u64,
    /// How long to wait between reconnect attempts when the backend is
    /// down. Defaults to keepalive_secs if zero.
    pub reconnect_secs: u64,
}

impl HalfOpenManager {
    pub fn new(state: Arc<Mutex<HealthState>>, events: broadcast::Sender<StateChange>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                watchers: HashMap::new(),
            }),
            state,
            events,
        }
    }

    /// Ensure a watcher is running for the given record. If a watcher
    /// already exists with the same target+keepalive, this is a no-op.
    /// If the target or keepalive changed, the old watcher is torn down
    /// and a new one started.
    pub async fn ensure(&self, spec: WatcherSpec) {
        let mut inner = self.inner.lock().await;
        if let Some(existing) = inner.watchers.get(&spec.record_id) {
            if existing.target == spec.target && existing.keepalive_secs == spec.keepalive_secs {
                return;
            }
            // Config changed — tear down the old one.
            let _ = existing.stop.send(true);
            existing.handle.abort();
            inner.watchers.remove(&spec.record_id);
        }

        let (stop_tx, stop_rx) = watch::channel(false);
        let target = spec.target;
        let keepalive_secs = spec.keepalive_secs.max(1);
        let reconnect_secs = if spec.reconnect_secs == 0 {
            keepalive_secs
        } else {
            spec.reconnect_secs
        };

        let state = self.state.clone();
        let events = self.events.clone();
        let task_spec = TaskSpec {
            record_id: spec.record_id,
            zone_id: spec.zone_id,
            zone_name: spec.zone_name,
            name: spec.name,
            record_type: spec.record_type,
            target: spec.target,
            keepalive_secs,
            reconnect_secs,
        };

        let handle = tokio::spawn(run_watcher(task_spec, state, events, stop_rx));

        inner.watchers.insert(
            spec.record_id,
            Watcher {
                target,
                keepalive_secs,
                handle,
                stop: stop_tx,
            },
        );
    }

    /// Stop watchers for any record IDs not in `live`. Returns the IDs that
    /// were stopped.
    pub async fn retain_only(&self, live: &std::collections::HashSet<Uuid>) -> Vec<Uuid> {
        let mut inner = self.inner.lock().await;
        let to_drop: Vec<Uuid> = inner
            .watchers
            .keys()
            .filter(|id| !live.contains(*id))
            .copied()
            .collect();
        for id in &to_drop {
            if let Some(w) = inner.watchers.remove(id) {
                let _ = w.stop.send(true);
                w.handle.abort();
            }
        }
        to_drop
    }

    /// Stop every watcher (called at shutdown).
    pub async fn shutdown(&self) {
        let mut inner = self.inner.lock().await;
        for (_, w) in inner.watchers.drain() {
            let _ = w.stop.send(true);
            w.handle.abort();
        }
    }

    pub async fn watcher_count(&self) -> usize {
        self.inner.lock().await.watchers.len()
    }
}

#[derive(Clone)]
struct TaskSpec {
    record_id: Uuid,
    zone_id: Uuid,
    zone_name: String,
    name: String,
    record_type: String,
    target: SocketAddr,
    keepalive_secs: u64,
    reconnect_secs: u64,
}

async fn run_watcher(
    spec: TaskSpec,
    state: Arc<Mutex<HealthState>>,
    events: broadcast::Sender<StateChange>,
    mut stop: watch::Receiver<bool>,
) {
    info!(
        "half-open watcher started: record={} target={} keepalive={}s",
        spec.record_id, spec.target, spec.keepalive_secs
    );

    loop {
        if *stop.borrow() {
            break;
        }

        // Try to connect. Bounded by keepalive_secs to avoid hanging forever
        // on a black-holed peer.
        let connect_result = tokio::select! {
            r = tokio::time::timeout(
                Duration::from_secs(spec.keepalive_secs),
                TcpStream::connect(spec.target),
            ) => r,
            _ = stop.changed() => {
                if *stop.borrow() { break; } else { continue; }
            }
        };

        let mut stream = match connect_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                publish_status(
                    &spec,
                    &state,
                    &events,
                    HealthStatus::Unhealthy,
                    format!("connect: {e}"),
                )
                .await;
                if !sleep_or_stop(Duration::from_secs(spec.reconnect_secs), &mut stop).await {
                    break;
                }
                continue;
            }
            Err(_) => {
                publish_status(
                    &spec,
                    &state,
                    &events,
                    HealthStatus::Unhealthy,
                    format!(
                        "connect timeout after {}s",
                        spec.keepalive_secs
                    ),
                )
                .await;
                if !sleep_or_stop(Duration::from_secs(spec.reconnect_secs), &mut stop).await {
                    break;
                }
                continue;
            }
        };

        // Configure SO_KEEPALIVE and TCP_KEEPIDLE/INTVL/CNT.
        if let Err(e) = enable_keepalive(&stream, spec.keepalive_secs) {
            warn!(
                "half-open: keepalive setup failed for {}: {e} — falling back to default kernel timeout",
                spec.target
            );
        }

        publish_status(
            &spec,
            &state,
            &events,
            HealthStatus::Healthy,
            format!("connected to {}", spec.target),
        )
        .await;

        // Sit on read forever. The connection breaks when:
        //   • the peer sends RST (read returns ECONNRESET / 0 bytes)
        //   • the peer sends FIN (read returns 0 bytes)
        //   • keepalive declares the peer dead (read returns ETIMEDOUT)
        //   • the route disappears (varies)
        let mut buf = [0u8; 64];
        let break_reason = tokio::select! {
            r = stream.read(&mut buf) => match r {
                Ok(0) => "peer closed (FIN/EOF)".to_string(),
                Ok(n) => {
                    // We didn't expect application data, but receiving
                    // some isn't a failure — the backend is clearly alive.
                    // Loop back to read more (treat the connection as
                    // still healthy).
                    debug!(
                        "half-open: unexpected {n} bytes from {} (record {}); ignoring",
                        spec.target, spec.record_id
                    );
                    // Continue reading until error.
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break "peer closed after data".to_string(),
                            Ok(_) => continue,
                            Err(e) => break format!("read after data: {e}"),
                        }
                    }
                }
                Err(e) => format!("read: {e}"),
            },
            _ = stop.changed() => {
                if *stop.borrow() {
                    info!("half-open watcher stopping: record={}", spec.record_id);
                    return;
                }
                continue;
            }
        };

        publish_status(&spec, &state, &events, HealthStatus::Unhealthy, break_reason).await;

        if !sleep_or_stop(Duration::from_secs(spec.reconnect_secs), &mut stop).await {
            break;
        }
    }
}

/// Sleep up to `dur` or until shutdown. Returns `true` if the caller
/// should continue looping, `false` if a shutdown was signalled.
async fn sleep_or_stop(dur: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => true,
        _ = stop.changed() => !*stop.borrow(),
    }
}

fn enable_keepalive(stream: &TcpStream, keepalive_secs: u64) -> std::io::Result<()> {
    let sock = socket2::SockRef::from(stream);
    sock.set_keepalive(true)?;
    let idle = Duration::from_secs(keepalive_secs);
    let intvl = Duration::from_secs(keepalive_secs.saturating_div(3).max(1));
    let mut ka = TcpKeepalive::new().with_time(idle);
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd"
    ))]
    {
        ka = ka.with_interval(intvl);
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            ka = ka.with_retries(3);
        }
    }
    let _ = intvl; // suppress unused warning on platforms where we can't set it
    sock.set_tcp_keepalive(&ka)?;
    Ok(())
}

/// Update HealthState for the watcher's record and emit a StateChange
/// event if the status flipped.
async fn publish_status(
    spec: &TaskSpec,
    state: &Arc<Mutex<HealthState>>,
    events: &broadcast::Sender<StateChange>,
    status: HealthStatus,
    detail: String,
) {
    let now = Utc::now();
    let success = matches!(status, HealthStatus::Healthy);

    // Make sure the record is registered.
    {
        let mut guard = state.lock().await;
        guard.register(
            spec.record_id,
            1,
            1,
            spec.zone_id,
            spec.name.clone(),
            spec.record_type.clone(),
        );
    }

    let result = {
        let mut guard = state.lock().await;
        guard.record_probe_result_with_prev(
            &spec.record_id,
            success,
            now,
            ProbeType::TcpHalfOpen,
            detail.clone(),
        )
    };

    if let Some((prev, Some(new_status))) = result {
        let change = StateChange {
            record_id: spec.record_id,
            zone_id: spec.zone_id,
            zone_name: spec.zone_name.clone(),
            name: spec.name.clone(),
            ip: spec.target.ip().to_string(),
            record_type: spec.record_type.clone(),
            status: new_status,
            previous_status: Some(prev),
            failsafe: false,
            probe_type: ProbeType::TcpHalfOpen,
            detail,
            at: now,
        };
        info!(
            "half-open: {}.{} {} → {}",
            spec.name, spec.zone_name, spec.target, new_status
        );
        let _ = events.send(change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn watcher_detects_listener_drop() {
        // Stand up a TCP listener that holds the connection open until we
        // kill it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (kill_tx, mut kill_rx) = tokio::sync::watch::channel(false);

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Hold open until kill signal, then drop (peer sees FIN/RST).
            let _ = kill_rx.changed().await;
            let _ = sock.shutdown().await;
            drop(sock);
            drop(listener);
        });

        let state = Arc::new(Mutex::new(HealthState::new()));
        let (events_tx, mut events_rx) = broadcast::channel::<StateChange>(16);
        let mgr = HalfOpenManager::new(state.clone(), events_tx);

        let record_id = Uuid::new_v4();
        let zone_id = Uuid::new_v4();
        mgr.ensure(WatcherSpec {
            record_id,
            zone_id,
            zone_name: "test.lo".into(),
            name: "api".into(),
            record_type: "A".into(),
            target: addr,
            keepalive_secs: 1,
            reconnect_secs: 1,
        })
        .await;

        // First state-change should be → Healthy
        let first =
            tokio::time::timeout(Duration::from_secs(3), events_rx.recv())
                .await
                .expect("no event for healthy state")
                .expect("event channel closed");
        assert_eq!(first.status, HealthStatus::Healthy);
        assert_eq!(first.record_id, record_id);
        assert_eq!(first.probe_type, ProbeType::TcpHalfOpen);

        // Kill the server side of the connection.
        let _ = kill_tx.send(true);
        let _ = server.await;

        // Watcher should see the EOF/RST and flip → Unhealthy.
        let next = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events_rx.recv().await {
                    Ok(c) if c.status == HealthStatus::Unhealthy => return Some(c),
                    Ok(_) => continue,
                    Err(_) => return None,
                }
            }
        })
        .await
        .expect("no unhealthy event")
        .expect("channel closed");
        assert_eq!(next.status, HealthStatus::Unhealthy);
        assert_eq!(next.record_id, record_id);

        mgr.shutdown().await;
    }
}
