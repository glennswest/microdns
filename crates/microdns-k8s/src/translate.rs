//! Pure translation from Kubernetes objects to DNS records, implementing the
//! Kubernetes DNS-Based Service Discovery spec (the same records CoreDNS and the
//! OpenShift DNS operator serve for the cluster zone).
//!
//! This module is deliberately free of any I/O (no DB, no apiserver): it takes
//! plain snapshot structs and produces zone-relative [`DesiredRecord`]s. That
//! keeps the interesting logic unit-testable without a cluster or a database.
//!
//! Coverage:
//! - ClusterIP services (dual-stack: an `A` and/or `AAAA` per cluster IP)
//! - Headless services (`A`/`AAAA` per ready endpoint, plus per-endpoint
//!   `<hostname|ip-dashed>.<svc>.<ns>.svc` names, honouring
//!   `publishNotReadyAddresses`)
//! - `ExternalName` services (`CNAME`)
//! - `SRV` for named ports — one per endpoint for headless services, one to the
//!   service name for ClusterIP services
//! - Pod records (`<ip-dashed>.<ns>.pod`)
//! - Zone apex / meta records (`NS`, `ns.dns`, `dns-version` TXT)
//!
//! Reverse (PTR) records are derived by the reconciler from the forward A/AAAA
//! records via `microdns_core::reverse`, so the reverse zones stay consistent
//! from a single source of truth.

use std::net::IpAddr;

use microdns_core::types::{RecordData, SrvData};

/// DNS schema version advertised at `dns-version.<zone>` (matches the value the
/// Kubernetes DNS spec / CoreDNS publish).
pub const DNS_SCHEMA_VERSION: &str = "1.1.0";

/// A record we want to exist in the managed cluster zone, named relative to the
/// zone origin (e.g. `kubernetes.default.svc`, or `@` for the apex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredRecord {
    /// Name relative to the cluster zone origin; `@` for the apex.
    pub name: String,
    pub ttl: u32,
    pub data: RecordData,
}

/// What kind of Service this is, from a DNS perspective.
#[derive(Debug, Clone)]
pub enum ServiceKind {
    /// Normal service with one or more cluster IPs (dual-stack → 2 entries).
    ClusterIp(Vec<IpAddr>),
    /// `clusterIP: None` — records come from the backing endpoints.
    Headless,
    /// `type: ExternalName` — a CNAME to an external name.
    ExternalName(String),
}

/// A Kubernetes Service, reduced to what DNS cares about.
#[derive(Debug, Clone)]
pub struct ServiceSnap {
    pub name: String,
    pub namespace: String,
    pub kind: ServiceKind,
    pub ports: Vec<PortSnap>,
    /// `spec.publishNotReadyAddresses` — publish endpoints even when not ready.
    pub publish_not_ready: bool,
}

#[derive(Debug, Clone)]
pub struct PortSnap {
    /// Only *named* ports get an SRV record, per the K8s DNS spec.
    pub name: Option<String>,
    pub port: u16,
    /// Lower-cased transport, e.g. `tcp` / `udp`.
    pub protocol: String,
}

/// The endpoints backing a service, aggregated across all EndpointSlices (or the
/// legacy Endpoints object).
#[derive(Debug, Clone, Default)]
pub struct EndpointsSnap {
    pub addresses: Vec<AddrSnap>,
    /// Endpoints whose address is a DNS name (EndpointSlice `addressType: FQDN`),
    /// rather than an IP — served as CNAMEs.
    pub fqdns: Vec<FqdnEndpointSnap>,
    /// Endpoint ports (used for headless SRV; falls back to service ports).
    pub ports: Vec<PortSnap>,
}

#[derive(Debug, Clone)]
pub struct FqdnEndpointSnap {
    pub hostname: Option<String>,
    pub fqdn: String,
    pub ready: bool,
}

