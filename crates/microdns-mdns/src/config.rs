//! Runtime configuration for the mDNS source.
//!
//! The TOML shape lives in `microdns_core::config::MdnsConfig`; the binary maps
//! it onto this struct, the same split the Kubernetes source uses.

use std::net::Ipv4Addr;

/// The IPv4 mDNS group and port (RFC 6762 §3).
pub const MDNS_GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
/// The IPv6 mDNS group (`ff02::fb`).
pub const MDNS_GROUP_V6: std::net::Ipv6Addr =
    std::net::Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x00fb);
/// mDNS is defined on port 5353 and nowhere else; the knob exists for tests.
pub const MDNS_PORT: u16 = 5353;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsConfig {
    /// Whether the source should be listening at all. Config lives in the
    /// database, so this flips at runtime rather than only at startup.
    pub enabled: bool,
    /// Zone discovered names are published into, e.g. `mdns.g9.lo`. A dedicated
    /// subzone keeps discovered names visibly separate from curated ones.
    pub zone: String,
    /// Floor for the announced TTL. Legacy unicast responses carry a 10 s TTL
    /// (RFC 6762 §6.7); honouring that literally would churn the zone.
    pub ttl_min: u32,
    /// Ceiling for the announced TTL, so a device advertising a huge TTL cannot
    /// pin a record that has gone stale.
    pub ttl_max: u32,
    /// Also mirror DNS-SD service records (PTR/SRV/TXT), not just addresses.
    pub services: bool,
    /// Glob patterns (`*` wildcard) of names to publish. Empty allows all.
    pub allow: Vec<String>,
    /// Glob patterns of names never to publish. Checked after `allow`.
    pub deny: Vec<String>,
    /// How often to send the DNS-SD service enumeration query. 0 disables
    /// active querying, leaving the source purely passive.
    pub query_interval_secs: u64,
    /// Also join the IPv6 group (`ff02::fb`). Off by default: responders that
    /// answer over IPv6 generally answer over IPv4 too, and a v6 join fails on
    /// hosts without IPv6 configured.
    pub ipv6: bool,
    /// Address to bind the listener to. `0.0.0.0` receives on every interface.
    pub bind: Ipv4Addr,
    /// Port to listen on. Always 5353 outside tests.
    pub port: u16,
    /// Local interface addresses to join the group on. Empty lets the kernel
    /// pick, which is what a single-interface container wants.
    pub interfaces: Vec<Ipv4Addr>,
    /// Quiet window before a burst of announcements is written to the zone.
    pub debounce_secs: u64,
    /// Sibling instances to mirror discoveries from. Empty derives them from
    /// this instance's DNS forwarders.
    pub peers: Vec<String>,
    /// How often to pull each sibling. 0 turns mirroring off.
    pub peer_sync_secs: u64,
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            zone: "mdns.lo".to_string(),
            ttl_min: 60,
            ttl_max: 1200,
            services: true,
            allow: Vec::new(),
            deny: Vec::new(),
            query_interval_secs: 300,
            ipv6: false,
            bind: Ipv4Addr::UNSPECIFIED,
            port: MDNS_PORT,
            interfaces: Vec::new(),
            debounce_secs: 5,
            peers: Vec::new(),
            peer_sync_secs: 30,
        }
    }
}

