//! Where discovered names get written.
//!
//! There is one `mdns.lo` zone for the whole network, so exactly one instance
//! holds it. That instance writes to its own database. Every other instance
//! writes into it over the REST API — the same "heard it, registered it" step,
//! just aimed at the box that owns the zone — and points its own clients there
//! for that zone.
//!
//! No copies and no zone transfers: a name exists once, in one place, whichever
//! segment happened to hear it.

use std::collections::HashMap;

use microdns_core::db::Db;
use microdns_core::types::{RecordData, RecordSource};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::MdnsConfig;
use crate::publish::{Applied, Publisher};
use crate::translate::DesiredRecord;

/// How long to wait on the holder before giving up on this round.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Database section a reporting instance remembers its own writes in.
const PUBLISHED_SECTION: &str = "mdns_published";

/// Something that can make the zone match a desired record set.
pub enum ZoneSink {
    /// This instance holds the zone.
    Local(Box<Publisher>),
    /// Another instance holds it; write over its API.
    Remote(Box<RemoteZone>),
}

impl ZoneSink {
    /// Build the sink the config calls for.
    pub fn new(db: Db, config: &MdnsConfig) -> anyhow::Result<Self> {
        if config.is_holder() {
            Ok(ZoneSink::Local(Box::new(Publisher::new(db, config)?)))
        } else {
            Ok(ZoneSink::Remote(Box::new(RemoteZone::new(db, config)?)))
        }
    }

    pub async fn apply(&mut self, desired: &[DesiredRecord], prune: bool) -> anyhow::Result<Applied> {
        match self {
            ZoneSink::Local(publisher) => publisher.apply(desired, prune),
            ZoneSink::Remote(remote) => remote.apply(desired, prune).await,
        }
    }

    /// Remove everything this instance put in the zone.
    pub async fn withdraw_all(&self) -> anyhow::Result<usize> {
        match self {
            ZoneSink::Local(publisher) => publisher.withdraw_all(),
            ZoneSink::Remote(remote) => remote.withdraw_all().await,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            ZoneSink::Local(publisher) => format!("{} (held here)", publisher.zone_name()),
            ZoneSink::Remote(remote) => format!("{} on {}", remote.zone, remote.holder),
        }
    }
}

/// The zone as held by another instance, written to over its REST API.
pub struct RemoteZone {
    db: Db,
    client: reqwest::Client,
    /// `host:port` of the holder's API.
    holder: String,
    zone: String,
    /// Record ids this instance created in the holder's zone, by name and
    /// rdata. Persisted locally so a restart still knows which records are ours
    /// to withdraw — the alternative is guessing, and guessing wrong here means
    /// deleting a name another segment is responsible for.
    published: HashMap<String, Uuid>,
}

/// One entry of the local memory of what we wrote.
#[derive(Debug, Serialize, Deserialize, Default)]
struct PublishedState {
    #[serde(default)]
    records: HashMap<String, Uuid>,
}

/// The holder's view of a record, as its API returns it.
#[derive(Debug, Deserialize)]
struct RemoteRecord {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    _record_type: String,
    data: RecordData,
    #[serde(default)]
    source: RecordSource,
}

#[derive(Debug, Deserialize)]
struct RemoteZoneRef {
    id: Uuid,
    name: String,
}

impl RemoteZone {
    fn new(db: Db, config: &MdnsConfig) -> anyhow::Result<Self> {
        let holder = api_addr(&config.holder);
        let published = db
            .get_runtime_section::<PublishedState>(PUBLISHED_SECTION)
            .ok()
            .flatten()
            .unwrap_or_default()
            .records;
        Ok(Self {
            db,
            client: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            holder,
            zone: config.zone.trim_end_matches('.').to_lowercase(),
            published,
        })
    }

