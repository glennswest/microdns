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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpV4Config {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub interface: String,
    pub pools: Vec<DhcpV4Pool>,
    #[serde(default)]
    pub reservations: Vec<DhcpReservation>,
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
    #[serde(default)]
    pub api_key: Option<String>,
}

impl Default for RestApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen: default_rest_listen(),
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
fn default_check_interval() -> u64 {
    10
}
fn default_probe_type() -> String {
    "ping".to_string()
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

[api.rest]
enabled = true
listen = "0.0.0.0:8080"
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
