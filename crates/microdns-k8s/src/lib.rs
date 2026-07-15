//! Kubernetes DNS source for MicroDNS.
//!
//! Watches a Kubernetes `kube-apiserver` (e.g. the rustkube control plane) and
//! keeps a managed `cluster.local` zone — plus its reverse zones — in sync with
//! the cluster's Services, EndpointSlices/Endpoints and Pods, implementing the
//! Kubernetes DNS-Based Service Discovery spec. This makes a MicroDNS instance
//! the in-cluster DNS (a CoreDNS / OpenShift-DNS equivalent) for the masters.
//!
//! It runs as an in-process task inside the microdns binary — the same way
//! CoreDNS runs its `kubernetes` plugin — sharing the process's redb DB and DNS
//! listeners. microdns itself is the "service"/pod; this is one component of it.
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use microdns_core::db::Db;
//! use microdns_k8s::{K8sConfig, K8sSource};
//!
//! let db = Arc::new(Db::open(std::path::Path::new("microdns.redb"))?);
//! let (_tx, shutdown) = tokio::sync::watch::channel(false);
//! let source = K8sSource::new(db, K8sConfig::default());
//! source.run(shutdown).await?;
//! # Ok(()) }
//! ```

mod config;
mod reconcile;
mod translate;
mod watch;

pub use config::{in_cluster, EndpointSource, K8sConfig};
pub use translate::DesiredRecord;

use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use k8s_openapi::api::core::v1::{Endpoints, Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::{reflector, watcher, WatchStreamExt};
use kube::{Api, Client};
use microdns_core::db::Db;
use tokio::sync::{watch as tokio_watch, Notify};
use tokio::task::JoinSet;
use tracing::{info, warn};

use reconcile::Reconciler;
use watch::Stores;

/// The Kubernetes DNS source: owns the DB handle and config, and runs the
/// watch + reconcile loop until the shutdown signal fires or the apiserver
/// connection fails unrecoverably.
pub struct K8sSource {
    db: Arc<Db>,
    config: K8sConfig,
}

impl K8sSource {
    pub fn new(db: Arc<Db>, config: K8sConfig) -> Self {
        Self { db, config }
    }

    /// Connect to the apiserver, start the reflector watchers and drive the
    /// reconcile loop until `shutdown` flips to `true`.
    pub async fn run(self, mut shutdown: tokio_watch::Receiver<bool>) -> anyhow::Result<()> {
        let client = build_client(&self.config).await?;
        info!(
            "microdns-k8s: watching apiserver for zone {} (endpoint source: {:?})",
            self.config.cluster_domain, self.config.endpoint_source
        );

        // A reflector store per resource, driven by its own watcher task.
        let (svc_store, svc_writer) = reflector::store::<Service>();
        let (ep_store, ep_writer) = reflector::store::<Endpoints>();
        let (slice_store, slice_writer) = reflector::store::<EndpointSlice>();
        let (pod_store, pod_writer) = reflector::store::<Pod>();

        let stores = Stores {
            services: svc_store,
            endpoints: ep_store,
            slices: slice_store,
            pods: pod_store,
        };

        // Any watch event wakes the (debounced) reconcile loop.
        let notify = Arc::new(Notify::new());
        let wcfg = watcher::Config::default();

        let mut set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        set.spawn(watch_loop(
            reflector(svc_writer, watcher(Api::<Service>::all(client.clone()), wcfg.clone())),
            notify.clone(),
            "services",
        ));
        set.spawn(watch_loop(
            reflector(ep_writer, watcher(Api::<Endpoints>::all(client.clone()), wcfg.clone())),
            notify.clone(),
            "endpoints",
        ));
        set.spawn(watch_loop(
            reflector(slice_writer, watcher(Api::<EndpointSlice>::all(client.clone()), wcfg.clone())),
            notify.clone(),
            "endpointslices",
        ));
        set.spawn(watch_loop(
            reflector(pod_writer, watcher(Api::<Pod>::all(client.clone()), wcfg.clone())),
            notify.clone(),
            "pods",
        ));
        set.spawn(reconcile_loop(self.db.clone(), self.config.clone(), stores, notify));

        // Run until shutdown, or until a task exits (which is always an error:
        // the loops are meant to run forever).
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("microdns-k8s: shutdown requested");
                        break;
                    }
                }
                Some(joined) = set.join_next() => {
                    match joined {
                        Ok(Ok(())) => warn!("microdns-k8s: a watch task exited unexpectedly"),
                        Ok(Err(e)) => warn!("microdns-k8s: task failed: {e}"),
                        Err(e) => warn!("microdns-k8s: task panicked: {e}"),
                    }
                    break;
                }
            }
        }

        set.shutdown().await;
        Ok(())
    }
}

/// Drive a reflector stream to keep its store current, waking `notify` on every
/// event. `default_backoff` keeps the watch alive across transient errors and
/// 410-Gone relists.
async fn watch_loop<S, K>(stream: S, notify: Arc<Notify>, what: &'static str) -> anyhow::Result<()>
where
    S: futures::Stream<Item = Result<watcher::Event<K>, watcher::Error>> + Send + 'static,
    K: kube::Resource + Clone + Send + Sync + 'static,
    K::DynamicType: Default + Eq + std::hash::Hash + Clone,
{
    let mut stream = Box::pin(stream.default_backoff());
    while stream.try_next().await?.is_some() {
        notify.notify_one();
    }
    warn!("microdns-k8s: {what} watch stream ended");
    Ok(())
}

/// Debounced reconcile loop: on any watch event, wait for a quiet window then
/// converge the zone to the current cluster state.
async fn reconcile_loop(
    db: Arc<Db>,
    config: K8sConfig,
    stores: Stores,
    notify: Arc<Notify>,
) -> anyhow::Result<()> {
    let mut reconciler = Reconciler::new(db, &config)?;
    let debounce = Duration::from_millis(config.debounce_ms);
    loop {
        notify.notified().await;
        tokio::time::sleep(debounce).await;
        let desired = stores.desired(
            &config.cluster_domain,
            config.default_ttl,
            config.endpoint_source,
            &config.dns_service_ips,
        );
        if let Err(e) = reconciler.apply(desired) {
            warn!("microdns-k8s: reconcile failed: {e}");
        }
    }
}

/// Build a kube client, either from an explicit kubeconfig path or inferred from
/// the environment (in-cluster service account / default kubeconfig).
async fn build_client(config: &K8sConfig) -> anyhow::Result<Client> {
    match &config.kubeconfig {
        Some(path) => {
            let kubeconfig = kube::config::Kubeconfig::read_from(path)?;
            let cfg = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default()).await?;
            Ok(Client::try_from(cfg)?)
        }
        None => Ok(Client::try_default().await?),
    }
}
