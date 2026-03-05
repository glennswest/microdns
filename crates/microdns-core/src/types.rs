use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr};
use uuid::Uuid;

/// DNS record types supported by microdns
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RecordType {
    A,
    AAAA,
    CNAME,
    MX,
    NS,
    PTR,
    SOA,
    SRV,
    TXT,
    CAA,
}

impl std::fmt::Display for RecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordType::A => write!(f, "A"),
            RecordType::AAAA => write!(f, "AAAA"),
            RecordType::CNAME => write!(f, "CNAME"),
            RecordType::MX => write!(f, "MX"),
            RecordType::NS => write!(f, "NS"),
            RecordType::PTR => write!(f, "PTR"),
            RecordType::SOA => write!(f, "SOA"),
            RecordType::SRV => write!(f, "SRV"),
            RecordType::TXT => write!(f, "TXT"),
            RecordType::CAA => write!(f, "CAA"),
        }
    }
}

impl std::str::FromStr for RecordType {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "A" => Ok(RecordType::A),
            "AAAA" => Ok(RecordType::AAAA),
            "CNAME" => Ok(RecordType::CNAME),
            "MX" => Ok(RecordType::MX),
            "NS" => Ok(RecordType::NS),
            "PTR" => Ok(RecordType::PTR),
            "SOA" => Ok(RecordType::SOA),
            "SRV" => Ok(RecordType::SRV),
            "TXT" => Ok(RecordType::TXT),
            "CAA" => Ok(RecordType::CAA),
            _ => Err(crate::error::Error::InvalidRecord(format!(
                "unknown record type: {s}"
            ))),
        }
    }
}

/// DNS record data variants
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum RecordData {
    A(Ipv4Addr),
    AAAA(Ipv6Addr),
    CNAME(String),
    MX { preference: u16, exchange: String },
    NS(String),
    PTR(String),
    SOA(SoaData),
    SRV(SrvData),
    TXT(String),
    CAA(CaaData),
}

