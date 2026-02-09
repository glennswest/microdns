use std::collections::HashMap;
use std::net::SocketAddr;

/// Manages forward zone configuration.
/// Maps zone names to lists of upstream DNS servers.
pub struct ForwardTable {
    zones: Vec<ForwardZone>,
}

struct ForwardZone {
    /// Zone name (lowercase, no trailing dot)
    name: String,
    /// Upstream servers to forward to
    servers: Vec<SocketAddr>,
}

impl ForwardTable {
    /// Build from config map: zone_name -> [addr:port, ...]
    pub fn from_config(config: &HashMap<String, Vec<String>>) -> Self {
        let mut zones: Vec<ForwardZone> = config
            .iter()
            .filter_map(|(name, addrs)| {
                let servers: Vec<SocketAddr> = addrs
                    .iter()
                    .filter_map(|a| {
                        // Accept "host:port" or just "host" (default port 53)
                        if a.contains(':') {
                            a.parse().ok()
                        } else {
                            format!("{a}:53").parse().ok()
                        }
                    })
                    .collect();

                if servers.is_empty() {
                    tracing::warn!("forward zone {name}: no valid upstream servers");
                    return None;
                }

                Some(ForwardZone {
                    name: name.trim_end_matches('.').to_lowercase(),
                    servers,
                })
            })
            .collect();

        // Sort by length descending so most-specific match wins
        zones.sort_by(|a, b| b.name.len().cmp(&a.name.len()));

        Self { zones }
    }

    /// Find the forward servers for a given FQDN.
    /// Returns the most specific matching zone's servers.
    pub fn lookup(&self, fqdn: &str) -> Option<&[SocketAddr]> {
        let fqdn = fqdn.trim_end_matches('.').to_lowercase();

        for fz in &self.zones {
            if fqdn == fz.name || fqdn.ends_with(&format!(".{}", fz.name)) {
                return Some(&fz.servers);
            }
        }

        None
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forward_lookup() {
        let mut config = HashMap::new();
        config.insert(
            "corp.local".to_string(),
            vec!["10.0.1.1:53".to_string(), "10.0.1.2:53".to_string()],
        );
        config.insert(
            "dev.corp.local".to_string(),
            vec!["10.0.2.1:53".to_string()],
        );

        let table = ForwardTable::from_config(&config);

        // Most specific match
        let servers = table.lookup("host.dev.corp.local").unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].to_string(), "10.0.2.1:53");

        // Less specific match
        let servers = table.lookup("host.corp.local").unwrap();
        assert_eq!(servers.len(), 2);

        // No match
        assert!(table.lookup("example.com").is_none());
    }

    #[test]
    fn test_forward_exact_match() {
        let mut config = HashMap::new();
        config.insert(
            "corp.local".to_string(),
            vec!["10.0.1.1".to_string()],
        );

        let table = ForwardTable::from_config(&config);
        let servers = table.lookup("corp.local").unwrap();
        assert_eq!(servers[0].to_string(), "10.0.1.1:53");
    }

    #[test]
    fn test_forward_empty() {
        let config = HashMap::new();
        let table = ForwardTable::from_config(&config);
        assert!(table.is_empty());
        assert!(table.lookup("anything.com").is_none());
    }
}
