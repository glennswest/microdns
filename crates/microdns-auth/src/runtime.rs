//! Live zone-transfer settings, shared by everything that acts on them.
//!
//! Three parts of the server need these settings and none of them can be
//! restarted to pick up a change: the DNS listener enforces the AXFR ACL and
//! decides whose NOTIFY to believe, the announcer needs the secondary list, and
//! the mirror agent needs the zone list. They all read the same parsed snapshot
//! through this handle, and a watcher swaps it when the stored config changes.
//!
//! Parsing happens once, on the way in. Entries that do not parse are dropped
//! with a warning rather than rejected wholesale — a typo in one CIDR must not
//! take DNS down, and dropping an ACL entry denies rather than permits.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use microdns_core::config::ZoneTransferConfig;
use tracing::{info, warn};

use crate::secondary::SecondaryZone;
use crate::server::IpNet;

/// The parsed form of [`ZoneTransferConfig`].
#[derive(Debug, Default, Clone)]
pub struct TransferSettings {
    pub allow_transfer: Vec<IpNet>,
    pub notify: Vec<SocketAddr>,
    pub secondaries: Vec<SecondaryZone>,
}

impl TransferSettings {
    pub fn parse(config: &ZoneTransferConfig) -> Self {
        let allow_transfer = config
            .allow_transfer
            .iter()
            .filter_map(|c| match IpNet::parse(c) {
                Some(net) => Some(net),
                None => {
                    warn!("ignoring invalid allow_transfer entry '{c}'");
                    None
                }
            })
            .collect::<Vec<_>>();

        let notify = config
            .notify
            .iter()
            .filter_map(|t| match parse_addr(t) {
                Some(addr) => Some(addr),
                None => {
                    warn!("ignoring invalid notify target '{t}'");
                    None
                }
            })
            .collect();

        let secondaries = config
            .secondary
            .iter()
            .filter_map(|s| {
                match SecondaryZone::parse(&s.zone, &s.primary, s.refresh_secs) {
                    Some(zone) => Some(zone),
                    None => {
                        warn!(
                            "ignoring secondary zone '{}': primary '{}' is not an address",
                            s.zone, s.primary
                        );
                        None
                    }
                }
            })
            .collect();

        Self {
            allow_transfer,
            notify,
            secondaries,
        }
    }

    /// One line describing what is in force, for the log on startup or change.
    pub fn summary(&self) -> String {
        let transfers = if self.allow_transfer.is_empty() {
            "transfers denied to all".to_string()
        } else {
            format!("transfers allowed from {} CIDR(s)", self.allow_transfer.len())
        };
        format!(
            "{transfers}, {} notify target(s), {} mirrored zone(s)",
            self.notify.len(),
            self.secondaries.len()
        )
    }
}

/// Shared handle to the settings in force.
#[derive(Clone, Default)]
pub struct TransferState {
    inner: Arc<Mutex<TransferSettings>>,
}

impl TransferState {
    pub fn new(config: &ZoneTransferConfig) -> Self {
        let settings = TransferSettings::parse(config);
        info!("zone transfer: {}", settings.summary());
        Self {
            inner: Arc::new(Mutex::new(settings)),
        }
    }

    /// Swap in new settings. Returns whether anything an operator would notice
    /// actually changed, so the caller can log a change and not a heartbeat.
    pub fn replace(&self, config: &ZoneTransferConfig) -> bool {
        let next = TransferSettings::parse(config);
        let mut current = self.inner.lock().unwrap();
        let changed = current.summary() != next.summary()
            || current.secondaries.len() != next.secondaries.len()
            || !same_secondaries(&current.secondaries, &next.secondaries)
            || current.notify != next.notify
            || current.allow_transfer != next.allow_transfer;
        *current = next;
        changed
    }

    /// CIDRs permitted to request a transfer.
    pub fn allow_transfer(&self) -> Vec<IpNet> {
        self.inner.lock().unwrap().allow_transfer.clone()
    }