impl RecordData {
    pub fn record_type(&self) -> RecordType {
        match self {
            RecordData::A(_) => RecordType::A,
            RecordData::AAAA(_) => RecordType::AAAA,
            RecordData::CNAME(_) => RecordType::CNAME,
            RecordData::MX { .. } => RecordType::MX,
            RecordData::NS(_) => RecordType::NS,
            RecordData::PTR(_) => RecordType::PTR,
            RecordData::SOA(_) => RecordType::SOA,
            RecordData::SRV(_) => RecordType::SRV,
            RecordData::TXT(_) => RecordType::TXT,
            RecordData::CAA(_) => RecordType::CAA,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoaData {
    pub mname: String,
    pub rname: String,
    pub serial: u32,
    pub refresh: u32,
    pub retry: u32,
    pub expire: u32,
    pub minimum: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrvData {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaaData {
    pub flags: u8,
    pub tag: String,
    pub value: String,
}

/// A DNS zone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    pub id: Uuid,
    pub name: String,
    pub soa: SoaData,
    pub default_ttl: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A DNS record within a zone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: Uuid,
    pub zone_id: Uuid,
    pub name: String,
    pub ttl: u32,
    pub data: RecordData,
    pub enabled: bool,
    /// Health check configuration for load balancer
    pub health_check: Option<HealthCheck>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Health check configuration for a record (used by LB)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    pub probe_type: ProbeType,
    pub interval_secs: u32,
    pub timeout_secs: u32,
    pub unhealthy_threshold: u32,
    pub healthy_threshold: u32,
    /// Optional: specific port/path for HTTP checks
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeType {
    Ping,
    Http,
    Https,
    Tcp,
}

/// DHCP lease record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub id: Uuid,
    pub ip_addr: String,
    pub mac_addr: String,
    pub hostname: Option<String>,
    pub lease_start: DateTime<Utc>,
    pub lease_end: DateTime<Utc>,
    pub pool_id: String,
    pub state: LeaseState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseState {
    Active,
    Expired,
    Released,
}

/// IPAM allocation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpamAllocation {
    pub id: Uuid,
    pub pool: String,
    pub ip_addr: String,
    pub container: String,
    pub gateway: String,
    pub bridge: String,
    pub subnet: String,
    pub created_at: DateTime<Utc>,
}

/// Replication metadata for tracking zone sync state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationMeta {
    pub zone_id: Uuid,
    pub zone_name: String,
    pub source_peer_id: String,
    pub last_synced: DateTime<Utc>,
    pub source_serial: u32,
}

/// Classless static route for DHCP option 121
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticRoute {
    /// CIDR notation, e.g. "10.0.0.0/8"
    pub destination: String,
    pub gateway: String,
}

/// DHCP pool stored in database (replaces TOML-based DhcpV4Pool)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpPool {
    pub id: Uuid,
    pub name: String,
    pub range_start: String,
    pub range_end: String,
    pub subnet: String,
    pub gateway: String,
    pub dns_servers: Vec<String>,
    pub domain: String,
    #[serde(default = "default_lease_time")]
    pub lease_time_secs: u64,
    // PXE options
    #[serde(default)]
    pub next_server: Option<String>,
    #[serde(default)]
    pub boot_file: Option<String>,
    #[serde(default)]
    pub boot_file_efi: Option<String>,
    #[serde(default)]
    pub ipxe_boot_url: Option<String>,
    // Extended DHCP options
    #[serde(default)]
    pub ntp_servers: Option<Vec<String>>,
    #[serde(default)]
    pub domain_search: Option<Vec<String>>,
    #[serde(default)]
    pub mtu: Option<u16>,
    #[serde(default)]
    pub static_routes: Option<Vec<StaticRoute>>,
    #[serde(default)]
    pub log_server: Option<String>,
    #[serde(default)]
    pub time_offset: Option<i32>,
    #[serde(default)]
    pub wpad_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_lease_time() -> u64 {
    3600
}

/// DHCP reservation stored in database (replaces TOML-based DhcpReservation).
/// Per-reservation fields override pool defaults when set (None = inherit from pool).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpDbReservation {
    /// MAC address (normalized lowercase), used as key
    pub mac: String,
    pub ip: String,
    #[serde(default)]
    pub hostname: Option<String>,
    // Per-reservation overrides
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub dns_servers: Option<Vec<String>>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub ntp_servers: Option<Vec<String>>,
    #[serde(default)]
    pub domain_search: Option<Vec<String>>,
    #[serde(default)]
    pub mtu: Option<u16>,
    #[serde(default)]
    pub next_server: Option<String>,
    #[serde(default)]
    pub boot_file: Option<String>,
    #[serde(default)]
    pub boot_file_efi: Option<String>,
    #[serde(default)]
    pub ipxe_boot_url: Option<String>,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub static_routes: Option<Vec<StaticRoute>>,
    #[serde(default)]
    pub log_server: Option<String>,
    #[serde(default)]
    pub time_offset: Option<i32>,
    #[serde(default)]
    pub wpad_url: Option<String>,
    #[serde(default)]
    pub lease_time_secs: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// DNS forward zone stored in database (replaces TOML forward_zones)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsForwarder {
    /// Zone name (key)
    pub zone: String,
    /// Upstream DNS servers (host or host:port)
    pub servers: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Instance-level DHCP config stored in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbInstanceConfig {
    pub listen_dns: Option<String>,
    pub listen_api: Option<String>,
    pub dhcp_interface: Option<String>,
    pub dhcp_mode: Option<String>,
    pub server_ip: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Instance mode for federation and runtime environment.
///
/// - `standalone` — single-instance, no federation, standard DHCP.
/// - `gateway`    — single-instance, no federation, DHCP defaults to
///                  relay-only mode (for RouterOS / rose containers).
/// - `leaf`       — federation leaf, reports to coordinator.
/// - `coordinator` — federation coordinator, aggregates leaf status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceMode {
    #[default]
    Standalone,
    Gateway,
    Leaf,
    Coordinator,
}