impl From<&microdns_core::config::MdnsSourceConfig> for MdnsConfig {
    /// Map the stored/bootstrap shape onto the runtime one. Addresses that do
    /// not parse fall back to the default rather than failing the source: a
    /// typo in one interface must not silence discovery altogether.
    fn from(c: &microdns_core::config::MdnsSourceConfig) -> Self {
        let bind = c.bind.parse().unwrap_or_else(|_| {
            tracing::warn!("mdns: bind '{}' is not an IPv4 address; using 0.0.0.0", c.bind);
            Ipv4Addr::UNSPECIFIED
        });
        let interfaces = c
            .interfaces
            .iter()
            .filter_map(|s| match s.parse() {
                Ok(ip) => Some(ip),
                Err(_) => {
                    tracing::warn!("mdns: ignoring invalid interface address '{s}'");
                    None
                }
            })
            .collect();

        Self {
            enabled: c.enabled,
            zone: c.zone.clone(),
            ttl_min: c.ttl_min,
            ttl_max: c.ttl_max,
            services: c.services,
            allow: c.allow.clone(),
            deny: c.deny.clone(),
            query_interval_secs: c.query_interval_secs,
            ipv6: c.ipv6,
            bind,
            port: MDNS_PORT,
            interfaces,
            debounce_secs: c.debounce_secs,
            peers: c.peers.clone(),
            peer_sync_secs: c.peer_sync_secs,
        }
    }
}

impl MdnsConfig {
    /// Clamp an announced TTL into the configured window.
    pub fn clamp_ttl(&self, ttl: u32) -> u32 {
        ttl.clamp(self.ttl_min.min(self.ttl_max), self.ttl_max)
    }

    /// Whether a discovered name (relative to the publish zone, e.g.
    /// `teslatracker-52c4` or `printer._ipp._tcp`) may be published.
    ///
    /// `deny` is checked last so it always wins: a broad `allow` plus a narrow
    /// `deny` is the readable way to express "everything except the noisy ones".
    pub fn permits(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        if !self.allow.is_empty() && !self.allow.iter().any(|p| glob_match(p, &name)) {
            return false;
        }
        !self.deny.iter().any(|p| glob_match(p, &name))
    }
}

/// Match a name against a pattern where `*` stands for any run of characters.
/// Deliberately not a full glob: operators write `chromecast-*`, not character
/// classes, and a tiny matcher is one less dependency to audit.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let mut parts = pattern.split('*');

    let Some(first) = parts.next() else {
        return true;
    };
    if !name.starts_with(first) {
        return false;
    }
    let mut rest = &name[first.len()..];

    let mut last: Option<&str> = None;
    for part in parts {
        last = Some(part);
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(idx) => rest = &rest[idx + part.len()..],
            None => return false,
        }
    }

    // A pattern with no `*` must match in full; one ending in a literal must
    // have consumed the tail of the name.
    match last {
        None => rest.is_empty(),
        Some(tail) if !tail.is_empty() => rest.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_literals_prefixes_and_suffixes() {
        assert!(glob_match("printer", "printer"));
        assert!(!glob_match("printer", "printer2"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("chromecast-*", "chromecast-1234"));
        assert!(!glob_match("chromecast-*", "roku-1234"));
        assert!(glob_match("*._ipp._tcp", "brother._ipp._tcp"));
        assert!(!glob_match("*._ipp._tcp", "brother._http._tcp"));
        assert!(glob_match("tesla*tracker*", "teslatracker-52c4"));
        assert!(glob_match("TESLA*", "teslatracker-52c4"));
    }

    #[test]
    fn empty_allow_permits_everything_and_deny_wins() {
        let mut cfg = MdnsConfig::default();
        assert!(cfg.permits("anything"));

        cfg.allow = vec!["tesla*".into()];
        assert!(cfg.permits("teslatracker-52c4"));
        assert!(!cfg.permits("chromecast-99"));

        cfg.allow = vec!["*".into()];
        cfg.deny = vec!["chromecast-*".into()];
        assert!(cfg.permits("teslatracker-52c4"));
        assert!(!cfg.permits("chromecast-99"));
    }

    #[test]
    fn ttl_is_clamped_into_the_window() {
        let cfg = MdnsConfig {
            ttl_min: 60,
            ttl_max: 1200,
            ..Default::default()
        };
        assert_eq!(cfg.clamp_ttl(10), 60);
        assert_eq!(cfg.clamp_ttl(120), 120);
        assert_eq!(cfg.clamp_ttl(4500), 1200);
    }
}
