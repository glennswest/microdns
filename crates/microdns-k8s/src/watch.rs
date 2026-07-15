//! Watch the apiserver and translate the current cluster state into the desired
//! record set. Reads from kube-rs reflector stores, which handle list+watch,
//! `resourceVersion` tracking and 410-Gone relists.
//!
//! Endpoints come from **EndpointSlices** (`discovery.k8s.io/v1`, the modern
//! source of truth used by K8s ≥1.19 and OpenShift 4), aggregated across the
//! multiple slices that back a single service, with a fallback to the legacy
//! core/v1 Endpoints object for older clusters.

use std::collections::HashMap;
use std::net::IpAddr;

use k8s_openapi::api::core::v1::{Endpoints, Pod, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::runtime::reflector::Store;
use kube::ResourceExt;

use crate::config::EndpointSource;
use crate::translate::{
    meta_records, pod_records, service_records, AddrSnap, DesiredRecord, EndpointsSnap, PodSnap,
    PortSnap, ServiceKind, ServiceSnap,
};

/// Label EndpointSlices carry to point back at their owning Service.
const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";

type NsName = (String, String);

/// Read-only handles onto the reflector-maintained cluster state.
#[derive(Clone)]
pub struct Stores {
    pub services: Store<Service>,
    pub endpoints: Store<Endpoints>,
    pub slices: Store<EndpointSlice>,
    pub pods: Store<Pod>,
}

impl Stores {
    /// Compute the full desired record set from the current cluster state.
    pub fn desired(
        &self,
        cluster_domain: &str,
        ttl: u32,
        endpoint_source: EndpointSource,
        dns_service_ips: &[IpAddr],
    ) -> Vec<DesiredRecord> {
        let ep_by_key = self.aggregate_endpoints(endpoint_source);

        let mut out = Vec::new();
        for svc in self.services.state() {
            if let Some(snap) = to_service_snap(&svc) {
                let ep = ep_by_key.get(&(snap.namespace.clone(), snap.name.clone()));
                out.extend(service_records(&snap, ep, ttl, cluster_domain));
            }
        }
        for pod in self.pods.state() {
            if let Some(snap) = to_pod_snap(&pod) {
                out.extend(pod_records(&snap, ttl));
            }
        }
        out.extend(meta_records(dns_service_ips, ttl));
        out
    }

    /// Merge endpoint state into one snapshot per service, from EndpointSlices
    /// and/or the legacy Endpoints object per the configured source.
    fn aggregate_endpoints(&self, source: EndpointSource) -> HashMap<NsName, EndpointsSnap> {
        let mut map: HashMap<NsName, EndpointsSnap> = HashMap::new();

        if source != EndpointSource::Endpoints {
            for slice in self.slices.state() {
                let Some(key) = slice_service_key(&slice) else {
                    continue;
                };
                let entry = map.entry(key).or_default();
                merge_slice(entry, &slice);
            }
        }

        if source != EndpointSource::Slices {
            for ep in self.endpoints.state() {
                let Some(name) = ep.metadata.name.clone() else {
                    continue;
                };
                let Some(ns) = ep.namespace() else { continue };
                let key = (ns, name);
                // In Auto mode, slices win: only fall back where no slice exists.
                if source == EndpointSource::Auto && map.contains_key(&key) {
                    continue;
                }
                let entry = map.entry(key).or_default();
                merge_core_endpoints(entry, &ep);
            }
        }

        map
    }
}

/// Convert a Service, returning `None` only for services that produce no records
/// (a missing/malformed cluster IP that isn't headless or ExternalName).
fn to_service_snap(svc: &Service) -> Option<ServiceSnap> {
    let name = svc.metadata.name.clone()?;
    let namespace = svc.namespace()?;
    let spec = svc.spec.as_ref()?;

    let kind = if spec.type_.as_deref() == Some("ExternalName") {
        ServiceKind::ExternalName(spec.external_name.clone()?)
    } else if spec.cluster_ip.as_deref() == Some("None") {
        ServiceKind::Headless
    } else {
        // Dual-stack: prefer clusterIPs, fall back to the single clusterIP.
        let raw = spec
            .cluster_ips
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| spec.cluster_ip.clone().map(|s| vec![s]))
            .unwrap_or_default();
        let ips: Vec<IpAddr> = raw.iter().filter_map(|s| s.parse().ok()).collect();
        if ips.is_empty() {
            return None;
        }
        ServiceKind::ClusterIp(ips)
    };

    Some(ServiceSnap {
        name,
        namespace,
        kind,
        ports: service_ports(spec.ports.as_deref()),
        publish_not_ready: spec.publish_not_ready_addresses.unwrap_or(false),
    })
}

