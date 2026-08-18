use crate::types::InstanceMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub instance: InstanceConfig,
    #[serde(default)]
    pub coordinator: Option<CoordinatorConfig>,
    #[serde(default)]
    pub dns: DnsConfig,
    #[serde(default)]
    pub dhcp: Option<DhcpConfig>,
    #[serde(default)]
    pub messaging: Option<MessagingConfig>,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub ipam: Option<IpamConfig>,
    #[serde(default)]
    pub replication: Option<ReplicationConfig>,
    #[serde(default)]
    pub k8s: Option<K8sSourceConfig>,
    #[serde(default)]
    pub mdns: Option<MdnsSourceConfig>,
}

/// mDNS ingest — listens for `.local` announcements on the local segment and
/// publishes what it hears as authoritative records.
///
/// mDNS queries are sent with an IP TTL of 1 and never cross a router, so a
/// device advertising itself is invisible to clients on any other subnet. This
/// bridges those names into unicast DNS at the instance that already sits on
/// the segment where the announcements are audible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdnsSourceConfig {
    /// Off by default: existing deployments must not start publishing whatever
    /// happens to be shouting on their LAN because they upgraded.
    #[serde(default)]
    pub enabled: bool,
    /// Zone discovered names are published into, e.g. `mdns.g9.lo`. A dedicated
    /// subzone keeps discovered names visibly apart from curated ones.
    pub zone: String,
    /// Floor for the announced TTL. Responses to a legacy query carry a 10 s
    /// TTL (RFC 6762 §6.7); honouring that literally would churn the zone.
    #[serde(default = "default_mdns_ttl_min")]
    pub ttl_min: u32,
    /// Ceiling for the announced TTL, so a device advertising a huge TTL cannot
    /// pin a record that has gone stale.
    #[serde(default = "default_mdns_ttl_max")]
    pub ttl_max: u32,
    /// Also mirror DNS-SD service records (PTR/SRV/TXT), so service discovery
    /// works cross-subnet too — not just plain hostnames.
    #[serde(default = "default_true")]
    pub services: bool,
    /// Glob patterns (`*` wildcard) of names to publish. Empty allows all.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Glob patterns never to publish — the escape hatch for a network full of
    /// printers and Chromecasts. Checked after `allow`, so deny always wins.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Seconds between DNS-SD enumeration queries. 0 makes the source purely
    /// passive, learning only from announcements it happens to overhear.
    #[serde(default = "default_mdns_query_interval")]
    pub query_interval_secs: u64,
    /// Also join the IPv6 group (`ff02::fb`). Off by default: responders that
    /// answer over IPv6 answer over IPv4 too, and the join fails on hosts
    /// without IPv6.
    #[serde(default)]
    pub ipv6: bool,
    /// Address to bind the listener to. `0.0.0.0` listens on every interface.
    #[serde(default = "default_mdns_bind")]
    pub bind: String,
    /// Local interface addresses to join the group on. Empty lets the kernel
    /// choose, which is what a single-interface container wants.
    #[serde(default)]
    pub interfaces: Vec<String>,
    /// Quiet window (seconds) before a burst of announcements is written to
    /// the zone.
    #[serde(default = "default_mdns_debounce")]
    pub debounce_secs: u64,
}