    /// Make the holder's zone match what this instance currently hears.
    async fn apply(&mut self, desired: &[DesiredRecord], prune: bool) -> anyhow::Result<Applied> {
        let zone_id = self.ensure_zone().await?;
        let existing = self.fetch_records(&zone_id).await?;

        // What the holder actually has, by key — the source of truth for
        // whether one of our earlier writes is still there.
        let present: HashMap<String, &RemoteRecord> = existing
            .iter()
            .map(|r| (key(&r.name, &r.data), r))
            .collect();

        let mut applied = Applied::default();
        let mut ours: HashMap<String, Uuid> = HashMap::new();

        for want in desired {
            let k = key(&want.name, &want.data);
            match present.get(&k) {
                Some(record) => {
                    // Already there. Claim it as ours only if we put it there,
                    // or if nobody else has: a curated record with the same
                    // name and value is left entirely alone.
                    if record.source == RecordSource::Mdns {
                        ours.insert(k, record.id);
                    } else {
                        applied.shadowed += 1;
                    }
                }
                None => {
                    // A curated record already owning this name wins.
                    if existing.iter().any(|r| {
                        r.name == want.name
                            && r.data.record_type() == want.record_type()
                            && r.source != RecordSource::Mdns
                    }) {
                        applied.shadowed += 1;
                        continue;
                    }
                    match self.create(&zone_id, want).await {
                        Ok(id) => {
                            ours.insert(k, id);
                            applied.created += 1;
                        }
                        Err(e) => warn!("mdns: could not register {}.{}: {e}", want.name, self.zone),
                    }
                }
            }
        }

        if prune {
            let stale: Vec<(String, Uuid)> = self
                .published
                .iter()
                .filter(|(k, _)| !ours.contains_key(*k))
                .map(|(k, id)| (k.clone(), *id))
                .collect();
            for (k, id) in stale {
                // Only delete what is still there; a record the holder has
                // already lost is simply forgotten.
                if existing.iter().any(|r| r.id == id) {
                    match self.delete(&zone_id, &id).await {
                        Ok(()) => applied.deleted += 1,
                        Err(e) => {
                            warn!("mdns: could not withdraw {k} from {}: {e}", self.zone);
                            ours.insert(k, id);
                            continue;
                        }
                    }
                }
            }
        } else {
            // Still starting up: keep claiming what we wrote before, so the
            // next prune does not treat it as somebody else's.
            for (k, id) in &self.published {
                ours.entry(k.clone()).or_insert(*id);
            }
        }

        if ours != self.published {
            self.published = ours;
            self.remember();
        }
        if applied.changed() {
            info!(
                "mdns: {} on {} updated (+{} -{})",
                self.zone, self.holder, applied.created, applied.deleted
            );
        }
        Ok(applied)
    }

    async fn withdraw_all(&self) -> anyhow::Result<usize> {
        let zone_id = match self.zone_id().await? {
            Some(id) => id,
            None => return Ok(0),
        };
        let mut removed = 0;
        for id in self.published.values() {
            if self.delete(&zone_id, id).await.is_ok() {
                removed += 1;
            }
        }
        let _ = self.db.delete_runtime_section(PUBLISHED_SECTION);
        Ok(removed)
    }

    /// Persist which records are ours, so a restart can still withdraw them.
    fn remember(&self) {
        let state = PublishedState {
            records: self.published.clone(),
        };
        if let Err(e) = self.db.set_runtime_section(PUBLISHED_SECTION, &state) {
            warn!("mdns: could not record what was registered: {e}");
        }
    }

    async fn zone_id(&self) -> anyhow::Result<Option<Uuid>> {
        let url = format!("http://{}/api/v1/zones", self.holder);
        let zones: Vec<RemoteZoneRef> = self.client.get(&url).send().await?.json().await?;
        Ok(zones
            .into_iter()
            .find(|z| z.name.trim_end_matches('.').eq_ignore_ascii_case(&self.zone))
            .map(|z| z.id))
    }

