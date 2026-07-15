//! Configuration for the Kubernetes DNS source.

use std::net::IpAddr;
use std::path::PathBuf;

/// Where to read endpoint (backing-pod) state from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSource {
    /// Prefer EndpointSlices, fall back to legacy Endpoints per service (default).
    Auto,
    /// EndpointSlices only (`discovery.k8s.io/v1`).
    Slices,
    /// Legacy core/v1 Endpoints only.
    Endpoints,
}

impl Default for EndpointSource {
    fn default() -> Self {
        EndpointSource::Auto
    }
}

/// How to connect to the apiserver and which zone to own.
#[derive(Debug, Clone)]
pub struct K8sConfig {
    /// Zone this source is authoritative for (default `cluster.local`).
    pub cluster_domain: String,
    /// TTL applied to generated records.
    pub default_ttl: u32,
    /// Whether to also manage reverse (PTR) records for service/pod IPs.
    pub manage_ptr: bool,
    /// Where endpoint state is read from.
    pub endpoint_source: EndpointSource,
    /// Cluster IP(s) of the in-cluster DNS service. When set, the source also
    /// publishes apex `NS` + `ns.dns` records (otherwise it would be a lame
    /// delegation). Usually the ClusterIP of the `kube-dns`/microdns service.
    pub dns_service_ips: Vec<IpAddr>,
    /// Explicit kubeconfig path. When `None`, connection is inferred from the
    /// environment (in-cluster service account, or the default kubeconfig).
    pub kubeconfig: Option<PathBuf>,
    /// Coalesce a burst of watch events into a single reconcile after this many
    /// milliseconds of quiet.
    pub debounce_ms: u64,
}

impl Default for K8sConfig {
    fn default() -> Self {
        Self {
            cluster_domain: "cluster.local".to_string(),
            default_ttl: 30,
            manage_ptr: true,
            endpoint_source: EndpointSource::Auto,
            dns_service_ips: Vec::new(),
            kubeconfig: None,
            debounce_ms: 500,
        }
    }
}

/// Detect whether we are running inside a Kubernetes pod.
///
/// Uses the same signals kube-rs's in-cluster config keys off: the
/// `KUBERNETES_SERVICE_HOST` env var kubelet injects into every pod, and the
/// mounted service-account token. Either being present means we're in-cluster.
pub fn in_cluster() -> bool {
    if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
        return true;
    }
    std::path::Path::new("/var/run/secrets/kubernetes.io/serviceaccount/token").exists()
}