/// Kubernetes DNS source — makes this instance authoritative for a cluster's
/// internal zone (`cluster.local`), populated live from a kube-apiserver.
/// This is the CoreDNS / OpenShift-DNS equivalent for the rustkube control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sSourceConfig {
    /// Whether the source is on. When unset, it auto-detects: enabled iff
    /// running inside a Kubernetes pod (the way upstream in-cluster clients do).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Zone this source owns (default `cluster.local`).
    #[serde(default = "default_cluster_domain")]
    pub cluster_domain: String,
    /// TTL applied to generated records.
    #[serde(default = "default_k8s_ttl")]
    pub ttl: u32,
    /// Also manage reverse (PTR) zones for service/pod IPs.
    #[serde(default = "default_true")]
    pub manage_ptr: bool,
    /// Endpoint source: `auto` (slices, fall back to endpoints), `slices`, or
    /// `endpoints`.
    #[serde(default = "default_endpoint_source")]
    pub endpoint_source: String,
    /// Cluster IP(s) of the in-cluster DNS service. When set, the source also
    /// publishes apex `NS` + `ns.dns` records. Accepts IPv4 and IPv6.
    #[serde(default)]
    pub dns_service_ips: Vec<String>,
    /// Explicit kubeconfig path. When unset, connection is inferred from the
    /// environment (in-cluster service account / default kubeconfig).
    #[serde(default)]
    pub kubeconfig: Option<PathBuf>,
    /// Coalesce a burst of watch events into one reconcile after this quiet
    /// window (milliseconds).
    #[serde(default = "default_k8s_debounce_ms")]
    pub debounce_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceConfig {
    pub id: String,
    #[serde(default)]
    pub mode: InstanceMode,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    pub id: String,
    pub addr: String,
    #[serde(default = "default_peer_dns_port")]
    pub dns_port: u16,
    #[serde(default = "default_peer_http_port")]
    pub http_port: u16,
    #[serde(default = "default_peer_grpc_port")]
    pub grpc_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorConfig {
    pub endpoint: String,
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,
    #[serde(default = "default_report_interval")]
    pub report_interval_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DnsConfig {
    #[serde(default)]
    pub auth: Option<DnsAuthConfig>,
    #[serde(default)]
    pub recursor: Option<DnsRecursorConfig>,
    #[serde(default)]
    pub loadbalancer: Option<DnsLbConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsAuthConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_dns_listen")]
    pub listen: String,
    #[serde(default)]
    pub zones: Vec<String>,
    /// Addresses permitted to request a zone transfer (AXFR), as CIDRs.
    ///
    /// An AXFR hands over a complete map of internal hosts, so this defaults to
    /// private ranges only — enough for a secondary on the same network to
    /// replicate, while refusing anything routed in from outside.
    #[serde(default = "default_allow_transfer")]
    pub allow_transfer: Vec<String>,
    /// Secondaries to send NOTIFY to when a zone changes, as `ip` or `ip:port`.
    ///
    /// Without this a secondary only discovers changes when its SOA refresh
    /// timer fires — an hour on the zones here — so a record added during an
    /// incident would take that long to reach the fallback.
    #[serde(default)]
    pub notify: Vec<String>,
    /// Zones this instance mirrors from a primary over AXFR — the other side of
    /// `notify`. Each entry names the zone and the primary to pull it from.
    #[serde(default)]
    pub secondary: Vec<SecondaryZoneConfig>,
}

/// Zone transfer settings, stored in the database and managed through the API.
///
/// Same reason as the mDNS section: on deployments whose `microdns.toml` is
/// generated for them, a `[dns.auth]` edit does not survive the next
/// regeneration. The `[dns.auth]` fields seed this once, and after that the
/// stored value is what the instance runs on.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneTransferConfig {
    /// CIDRs permitted to request an AXFR. Empty denies every transfer.
    #[serde(default = "default_allow_transfer")]
    pub allow_transfer: Vec<String>,
    /// Secondaries to send NOTIFY to when a zone changes.
    #[serde(default)]
    pub notify: Vec<String>,
    /// Zones mirrored from a primary.
    #[serde(default)]
    pub secondary: Vec<SecondaryZoneConfig>,
}

impl ZoneTransferConfig {
    /// The settings a `[dns.auth]` block implies, used to seed the stored value.
    pub fn from_auth(auth: &DnsAuthConfig) -> Self {
        Self {
            allow_transfer: auth.allow_transfer.clone(),
            notify: auth.notify.clone(),
            secondary: auth.secondary.clone(),
        }
    }
}

/// One zone mirrored from a primary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecondaryZoneConfig {
    /// Zone to mirror, e.g. `gw.lo`.
    pub zone: String,
    /// Primary to transfer from, as `ip` or `ip:port` (port defaults to 53).
    /// Only this address is believed when a NOTIFY for the zone arrives.
    pub primary: String,
    /// Fallback poll interval. NOTIFY is a hint, not a delivery guarantee
    /// (RFC 1996 §4), so the timer is what catches a lost one.
    #[serde(default = "default_secondary_refresh")]
    pub refresh_secs: u64,
}