#[derive(Debug, Clone)]
pub struct AddrSnap {
    pub ip: IpAddr,
    /// Optional per-endpoint hostname (StatefulSet pods / `hostname` set it).
    pub hostname: Option<String>,
    /// Whether the endpoint is ready to serve.
    pub ready: bool,
}

/// A pod that should get an `A`/`AAAA` record in the `pod` subdomain.
#[derive(Debug, Clone)]
pub struct PodSnap {
    pub namespace: String,
    pub ip: IpAddr,
}

fn a_or_aaaa(ip: IpAddr) -> RecordData {
    match ip {
        IpAddr::V4(v4) => RecordData::A(v4),
        IpAddr::V6(v6) => RecordData::AAAA(v6),
    }
}

/// The dash-joined form of an IP used in K8s DNS names, matching upstream
/// CoreDNS exactly: a naive replacement of `.`/`:` with `-` on the canonical
/// address string. IPv4 `1.2.3.4` → `1-2-3-4`; IPv6 uses the compressed form,
/// so `2001:db8::1` → `2001-db8--1` (same as CoreDNS `strings.Replace`).
pub fn dashed_ip(ip: IpAddr) -> String {
    ip.to_string().replace(['.', ':'], "-")
}

/// Absolute FQDN (trailing dot) for a zone-relative name, matching the trailing
/// dot convention used elsewhere in microdns-core (see `reverse::ptr_target`).
fn fqdn(rel: &str, cluster_domain: &str) -> String {
    format!("{}.{}.", rel, cluster_domain.trim_end_matches('.'))
}

/// Ensure a name is an absolute FQDN (single trailing dot).
fn absolute(name: &str) -> String {
    format!("{}.", name.trim_end_matches('.'))
}

/// Records for a single service. `endpoints` is only consulted for headless
/// services (where the A/AAAA records come from the backing endpoint IPs).
pub fn service_records(
    svc: &ServiceSnap,
    endpoints: Option<&EndpointsSnap>,
    ttl: u32,
    cluster_domain: &str,
) -> Vec<DesiredRecord> {
    let mut out = Vec::new();
    // e.g. "kubernetes.default.svc"
    let base = format!("{}.{}.svc", svc.name, svc.namespace);

    match &svc.kind {
        ServiceKind::ExternalName(target) => {
            // CNAME the service name to the external target; no SRV/A.
            out.push(DesiredRecord {
                name: base,
                ttl,
                data: RecordData::CNAME(absolute(target)),
            });
            return out;
        }
        ServiceKind::ClusterIp(ips) => {
            for ip in ips {
                out.push(DesiredRecord {
                    name: base.clone(),
                    ttl,
                    data: a_or_aaaa(*ip),
                });
            }
            // SRV for named service ports → the service name.
            let target = fqdn(&base, cluster_domain);
            for port in &svc.ports {
                let Some(pname) = &port.name else { continue };
                out.push(srv(&base, pname, &port.protocol, port.port, &target, ttl));
            }
        }
        ServiceKind::Headless => {
            let Some(ep) = endpoints else { return out };
            let include = |a: &AddrSnap| a.ready || svc.publish_not_ready;

            // FQDN-typed endpoints are served as CNAMEs. They can only be named
            // via their per-endpoint hostname — a CNAME at the service apex would
            // collide across endpoints — so endpoints without a hostname are
            // skipped (there is no valid single-owner name for them).
            for fe in ep
                .fqdns
                .iter()
                .filter(|f| f.ready || svc.publish_not_ready)
            {
                if let Some(host) = &fe.hostname {
                    out.push(DesiredRecord {
                        name: format!("{host}.{base}"),
                        ttl,
                        data: RecordData::CNAME(absolute(&fe.fqdn)),
                    });
                }
            }

            for addr in ep.addresses.iter().filter(|a| include(a)) {
                // Set of A/AAAA at the service name.
                out.push(DesiredRecord {
                    name: base.clone(),
                    ttl,
                    data: a_or_aaaa(addr.ip),
                });
                // Per-endpoint stable name: hostname if present else dashed IP.
                let label = endpoint_label(addr);
                out.push(DesiredRecord {
                    name: format!("{label}.{base}"),
                    ttl,
                    data: a_or_aaaa(addr.ip),
                });
            }

            // SRV per named port per included endpoint → the endpoint's name.
            // Prefer endpoint ports; fall back to the service's named ports.
            let srv_ports = if ep.ports.iter().any(|p| p.name.is_some()) {
                &ep.ports
            } else {
                &svc.ports
            };
            for addr in ep.addresses.iter().filter(|a| include(a)) {
                let label = endpoint_label(addr);
                let target = fqdn(&format!("{label}.{base}"), cluster_domain);
                for port in srv_ports {
                    let Some(pname) = &port.name else { continue };
                    out.push(srv(&base, pname, &port.protocol, port.port, &target, ttl));
                }
            }
        }
    }

    out
}