    /// Find the zone on the holder, creating it if this is the first name to
    /// land there.
    async fn ensure_zone(&self) -> anyhow::Result<Uuid> {
        if let Some(id) = self.zone_id().await? {
            return Ok(id);
        }
        let url = format!("http://{}/api/v1/zones", self.holder);
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "name": self.zone }))
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("holder refused to create {}: {}", self.zone, response.status());
        }
        let created: RemoteZoneRef = response.json().await?;
        info!("mdns: created {} on {}", self.zone, self.holder);
        Ok(created.id)
    }

    async fn fetch_records(&self, zone_id: &Uuid) -> anyhow::Result<Vec<RemoteRecord>> {
        let url = format!(
            "http://{}/api/v1/zones/{zone_id}/records?limit=5000",
            self.holder
        );
        Ok(self.client.get(&url).send().await?.json().await?)
    }

    async fn create(&self, zone_id: &Uuid, want: &DesiredRecord) -> anyhow::Result<Uuid> {
        let url = format!("http://{}/api/v1/zones/{zone_id}/records", self.holder);
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "name": want.name,
                "ttl": want.ttl,
                "data": want.data,
                "enabled": true,
                "source": "mdns",
            }))
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("holder answered {}", response.status());
        }
        let created: RemoteRecord = response.json().await?;
        debug!("mdns: registered {}.{} on {}", want.name, self.zone, self.holder);
        Ok(created.id)
    }

    async fn delete(&self, zone_id: &Uuid, id: &Uuid) -> anyhow::Result<()> {
        let url = format!("http://{}/api/v1/zones/{zone_id}/records/{id}", self.holder);
        let response = self.client.delete(&url).send().await?;
        // Already gone is the outcome we wanted.
        if !response.status().is_success() && response.status().as_u16() != 404 {
            anyhow::bail!("holder answered {}", response.status());
        }
        Ok(())
    }
}

/// A key identifying one record by what it says, not by who stored it.
fn key(name: &str, data: &RecordData) -> String {
    format!("{}|{}|{:?}", name.to_lowercase(), data.record_type(), data)
}

/// The holder's API address. Config gives a DNS address (or a bare IP); the API
/// is on 8080 on the same host, as it is on every instance in a fleet.
fn api_addr(holder: &str) -> String {
    let host = holder
        .trim()
        .rsplit_once(':')
        .map_or(holder.trim(), |(h, _)| h);
    format!("{host}:8080")
}

/// Point this instance's own clients at the holder for the shared zone, so a
/// lookup arriving here is answered by the box that has the names.
pub fn ensure_forwarder(db: &Db, config: &MdnsConfig) {
    let Some(target) = config.holder_dns_addr() else {
        return;
    };
    let zone = config.zone.trim_end_matches('.').to_lowercase();

    if let Ok(Some(existing)) = db.get_dns_forwarder(&zone) {
        if existing.servers == vec![target.clone()] {
            return;
        }
    }
    let forwarder = microdns_core::types::DnsForwarder {
        zone: zone.clone(),
        servers: vec![target.clone()],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    match db.create_dns_forwarder(&forwarder) {
        Ok(()) => info!("mdns: {zone} lookups from this instance now go to {target}"),
        Err(e) => warn!("mdns: could not point {zone} at {target}: {e}"),
    }
}

/// Stop pointing clients at the holder — used when the source is turned off.
pub fn remove_forwarder(db: &Db, config: &MdnsConfig) {
    if config.is_holder() {
        return;
    }
    let zone = config.zone.trim_end_matches('.').to_lowercase();
    let _ = db.delete_dns_forwarder(&zone);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_address_is_derived_from_the_dns_one() {
        assert_eq!(api_addr("192.168.1.52"), "192.168.1.52:8080");
        assert_eq!(api_addr("192.168.1.52:53"), "192.168.1.52:8080");
        assert_eq!(api_addr(" 192.168.1.52 "), "192.168.1.52:8080");
    }

    #[test]
    fn a_record_key_ignores_case_but_not_value() {
        let a1 = RecordData::A("192.168.9.1".parse().unwrap());
        let a2 = RecordData::A("192.168.9.2".parse().unwrap());
        assert_eq!(key("Host", &a1), key("host", &a1));
        assert_ne!(key("host", &a1), key("host", &a2));
    }

    #[test]
    fn an_empty_holder_means_this_instance_owns_the_zone() {
        let mut config = MdnsConfig::default();
        assert!(config.is_holder());
        assert_eq!(config.holder_dns_addr(), None);

        config.holder = "192.168.1.52".into();
        assert!(!config.is_holder());
        assert_eq!(config.holder_dns_addr(), Some("192.168.1.52:53".to_string()));

        config.holder = "192.168.1.52:5353".into();
        assert_eq!(
            config.holder_dns_addr(),
            Some("192.168.1.52:5353".to_string())
        );
    }
}