    /// Secondaries to announce zone changes to.
    pub fn notify_targets(&self) -> Vec<SocketAddr> {
        self.inner.lock().unwrap().notify.clone()
    }

    /// Zones this instance mirrors.
    pub fn secondaries(&self) -> Vec<SecondaryZone> {
        self.inner.lock().unwrap().secondaries.clone()
    }

    /// The address allowed to announce changes to `zone`, if it is mirrored.
    pub fn primary_for(&self, zone: &str) -> Option<IpAddr> {
        let zone = zone.trim_end_matches('.').to_lowercase();
        self.inner
            .lock()
            .unwrap()
            .secondaries
            .iter()
            .find(|s| s.zone == zone)
            .map(|s| s.primary.ip())
    }

    pub fn summary(&self) -> String {
        self.inner.lock().unwrap().summary()
    }
}

fn same_secondaries(a: &[SecondaryZone], b: &[SecondaryZone]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.zone == y.zone && x.primary == y.primary && x.refresh == y.refresh
        })
}

/// Parse `192.168.1.253` or `192.168.1.253:53`, defaulting to port 53.
pub fn parse_addr(target: &str) -> Option<SocketAddr> {
    let t = target.trim();
    if let Ok(addr) = t.parse::<SocketAddr>() {
        return Some(addr);
    }
    t.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53))
}

#[cfg(test)]
mod tests {
    use super::*;
    use microdns_core::config::SecondaryZoneConfig;

    fn config() -> ZoneTransferConfig {
        ZoneTransferConfig {
            allow_transfer: vec!["192.168.0.0/16".into()],
            notify: vec!["192.168.1.51".into()],
            secondary: vec![SecondaryZoneConfig {
                zone: "gw.lo".into(),
                primary: "192.168.1.252".into(),
                refresh_secs: 900,
            }],
        }
    }

    #[test]
    fn parsing_keeps_the_good_entries_and_drops_the_rest() {
        let mut cfg = config();
        cfg.allow_transfer.push("not-a-cidr".into());
        cfg.notify.push("nonsense".into());
        cfg.secondary.push(SecondaryZoneConfig {
            zone: "broken.lo".into(),
            primary: "also-nonsense".into(),
            refresh_secs: 900,
        });

        let settings = TransferSettings::parse(&cfg);
        assert_eq!(settings.allow_transfer.len(), 1);
        assert_eq!(settings.notify, vec!["192.168.1.51:53".parse().unwrap()]);
        assert_eq!(settings.secondaries.len(), 1);
        assert_eq!(settings.secondaries[0].zone, "gw.lo");
    }

    #[test]
    fn state_reports_only_real_changes() {
        let state = TransferState::new(&config());
        assert!(!state.replace(&config()), "identical config is not a change");

        let mut changed = config();
        changed.notify.push("192.168.8.253".into());
        assert!(state.replace(&changed));
        assert_eq!(state.notify_targets().len(), 2);

        let mut rehomed = config();
        rehomed.secondary[0].primary = "192.168.1.99".into();
        assert!(state.replace(&rehomed));
        assert_eq!(
            state.primary_for("gw.lo."),
            Some("192.168.1.99".parse().unwrap())
        );
    }

    #[test]
    fn a_zone_that_is_not_mirrored_has_no_primary() {
        let state = TransferState::new(&config());
        assert!(state.primary_for("g10.lo").is_none());
        assert_eq!(
            state.primary_for("GW.lo"),
            Some("192.168.1.252".parse().unwrap()),
            "zone matching ignores case and the trailing dot"
        );
    }

    #[test]
    fn empty_config_denies_transfers_and_mirrors_nothing() {
        let state = TransferState::new(&ZoneTransferConfig {
            allow_transfer: vec![],
            notify: vec![],
            secondary: vec![],
        });
        assert!(state.allow_transfer().is_empty());
        assert!(state.secondaries().is_empty());
        assert!(state.summary().contains("denied to all"));
    }
}