fn endpoint_label(addr: &AddrSnap) -> String {
    addr.hostname
        .clone()
        .unwrap_or_else(|| dashed_ip(addr.ip))
}

fn srv(
    base: &str,
    port_name: &str,
    protocol: &str,
    port: u16,
    target: &str,
    ttl: u32,
) -> DesiredRecord {
    DesiredRecord {
        name: format!("_{port_name}._{protocol}.{base}"),
        ttl,
        data: RecordData::SRV(SrvData {
            priority: 0,
            weight: 100,
            port,
            target: target.to_string(),
        }),
    }
}

/// The `A`/`AAAA` record for a pod: `<ip-dashed>.<ns>.pod`.
pub fn pod_records(pod: &PodSnap, ttl: u32) -> Vec<DesiredRecord> {
    vec![DesiredRecord {
        name: format!("{}.{}.pod", dashed_ip(pod.ip), pod.namespace),
        ttl,
        data: a_or_aaaa(pod.ip),
    }]
}

/// Zone apex / meta records that CoreDNS also serves:
/// - `dns-version.<zone>` TXT → schema version
/// - when the in-cluster DNS service IP is known: `ns.dns.<zone>` A/AAAA and an
///   apex `NS` delegating to it (avoids a lame delegation when it is not known).
pub fn meta_records(dns_service_ips: &[IpAddr], ttl: u32) -> Vec<DesiredRecord> {
    let mut out = vec![DesiredRecord {
        name: "dns-version".to_string(),
        ttl,
        data: RecordData::TXT(DNS_SCHEMA_VERSION.to_string()),
    }];

    if !dns_service_ips.is_empty() {
        out.push(DesiredRecord {
            name: "@".to_string(),
            ttl,
            data: RecordData::NS("ns.dns".to_string()),
        });
        for ip in dns_service_ips {
            out.push(DesiredRecord {
                name: "ns.dns".to_string(),
                ttl,
                data: a_or_aaaa(*ip),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse::<Ipv4Addr>().unwrap())
    }
    fn ready(ip_s: &str, host: Option<&str>) -> AddrSnap {
        AddrSnap {
            ip: ip(ip_s),
            hostname: host.map(String::from),
            ready: true,
        }
    }

    #[test]
    fn clusterip_service_gets_a_and_srv() {
        let svc = ServiceSnap {
            name: "kubernetes".into(),
            namespace: "default".into(),
            kind: ServiceKind::ClusterIp(vec![v4("10.96.0.1")]),
            ports: vec![PortSnap {
                name: Some("https".into()),
                port: 443,
                protocol: "tcp".into(),
            }],
            publish_not_ready: false,
        };
        let recs = service_records(&svc, None, 30, "cluster.local");

        assert!(recs.contains(&DesiredRecord {
            name: "kubernetes.default.svc".into(),
            ttl: 30,
            data: RecordData::A("10.96.0.1".parse().unwrap()),
        }));
        assert!(recs.contains(&DesiredRecord {
            name: "_https._tcp.kubernetes.default.svc".into(),
            ttl: 30,
            data: RecordData::SRV(SrvData {
                priority: 0,
                weight: 100,
                port: 443,
                target: "kubernetes.default.svc.cluster.local.".into(),
            }),
        }));
    }

    #[test]
    fn dual_stack_emits_a_and_aaaa() {
        let svc = ServiceSnap {
            name: "svc".into(),
            namespace: "ns".into(),
            kind: ServiceKind::ClusterIp(vec![v4("10.0.0.5"), ip("fd00::5")]),
            ports: vec![],
            publish_not_ready: false,
        };
        let recs = service_records(&svc, None, 30, "cluster.local");
        assert!(recs
            .iter()
            .any(|r| matches!(r.data, RecordData::A(_)) && r.name == "svc.ns.svc"));
        assert!(recs
            .iter()
            .any(|r| matches!(r.data, RecordData::AAAA(_)) && r.name == "svc.ns.svc"));
    }

    #[test]
    fn externalname_is_a_cname() {
        let svc = ServiceSnap {
            name: "db".into(),
            namespace: "prod".into(),
            kind: ServiceKind::ExternalName("rds.example.com".into()),
            ports: vec![],
            publish_not_ready: false,
        };
        let recs = service_records(&svc, None, 30, "cluster.local");
        assert_eq!(
            recs,
            vec![DesiredRecord {
                name: "db.prod.svc".into(),
                ttl: 30,
                data: RecordData::CNAME("rds.example.com.".into()),
            }]
        );
    }

    #[test]
    fn unnamed_port_produces_no_srv() {
        let svc = ServiceSnap {
            name: "svc".into(),
            namespace: "ns".into(),
            kind: ServiceKind::ClusterIp(vec![v4("10.0.0.5")]),
            ports: vec![PortSnap {
                name: None,
                port: 8080,
                protocol: "tcp".into(),
            }],
            publish_not_ready: false,
        };
        let recs = service_records(&svc, None, 30, "cluster.local");
        assert!(recs.iter().all(|r| !matches!(r.data, RecordData::SRV(_))));
        assert_eq!(recs.len(), 1); // just the A record
    }

    #[test]
    fn headless_expands_endpoints_with_dashed_and_hostnames() {
        let svc = ServiceSnap {
            name: "web".into(),
            namespace: "prod".into(),
            kind: ServiceKind::Headless,
            ports: vec![PortSnap {
                name: Some("http".into()),
                port: 80,
                protocol: "tcp".into(),
            }],
            publish_not_ready: false,
        };
        let ep = EndpointsSnap {
            addresses: vec![ready("10.1.1.1", Some("web-0")), ready("10.1.1.2", None)],
            fqdns: vec![],
            ports: vec![],
        };
        let recs = service_records(&svc, Some(&ep), 30, "cluster.local");

        // Both IPs answer the service name.
        assert_eq!(recs.iter().filter(|r| r.name == "web.prod.svc").count(), 2);
        // Named endpoint gets its stable record.
        assert!(recs.iter().any(|r| r.name == "web-0.web.prod.svc"
            && r.data == RecordData::A("10.1.1.1".parse().unwrap())));
        // Unnamed endpoint gets a dashed-IP record.
        assert!(recs.iter().any(|r| r.name == "10-1-1-2.web.prod.svc"
            && r.data == RecordData::A("10.1.1.2".parse().unwrap())));
        // Per-endpoint SRV to the endpoint name (falls back to service port).
        assert!(recs.iter().any(|r| r.name == "_http._tcp.web.prod.svc"
            && matches!(&r.data, RecordData::SRV(s) if s.target == "web-0.web.prod.svc.cluster.local." && s.port == 80)));
    }

    #[test]
    fn not_ready_excluded_unless_publish_not_ready() {
        let mk = |publish| ServiceSnap {
            name: "web".into(),
            namespace: "prod".into(),
            kind: ServiceKind::Headless,
            ports: vec![],
            publish_not_ready: publish,
        };
        let ep = EndpointsSnap {
            addresses: vec![AddrSnap {
                ip: v4("10.1.1.9"),
                hostname: None,
                ready: false,
            }],
            fqdns: vec![],
            ports: vec![],
        };
        assert!(service_records(&mk(false), Some(&ep), 30, "cluster.local").is_empty());
        assert!(!service_records(&mk(true), Some(&ep), 30, "cluster.local").is_empty());
    }

    #[test]
    fn headless_fqdn_endpoints_become_cnames() {
        let svc = ServiceSnap {
            name: "ext".into(),
            namespace: "prod".into(),
            kind: ServiceKind::Headless,
            ports: vec![],
            publish_not_ready: false,
        };
        let ep = EndpointsSnap {
            addresses: vec![],
            fqdns: vec![
                FqdnEndpointSnap {
                    hostname: Some("a".into()),
                    fqdn: "svc-a.example.com".into(),
                    ready: true,
                },
                // no hostname → skipped (no valid single-owner name)
                FqdnEndpointSnap {
                    hostname: None,
                    fqdn: "svc-b.example.com".into(),
                    ready: true,
                },
            ],
            ports: vec![],
        };
        let recs = service_records(&svc, Some(&ep), 30, "cluster.local");
        assert_eq!(
            recs,
            vec![DesiredRecord {
                name: "a.ext.prod.svc".into(),
                ttl: 30,
                data: RecordData::CNAME("svc-a.example.com.".into()),
            }]
        );
    }

    #[test]
    fn pod_record_uses_dashed_ip() {
        let recs = pod_records(
            &PodSnap {
                namespace: "default".into(),
                ip: v4("172.17.0.3"),
            },
            30,
        );
        assert_eq!(recs[0].name, "172-17-0-3.default.pod");
        assert_eq!(recs[0].data, RecordData::A("172.17.0.3".parse().unwrap()));
    }

    #[test]
    fn ipv6_pod_and_headless_use_upstream_dashed_form() {
        // Pod AAAA record (compressed, dashed — matches CoreDNS).
        let recs = pod_records(
            &PodSnap {
                namespace: "default".into(),
                ip: ip("2001:db8::1"),
            },
            30,
        );
        assert_eq!(recs[0].name, "2001-db8--1.default.pod");
        assert!(matches!(recs[0].data, RecordData::AAAA(_)));

        // Headless IPv6 endpoint without a hostname → dashed-IP label + AAAA.
        let svc = ServiceSnap {
            name: "web".into(),
            namespace: "prod".into(),
            kind: ServiceKind::Headless,
            ports: vec![],
            publish_not_ready: false,
        };
        let ep = EndpointsSnap {
            addresses: vec![ready("fd00::5", None)],
            fqdns: vec![],
            ports: vec![],
        };
        let recs = service_records(&svc, Some(&ep), 30, "cluster.local");
        assert!(recs
            .iter()
            .any(|r| r.name == "fd00--5.web.prod.svc" && matches!(r.data, RecordData::AAAA(_))));
        assert!(recs
            .iter()
            .any(|r| r.name == "web.prod.svc" && matches!(r.data, RecordData::AAAA(_))));
    }

    #[test]
    fn meta_records_include_version_and_optional_ns() {
        let none = meta_records(&[], 30);
        assert_eq!(none.len(), 1);
        assert!(matches!(&none[0].data, RecordData::TXT(v) if v == DNS_SCHEMA_VERSION));

        let with_ip = meta_records(&[v4("10.96.0.10")], 30);
        assert!(with_ip
            .iter()
            .any(|r| r.name == "@" && matches!(&r.data, RecordData::NS(t) if t == "ns.dns")));
        assert!(with_ip
            .iter()
            .any(|r| r.name == "ns.dns" && matches!(r.data, RecordData::A(_))));
    }
}