fn service_ports(ports: Option<&[k8s_openapi::api::core::v1::ServicePort]>) -> Vec<PortSnap> {
    ports
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            Some(PortSnap {
                name: p.name.clone(),
                port: u16::try_from(p.port).ok()?,
                protocol: proto(p.protocol.as_deref()),
            })
        })
        .collect()
}

fn slice_service_key(slice: &EndpointSlice) -> Option<NsName> {
    let ns = slice.namespace()?;
    let name = slice
        .labels()
        .get(SERVICE_NAME_LABEL)
        .filter(|s| !s.is_empty())?
        .clone();
    Some((ns, name))
}

/// Fold one EndpointSlice's addresses and ports into the service's snapshot.
fn merge_slice(snap: &mut EndpointsSnap, slice: &EndpointSlice) {
    // Only IP address types carry A/AAAA-relevant data (skip FQDN slices).
    let is_ip = matches!(slice.address_type.as_str(), "IPv4" | "IPv6");
    if is_ip {
        for ep in &slice.endpoints {
            // `ready` defaults to true when the condition is unset.
            let ready = ep.conditions.as_ref().and_then(|c| c.ready).unwrap_or(true);
            for addr in &ep.addresses {
                if let Ok(ip) = addr.parse::<IpAddr>() {
                    snap.addresses.push(AddrSnap {
                        ip,
                        hostname: ep.hostname.clone(),
                        ready,
                    });
                }
            }
        }
    }
    for p in slice.ports.as_deref().unwrap_or(&[]) {
        if let Some(port) = p.port.and_then(|v| u16::try_from(v).ok()) {
            push_unique_port(
                snap,
                PortSnap {
                    name: p.name.clone(),
                    port,
                    protocol: proto(p.protocol.as_deref()),
                },
            );
        }
    }
}

/// Fold a legacy core/v1 Endpoints object into the service's snapshot.
fn merge_core_endpoints(snap: &mut EndpointsSnap, ep: &Endpoints) {
    for subset in ep.subsets.as_deref().unwrap_or(&[]) {
        let mut add = |addrs: Option<&Vec<k8s_openapi::api::core::v1::EndpointAddress>>, ready| {
            for a in addrs.map(|v| v.as_slice()).unwrap_or(&[]) {
                if let Ok(ip) = a.ip.parse::<IpAddr>() {
                    snap.addresses.push(AddrSnap {
                        ip,
                        hostname: a.hostname.clone(),
                        ready,
                    });
                }
            }
        };
        add(subset.addresses.as_ref(), true);
        add(subset.not_ready_addresses.as_ref(), false);

        for p in subset.ports.as_deref().unwrap_or(&[]) {
            if let Ok(port) = u16::try_from(p.port) {
                push_unique_port(
                    snap,
                    PortSnap {
                        name: p.name.clone(),
                        port,
                        protocol: proto(p.protocol.as_deref()),
                    },
                );
            }
        }
    }
}

fn push_unique_port(snap: &mut EndpointsSnap, port: PortSnap) {
    if !snap
        .ports
        .iter()
        .any(|p| p.name == port.name && p.port == port.port && p.protocol == port.protocol)
    {
        snap.ports.push(port);
    }
}

fn proto(p: Option<&str>) -> String {
    p.unwrap_or("TCP").to_lowercase()
}

fn to_pod_snap(pod: &Pod) -> Option<PodSnap> {
    let namespace = pod.namespace()?;
    let status = pod.status.as_ref()?;
    // host-network pods share the node IP; skip them to avoid PTR collisions.
    let host_network = pod
        .spec
        .as_ref()
        .and_then(|s| s.host_network)
        .unwrap_or(false);
    if host_network {
        return None;
    }
    let ip = status.pod_ip.as_deref()?.parse::<IpAddr>().ok()?;
    Some(PodSnap { namespace, ip })
}