fn default_allow_transfer() -> Vec<String> {
    vec![
        "10.0.0.0/8".to_string(),
        "172.16.0.0/12".to_string(),
        "192.168.0.0/16".to_string(),
        "127.0.0.0/8".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsRecursorConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_recursor_listen")]
    pub listen: String,
    #[serde(default)]
    pub forward_zones: HashMap<String, Vec<String>>,
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsLbConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u64,
    #[serde(default = "default_probe_type")]
    pub default_probe: String,
    /// Max in-flight probes per cycle. Caps fan-out for hosts with many
    /// monitored records.
    #[serde(default = "default_probe_concurrency")]
    pub probe_concurrency: usize,
    /// Number of ICMP echo packets per ping probe. Probe is healthy if
    /// any echo replies. Matches ploadb's default of 3.
    #[serde(default = "default_ping_packet_count")]
    pub ping_packet_count: u8,
    /// Default per-probe timeout (seconds) when a HealthCheck doesn't set
    /// one explicitly.
    #[serde(default = "default_probe_timeout")]
    pub default_timeout_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpConfig {
    #[serde(default)]
    pub v4: Option<DhcpV4Config>,
    #[serde(default)]
    pub v6: Option<DhcpV6Config>,
    #[serde(default)]
    pub slaac: Option<SlaacConfig>,
    #[serde(default)]
    pub dns_registration: Option<DnsRegistrationConfig>,
}

/// DHCP operating mode.
///
/// - `normal`  — accept direct broadcast packets from clients on the local
///               subnet (standard DHCP).  No deadman timer.
/// - `gateway` — only accept relay-forwarded packets (giaddr != 0).  Includes
///               the veth deadman timer that works around RouterOS container
///               networking bugs.  Use this when microdns runs inside a
///               MikroTik / Rose appliance container behind a DHCP relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DhcpMode {
    Normal,
    Gateway,
}

impl Default for DhcpMode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpV4Config {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub interface: String,
    /// Operating mode: "normal" (direct broadcast) or "gateway" (relay-only).
    /// Defaults to "normal".
    #[serde(default)]
    pub mode: DhcpMode,
    /// The DHCP server's own IP address, used for siaddr and option 54
    /// (server identifier). If not set, falls back to the first pool's gateway.
    #[serde(default)]
    pub server_ip: Option<String>,
    #[serde(default)]
    pub pools: Vec<DhcpV4Pool>,
    #[serde(default)]
    pub reservations: Vec<DhcpReservation>,
    #[serde(default = "default_dhcp_ports")]
    pub listen_ports: Vec<u16>,
}

fn default_dhcp_ports() -> Vec<u16> {
    vec![67]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpV4Pool {
    pub range_start: String,
    pub range_end: String,
    pub subnet: String,
    pub gateway: String,
    pub dns: Vec<String>,
    pub domain: String,
    #[serde(default = "default_lease_time")]
    pub lease_time_secs: u64,
    #[serde(default)]
    pub next_server: Option<String>,
    #[serde(default)]
    pub boot_file: Option<String>,
    /// EFI boot file for UEFI clients (served when DHCP option 93 indicates EFI)
    #[serde(default)]
    pub boot_file_efi: Option<String>,
    #[serde(default)]
    pub ipxe_boot_url: Option<String>,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub domain_search: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpReservation {
    pub mac: String,
    pub ip: String,
    #[serde(default)]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpV6Config {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub interface: String,
    pub pools: Vec<DhcpV6Pool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpV6Pool {
    pub prefix: String,
    pub prefix_len: u8,
    pub dns: Vec<String>,
    pub domain: String,
    #[serde(default = "default_lease_time")]
    pub lease_time_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaacConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub interface: String,
    pub prefix: String,
    pub prefix_len: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsRegistrationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub forward_zone: String,
    pub reverse_zone_v4: String,
    pub reverse_zone_v6: String,
    #[serde(default = "default_ttl")]
    pub default_ttl: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagingConfig {
    #[serde(default = "default_messaging_backend")]
    pub backend: String,
    #[serde(default)]
    pub brokers: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_topic_prefix")]
    pub topic_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default)]
    pub rest: Option<RestApiConfig>,
    #[serde(default)]
    pub grpc: Option<GrpcApiConfig>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            rest: Some(RestApiConfig::default()),
            grpc: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestApiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_rest_listen")]
    pub listen: String,
    #[serde(default = "default_dashboard_listen")]
    pub dashboard_listen: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

impl Default for RestApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: default_rest_listen(),
            dashboard_listen: default_dashboard_listen(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_grpc_listen")]
    pub listen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpamConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub pools: Vec<IpamPool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpamPool {
    pub name: String,
    pub subnet: String,
    pub range_start: String,
    pub range_end: String,
    pub gateway: String,
    pub bridge: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_pull_interval")]
    pub pull_interval_secs: u64,
    #[serde(default = "default_stale_threshold")]
    pub stale_threshold_secs: u64,
    #[serde(default = "default_peer_timeout")]
    pub peer_timeout_secs: u64,
}

// Default value functions
fn default_true() -> bool {
    true
}
fn default_dns_listen() -> String {
    "0.0.0.0:53".to_string()
}
fn default_recursor_listen() -> String {
    "0.0.0.0:5353".to_string()
}
fn default_rest_listen() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_dashboard_listen() -> String {
    "0.0.0.0:80".to_string()
}
fn default_grpc_listen() -> String {
    "0.0.0.0:50051".to_string()
}
fn default_db_path() -> PathBuf {
    PathBuf::from("/data/microdns.redb")
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_format() -> String {
    "json".to_string()
}
fn default_cache_size() -> usize {
    10000
}
fn default_cluster_domain() -> String {
    "cluster.local".to_string()
}
fn default_k8s_ttl() -> u32 {
    30
}
fn default_endpoint_source() -> String {
    "auto".to_string()
}
fn default_k8s_debounce_ms() -> u64 {
    500
}
fn default_check_interval() -> u64 {
    10
}
fn default_probe_type() -> String {
    "ping".to_string()
}
fn default_probe_concurrency() -> usize {
    32
}
fn default_ping_packet_count() -> u8 {
    3
}
fn default_probe_timeout() -> u32 {
    5
}
fn default_lease_time() -> u64 {
    3600
}
fn default_ttl() -> u32 {
    300
}
fn default_heartbeat_interval() -> u64 {
    10
}
fn default_report_interval() -> u64 {
    30
}
fn default_messaging_backend() -> String {
    "noop".to_string()
}
fn default_peer_dns_port() -> u16 {
    53
}
fn default_peer_http_port() -> u16 {
    8080
}
fn default_peer_grpc_port() -> u16 {
    50051
}
fn default_pull_interval() -> u64 {
    60
}
fn default_stale_threshold() -> u64 {
    300
}
fn default_peer_timeout() -> u64 {
    10
}
fn default_topic_prefix() -> String {
    "microdns".to_string()
}
fn default_secondary_refresh() -> u64 {
    900
}
fn default_mdns_ttl_min() -> u32 {
    60
}
fn default_mdns_ttl_max() -> u32 {
    1200
}
fn default_mdns_query_interval() -> u64 {
    300
}
fn default_mdns_bind() -> String {
    "0.0.0.0".to_string()
}
fn default_mdns_debounce() -> u64 {
    5
}

impl Default for Config {
    fn default() -> Self {
        Self {
            instance: InstanceConfig::default(),
            coordinator: None,
            dns: DnsConfig::default(),
            dhcp: None,
            messaging: None,
            api: ApiConfig::default(),
            database: DatabaseConfig::default(),
            logging: LoggingConfig::default(),
            ipam: None,
            replication: None,
            k8s: None,
            mdns: None,
        }
    }
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            id: "microdns".to_string(),
            mode: InstanceMode::Standalone,
            peers: Vec::new(),
        }
    }
}

impl Config {
    pub fn from_file(path: &std::path::Path) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::Error::Config(format!("failed to read config: {e}")))?;
        let config: Config = toml::from_str(&content)
            .map_err(|e| crate::error::Error::Config(format!("failed to parse config: {e}")))?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml_str = r#"
[instance]
id = "test-01"
mode = "standalone"

[database]
path = "/tmp/test.redb"

[logging]
level = "debug"
format = "text"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.instance.id, "test-01");
        assert_eq!(config.instance.mode, InstanceMode::Standalone);
        assert_eq!(config.database.path, PathBuf::from("/tmp/test.redb"));
    }

    #[test]
    fn test_parse_full_config() {
        let toml_str = r#"
[instance]
id = "vlan10-dns01"
mode = "leaf"

[coordinator]
endpoint = "grpc://coordinator.microdns.svc:50051"
heartbeat_interval_secs = 10
report_interval_secs = 30

[dns.auth]
enabled = true
listen = "0.0.0.0:53"
zones = ["example.com", "10.in-addr.arpa"]

[dns.recursor]
enabled = true
listen = "0.0.0.0:5353"
cache_size = 10000

[dns.recursor.forward_zones]
"corp.local" = ["10.0.1.1:53"]

[dns.loadbalancer]
enabled = true
check_interval_secs = 10
default_probe = "ping"
probe_concurrency = 32
ping_packet_count = 3
default_timeout_secs = 5

[api.rest]
enabled = true
listen = "0.0.0.0:8080"
dashboard_listen = "0.0.0.0:80"
api_key = "secret"

[api.grpc]
enabled = true
listen = "0.0.0.0:50051"

[database]
path = "/data/microdns.redb"

[logging]
level = "info"
format = "json"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.instance.mode, InstanceMode::Leaf);
        let auth = config.dns.auth.unwrap();
        assert_eq!(auth.zones.len(), 2);
        let recursor = config.dns.recursor.unwrap();
        assert!(recursor.forward_zones.contains_key("corp.local"));
    }

    #[test]
    fn test_parse_peers_config() {
        let toml_str = r#"
[instance]
id = "test-main"
mode = "standalone"

[[instance.peers]]
id = "test-g10"
addr = "192.168.10.199"

[[instance.peers]]
id = "test-g11"
addr = "192.168.11.199"
dns_port = 5353
http_port = 9090

[database]
path = "/tmp/test.redb"

[logging]
level = "debug"
format = "text"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.instance.peers.len(), 2);
        assert_eq!(config.instance.peers[0].id, "test-g10");
        assert_eq!(config.instance.peers[0].addr, "192.168.10.199");
        assert_eq!(config.instance.peers[0].dns_port, 53); // default
        assert_eq!(config.instance.peers[0].http_port, 8080); // default
        assert_eq!(config.instance.peers[1].dns_port, 5353); // custom
        assert_eq!(config.instance.peers[1].http_port, 9090); // custom
    }

    #[test]
    fn test_parse_no_peers() {
        let toml_str = r#"
[instance]
id = "test-01"
mode = "standalone"

[database]
path = "/tmp/test.redb"

[logging]
level = "debug"
format = "text"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.instance.peers.is_empty());
    }

    #[test]
    fn test_parse_mdns_source() {
        let toml_str = r#"
[instance]
id = "test-mdns"
mode = "standalone"

[mdns]
enabled = true
zone = "mdns.g9.lo"
deny = ["chromecast-*"]

[database]
path = "/tmp/test.redb"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let mdns = config.mdns.unwrap();
        assert!(mdns.enabled);
        assert_eq!(mdns.zone, "mdns.g9.lo");
        assert_eq!(mdns.deny, vec!["chromecast-*".to_string()]);
        // Everything else falls back to defaults.
        assert_eq!(mdns.ttl_min, 60);
        assert_eq!(mdns.ttl_max, 1200);
        assert_eq!(mdns.query_interval_secs, 300);
        assert!(mdns.services);
        assert!(!mdns.ipv6);
        assert_eq!(mdns.bind, "0.0.0.0");
        assert!(mdns.allow.is_empty());
    }

    #[test]
    fn test_mdns_absent_when_not_configured() {
        let toml_str = r#"
[instance]
id = "test-01"

[database]
path = "/tmp/test.redb"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.mdns.is_none());
    }

    #[test]
    fn test_parse_dhcp_with_pxe_and_reservations() {
        let toml_str = r#"
[instance]
id = "test-dhcp"
mode = "standalone"

[dhcp.v4]
enabled = true
interface = "eth0"

[[dhcp.v4.pools]]
range_start = "10.0.10.100"
range_end = "10.0.10.200"
subnet = "10.0.10.0/24"
gateway = "10.0.10.1"
dns = ["10.0.10.2"]
domain = "test.lo"
lease_time_secs = 3600
next_server = "10.0.10.5"
boot_file = "pxelinux.0"

[[dhcp.v4.reservations]]
mac = "AA:BB:CC:DD:EE:FF"
ip = "10.0.10.10"
hostname = "server1"

[[dhcp.v4.reservations]]
mac = "11:22:33:44:55:66"
ip = "10.0.10.11"

[database]
path = "/tmp/test.redb"

[logging]
level = "debug"
format = "text"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let v4 = config.dhcp.unwrap().v4.unwrap();
        assert_eq!(v4.pools[0].next_server.as_deref(), Some("10.0.10.5"));
        assert_eq!(v4.pools[0].boot_file.as_deref(), Some("pxelinux.0"));
        assert_eq!(v4.reservations.len(), 2);
        assert_eq!(v4.reservations[0].mac, "AA:BB:CC:DD:EE:FF");
        assert_eq!(v4.reservations[0].hostname.as_deref(), Some("server1"));
        assert!(v4.reservations[1].hostname.is_none());
    }
}
