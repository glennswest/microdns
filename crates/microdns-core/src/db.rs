use crate::error::{Error, Result};
use crate::types::{IpamAllocation, Record, RecordType, ReplicationMeta, Zone};
use chrono::Utc;
use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

/// Zones table: zone_id (string) -> Zone (JSON)
const ZONES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("zones");

/// Zone name index: zone_name (string) -> zone_id (string)
const ZONE_NAME_INDEX: TableDefinition<&str, &str> = TableDefinition::new("zone_name_index");

/// Records table: record_id (string) -> Record (JSON)
const RECORDS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("records");

/// Records by zone index: "zone_id:name:type" -> comma-separated record_ids
const RECORDS_BY_ZONE: TableDefinition<&str, &str> = TableDefinition::new("records_by_zone");

/// Leases table: lease_id (string) -> Lease (JSON) - used by DHCP in later phases
const LEASES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("leases");

/// IPAM allocations table: allocation_id (string) -> IpamAllocation (JSON)
const IPAM_TABLE: TableDefinition<&str, &str> = TableDefinition::new("ipam_allocations");

/// Replication metadata table: zone_id (string) -> ReplicationMeta (JSON)
const REPLICATION_META_TABLE: TableDefinition<&str, &str> = TableDefinition::new("replication_meta");

#[derive(Clone)]
pub struct Db {
    inner: Arc<Database>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::create(path)?;

        // Initialize tables
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(ZONES_TABLE)?;
            let _ = write_txn.open_table(ZONE_NAME_INDEX)?;
            let _ = write_txn.open_table(RECORDS_TABLE)?;
            let _ = write_txn.open_table(RECORDS_BY_ZONE)?;
            let _ = write_txn.open_table(LEASES_TABLE)?;
            let _ = write_txn.open_table(IPAM_TABLE)?;
            let _ = write_txn.open_table(REPLICATION_META_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self {
            inner: Arc::new(db),
        })
    }

    /// Access the underlying redb Database for custom table operations.
    pub fn raw(&self) -> &Database {
        &self.inner
    }

    // --- Zone operations ---

    pub fn create_zone(&self, name: &str, zone: &Zone) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let mut name_idx = write_txn.open_table(ZONE_NAME_INDEX)?;
            if name_idx.get(name)?.is_some() {
                return Err(Error::DuplicateZone(name.to_string()));
            }

            let id_str = zone.id.to_string();
            let json = serde_json::to_string(zone)?;

            let mut zones = write_txn.open_table(ZONES_TABLE)?;
            zones.insert(id_str.as_str(), json.as_str())?;
            name_idx.insert(name, id_str.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_zone_by_name(&self, name: &str) -> Result<Option<Zone>> {
        let read_txn = self.inner.begin_read()?;
        let name_idx = read_txn.open_table(ZONE_NAME_INDEX)?;

        let zone_id = match name_idx.get(name)? {
            Some(v) => v.value().to_string(),
            None => return Ok(None),
        };

        let zones = read_txn.open_table(ZONES_TABLE)?;
        match zones.get(zone_id.as_str())? {
            Some(v) => {
                let zone: Zone = serde_json::from_str(v.value())?;
                Ok(Some(zone))
            }
            None => Ok(None),
        }
    }

    pub fn get_zone(&self, id: &Uuid) -> Result<Option<Zone>> {
        let read_txn = self.inner.begin_read()?;
        let zones = read_txn.open_table(ZONES_TABLE)?;
        let id_str = id.to_string();

        match zones.get(id_str.as_str())? {
            Some(v) => {
                let zone: Zone = serde_json::from_str(v.value())?;
                Ok(Some(zone))
            }
            None => Ok(None),
        }
    }

    pub fn list_zones(&self) -> Result<Vec<Zone>> {
        let read_txn = self.inner.begin_read()?;
        let zones = read_txn.open_table(ZONES_TABLE)?;
        let mut result = Vec::new();

        let iter = zones.iter()?;
        for entry in iter {
            let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
            let zone: Zone = serde_json::from_str(entry.1.value())?;
            result.push(zone);
        }

        Ok(result)
    }

    pub fn delete_zone(&self, id: &Uuid) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = id.to_string();
            let mut zones = write_txn.open_table(ZONES_TABLE)?;

            // Get zone to find name for index cleanup
            let zone_json = zones
                .get(id_str.as_str())?
                .ok_or_else(|| Error::ZoneNotFound(id_str.clone()))?;
            let zone: Zone = serde_json::from_str(zone_json.value())?;
            drop(zone_json);

            zones.remove(id_str.as_str())?;

            let mut name_idx = write_txn.open_table(ZONE_NAME_INDEX)?;
            name_idx.remove(zone.name.as_str())?;

            // Delete all records in this zone
            let mut records = write_txn.open_table(RECORDS_TABLE)?;
            let mut by_zone = write_txn.open_table(RECORDS_BY_ZONE)?;

            // Collect record IDs to delete
            let mut to_delete = Vec::new();
            let iter = by_zone.iter()?;
            for entry in iter {
                let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
                let key = entry.0.value().to_string();
                if key.starts_with(&format!("{id_str}:")) {
                    let record_ids: Vec<String> = entry
                        .1
                        .value()
                        .split(',')
                        .map(|s| s.to_string())
                        .collect();
                    to_delete.push((key, record_ids));
                }
            }

            for (index_key, record_ids) in to_delete {
                by_zone.remove(index_key.as_str())?;
                for rid in record_ids {
                    records.remove(rid.as_str())?;
                }
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Delete all records belonging to a zone. Returns count deleted.
    pub fn delete_zone_records(&self, zone_id: &Uuid) -> Result<usize> {
        let write_txn = self.inner.begin_write()?;
        let count;
        {
            let mut records = write_txn.open_table(RECORDS_TABLE)?;
            let mut by_zone = write_txn.open_table(RECORDS_BY_ZONE)?;

            let prefix = format!("{zone_id}:");
            let mut to_delete = Vec::new();

            let iter = by_zone.iter()?;
            for entry in iter {
                let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
                let key = entry.0.value().to_string();
                if key.starts_with(&prefix) {
                    let record_ids: Vec<String> = entry
                        .1
                        .value()
                        .split(',')
                        .map(|s| s.to_string())
                        .collect();
                    to_delete.push((key, record_ids));
                }
            }

            count = to_delete.iter().map(|(_, ids)| ids.len()).sum();

            for (index_key, record_ids) in to_delete {
                by_zone.remove(index_key.as_str())?;
                for rid in record_ids {
                    records.remove(rid.as_str())?;
                }
            }
        }
        write_txn.commit()?;
        Ok(count)
    }

    // --- Record operations ---

    pub fn create_record(&self, record: &Record) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = record.id.to_string();
            let json = serde_json::to_string(record)?;

            let mut records = write_txn.open_table(RECORDS_TABLE)?;
            records.insert(id_str.as_str(), json.as_str())?;

            // Update zone index
            let index_key = format!(
                "{}:{}:{}",
                record.zone_id,
                record.name,
                record.data.record_type()
            );
            let mut by_zone = write_txn.open_table(RECORDS_BY_ZONE)?;

            let new_val = match by_zone.get(index_key.as_str())? {
                Some(v) => format!("{},{}", v.value(), id_str),
                None => id_str.clone(),
            };
            by_zone.insert(index_key.as_str(), new_val.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_record(&self, id: &Uuid) -> Result<Option<Record>> {
        let read_txn = self.inner.begin_read()?;
        let records = read_txn.open_table(RECORDS_TABLE)?;
        let id_str = id.to_string();

        match records.get(id_str.as_str())? {
            Some(v) => {
                let record: Record = serde_json::from_str(v.value())?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Query records for a given zone, name, and record type
    pub fn query_records(
        &self,
        zone_id: &Uuid,
        name: &str,
        rtype: RecordType,
    ) -> Result<Vec<Record>> {
        let read_txn = self.inner.begin_read()?;
        let records = read_txn.open_table(RECORDS_TABLE)?;
        let by_zone = read_txn.open_table(RECORDS_BY_ZONE)?;

        let index_key = format!("{zone_id}:{name}:{rtype}");

        let record_ids = match by_zone.get(index_key.as_str())? {
            Some(v) => v.value().to_string(),
            None => return Ok(Vec::new()),
        };

        let mut result = Vec::new();
        for rid in record_ids.split(',') {
            if let Some(v) = records.get(rid)? {
                let record: Record = serde_json::from_str(v.value())?;
                if record.enabled {
                    result.push(record);
                }
            }
        }

        Ok(result)
    }

    /// List all records in a zone
    pub fn list_records(&self, zone_id: &Uuid) -> Result<Vec<Record>> {
        let read_txn = self.inner.begin_read()?;
        let records = read_txn.open_table(RECORDS_TABLE)?;
        let by_zone = read_txn.open_table(RECORDS_BY_ZONE)?;

        let prefix = format!("{zone_id}:");
        let mut result = Vec::new();

        let iter = by_zone.iter()?;
        for entry in iter {
            let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
            let key = entry.0.value().to_string();
            if key.starts_with(&prefix) {
                let record_ids = entry.1.value().to_string();
                for rid in record_ids.split(',') {
                    if let Some(v) = records.get(rid)? {
                        let record: Record = serde_json::from_str(v.value())?;
                        result.push(record);
                    }
                }
            }
        }

        Ok(result)
    }

    pub fn update_record(&self, record: &Record) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = record.id.to_string();
            let json = serde_json::to_string(record)?;

            let mut records = write_txn.open_table(RECORDS_TABLE)?;
            if records.get(id_str.as_str())?.is_none() {
                return Err(Error::RecordNotFound(id_str));
            }
            records.insert(id_str.as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn delete_record(&self, id: &Uuid) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = id.to_string();
            let mut records = write_txn.open_table(RECORDS_TABLE)?;

            // Get record to find zone index key
            let record_json = records
                .get(id_str.as_str())?
                .ok_or_else(|| Error::RecordNotFound(id_str.clone()))?;
            let record: Record = serde_json::from_str(record_json.value())?;
            drop(record_json);

            records.remove(id_str.as_str())?;

            // Update zone index
            let index_key = format!(
                "{}:{}:{}",
                record.zone_id,
                record.name,
                record.data.record_type()
            );
            let mut by_zone = write_txn.open_table(RECORDS_BY_ZONE)?;

            let existing_ids = by_zone
                .get(index_key.as_str())?
                .map(|v| v.value().to_string());

            if let Some(existing) = existing_ids {
                let ids: Vec<&str> = existing
                    .split(',')
                    .filter(|s| *s != id_str.as_str())
                    .collect();
                if ids.is_empty() {
                    by_zone.remove(index_key.as_str())?;
                } else {
                    let new_val = ids.join(",");
                    by_zone.insert(index_key.as_str(), new_val.as_str())?;
                }
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Get all zones and their record counts (for API listing)
    pub fn get_zone_record_counts(&self) -> Result<Vec<(Zone, usize)>> {
        let zones = self.list_zones()?;
        let mut result = Vec::new();
        for zone in zones {
            let records = self.list_records(&zone.id)?;
            result.push((zone, records.len()));
        }
        Ok(result)
    }

    /// Increment zone SOA serial (called on any record change)
    pub fn increment_soa_serial(&self, zone_id: &Uuid) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = zone_id.to_string();
            let mut zones = write_txn.open_table(ZONES_TABLE)?;

            let zone_json = zones
                .get(id_str.as_str())?
                .ok_or_else(|| Error::ZoneNotFound(id_str.clone()))?;
            let mut zone: Zone = serde_json::from_str(zone_json.value())?;
            drop(zone_json);

            // Use YYYYMMDDNN format, incrementing NN
            let today = Utc::now().format("%Y%m%d").to_string();
            let today_base: u32 = format!("{today}00").parse().unwrap_or(zone.soa.serial + 1);

            if zone.soa.serial >= today_base {
                zone.soa.serial += 1;
            } else {
                zone.soa.serial = today_base;
            }
            zone.updated_at = Utc::now();

            let json = serde_json::to_string(&zone)?;
            zones.insert(id_str.as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Query records across all zones for a given FQDN and record type.
    /// The name is matched against "record.name.zone.name" or "@.zone.name" (zone apex).
    pub fn query_fqdn(&self, fqdn: &str, rtype: RecordType) -> Result<Vec<Record>> {
        let fqdn = fqdn.trim_end_matches('.');
        let zones = self.list_zones()?;

        for zone in &zones {
            let zone_name = zone.name.trim_end_matches('.');
            if fqdn == zone_name {
                // Zone apex query
                return self.query_records(&zone.id, "@", rtype);
            } else if let Some(prefix) = fqdn.strip_suffix(&format!(".{zone_name}")) {
                return self.query_records(&zone.id, prefix, rtype);
            }
        }

        Ok(Vec::new())
    }

    // --- Replication operations ---

    /// Insert or update a zone by ID. Updates the name index if the zone name changed.
    pub fn upsert_zone(&self, zone: &Zone) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = zone.id.to_string();
            let mut zones = write_txn.open_table(ZONES_TABLE)?;
            let mut name_idx = write_txn.open_table(ZONE_NAME_INDEX)?;

            // If zone already exists, clean up old name index entry
            if let Some(existing_json) = zones.get(id_str.as_str())? {
                let existing: Zone = serde_json::from_str(existing_json.value())?;
                drop(existing_json);
                if existing.name != zone.name {
                    name_idx.remove(existing.name.as_str())?;
                }
            }

            let json = serde_json::to_string(zone)?;
            zones.insert(id_str.as_str(), json.as_str())?;
            name_idx.insert(zone.name.as_str(), id_str.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Atomically delete all records for a zone and insert new ones.
    pub fn replace_zone_records(&self, zone_id: &Uuid, records: &[Record]) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let mut records_table = write_txn.open_table(RECORDS_TABLE)?;
            let mut by_zone = write_txn.open_table(RECORDS_BY_ZONE)?;

            // Delete existing records for this zone
            let prefix = format!("{zone_id}:");
            let mut to_delete = Vec::new();
            let iter = by_zone.iter()?;
            for entry in iter {
                let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
                let key = entry.0.value().to_string();
                if key.starts_with(&prefix) {
                    let record_ids: Vec<String> = entry
                        .1
                        .value()
                        .split(',')
                        .map(|s| s.to_string())
                        .collect();
                    to_delete.push((key, record_ids));
                }
            }

            for (index_key, record_ids) in to_delete {
                by_zone.remove(index_key.as_str())?;
                for rid in record_ids {
                    records_table.remove(rid.as_str())?;
                }
            }

            // Insert new records
            for record in records {
                let id_str = record.id.to_string();
                let json = serde_json::to_string(record)?;
                records_table.insert(id_str.as_str(), json.as_str())?;

                let index_key = format!(
                    "{}:{}:{}",
                    record.zone_id,
                    record.name,
                    record.data.record_type()
                );

                let new_val = match by_zone.get(index_key.as_str())? {
                    Some(v) => format!("{},{}", v.value(), id_str),
                    None => id_str,
                };
                by_zone.insert(index_key.as_str(), new_val.as_str())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Set or update replication metadata for a zone.
    pub fn set_replication_meta(&self, meta: &ReplicationMeta) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = meta.zone_id.to_string();
            let json = serde_json::to_string(meta)?;
            let mut table = write_txn.open_table(REPLICATION_META_TABLE)?;
            table.insert(id_str.as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Get replication metadata for a zone.
    pub fn get_replication_meta(&self, zone_id: &Uuid) -> Result<Option<ReplicationMeta>> {
        let read_txn = self.inner.begin_read()?;
        let table = read_txn.open_table(REPLICATION_META_TABLE)?;
        let id_str = zone_id.to_string();
        match table.get(id_str.as_str())? {
            Some(v) => {
                let meta: ReplicationMeta = serde_json::from_str(v.value())?;
                Ok(Some(meta))
            }
            None => Ok(None),
        }
    }

    /// List all replication metadata entries.
    pub fn list_replication_meta(&self) -> Result<Vec<ReplicationMeta>> {
        let read_txn = self.inner.begin_read()?;
        let table = read_txn.open_table(REPLICATION_META_TABLE)?;
        let mut result = Vec::new();
        let iter = table.iter()?;
        for entry in iter {
            let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
            let meta: ReplicationMeta = serde_json::from_str(entry.1.value())?;
            result.push(meta);
        }
        Ok(result)
    }

    /// Delete replication metadata for a zone.
    pub fn delete_replication_meta(&self, zone_id: &Uuid) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = zone_id.to_string();
            let mut table = write_txn.open_table(REPLICATION_META_TABLE)?;
            table.remove(id_str.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Get all zones replicated from a specific peer.
    pub fn get_zones_for_peer(&self, peer_id: &str) -> Result<Vec<ReplicationMeta>> {
        let all = self.list_replication_meta()?;
        Ok(all
            .into_iter()
            .filter(|m| m.source_peer_id == peer_id)
            .collect())
    }

    /// Delete a replicated zone and its metadata.
    pub fn delete_replicated_zone(&self, zone_id: &Uuid) -> Result<()> {
        self.delete_zone(zone_id)?;
        self.delete_replication_meta(zone_id)?;
        Ok(())
    }

    // --- IPAM operations ---

    pub fn create_ipam_allocation(&self, alloc: &IpamAllocation) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = alloc.id.to_string();
            let json = serde_json::to_string(alloc)?;
            let mut table = write_txn.open_table(IPAM_TABLE)?;
            table.insert(id_str.as_str(), json.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_ipam_allocations(&self) -> Result<Vec<IpamAllocation>> {
        let read_txn = self.inner.begin_read()?;
        let table = read_txn.open_table(IPAM_TABLE)?;
        let mut result = Vec::new();
        let iter = table.iter()?;
        for entry in iter {
            let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
            let alloc: IpamAllocation = serde_json::from_str(entry.1.value())?;
            result.push(alloc);
        }
        Ok(result)
    }

    pub fn delete_ipam_allocation(&self, id: &Uuid) -> Result<()> {
        let write_txn = self.inner.begin_write()?;
        {
            let id_str = id.to_string();
            let mut table = write_txn.open_table(IPAM_TABLE)?;
            table.remove(id_str.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn find_ipam_by_container(&self, container: &str) -> Result<Option<IpamAllocation>> {
        let read_txn = self.inner.begin_read()?;
        let table = read_txn.open_table(IPAM_TABLE)?;
        let iter = table.iter()?;
        for entry in iter {
            let entry = entry.map_err(|e| Error::Database(e.to_string()))?;
            let alloc: IpamAllocation = serde_json::from_str(entry.1.value())?;
            if alloc.container == container {
                return Ok(Some(alloc));
            }
        }
        Ok(None)
    }

    /// Get the zone that owns a given FQDN
    pub fn find_zone_for_fqdn(&self, fqdn: &str) -> Result<Option<Zone>> {
        let fqdn = fqdn.trim_end_matches('.');
        let zones = self.list_zones()?;

        // Find the most specific (longest) matching zone
        let mut best: Option<&Zone> = None;
        for zone in &zones {
            let zone_name = zone.name.trim_end_matches('.');
            if (fqdn == zone_name || fqdn.ends_with(&format!(".{zone_name}")))
                && (best.is_none() || zone.name.len() > best.unwrap().name.len())
            {
                best = Some(zone);
            }
        }

        Ok(best.cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RecordData, SoaData};
    use tempfile::TempDir;

    fn test_db() -> (Db, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("test.redb")).unwrap();
        (db, dir)
    }

    fn make_zone(name: &str) -> Zone {
        Zone {
            id: Uuid::new_v4(),
            name: name.to_string(),
            soa: SoaData {
                mname: format!("ns1.{name}"),
                rname: format!("admin.{name}"),
                serial: 2024010100,
                refresh: 3600,
                retry: 900,
                expire: 604800,
                minimum: 300,
            },
            default_ttl: 300,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_record(zone_id: Uuid, name: &str, data: RecordData) -> Record {
        Record {
            id: Uuid::new_v4(),
            zone_id,
            name: name.to_string(),
            ttl: 300,
            data,
            enabled: true,
            health_check: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_zone_crud() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");

        db.create_zone("example.com", &zone).unwrap();

        let fetched = db.get_zone_by_name("example.com").unwrap().unwrap();
        assert_eq!(fetched.id, zone.id);
        assert_eq!(fetched.name, "example.com");

        let zones = db.list_zones().unwrap();
        assert_eq!(zones.len(), 1);

        db.delete_zone(&zone.id).unwrap();
        assert!(db.get_zone_by_name("example.com").unwrap().is_none());
    }

    #[test]
    fn test_duplicate_zone() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");
        db.create_zone("example.com", &zone).unwrap();

        let zone2 = make_zone("example.com");
        assert!(db.create_zone("example.com", &zone2).is_err());
    }

    #[test]
    fn test_record_crud() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");
        db.create_zone("example.com", &zone).unwrap();

        let record = make_record(
            zone.id,
            "www",
            RecordData::A("10.0.0.1".parse().unwrap()),
        );
        db.create_record(&record).unwrap();

        let fetched = db.get_record(&record.id).unwrap().unwrap();
        assert_eq!(fetched.name, "www");

        let results = db
            .query_records(&zone.id, "www", RecordType::A)
            .unwrap();
        assert_eq!(results.len(), 1);

        db.delete_record(&record.id).unwrap();
        assert!(db.get_record(&record.id).unwrap().is_none());
    }

    #[test]
    fn test_query_fqdn() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");
        db.create_zone("example.com", &zone).unwrap();

        let record = make_record(
            zone.id,
            "www",
            RecordData::A("10.0.0.1".parse().unwrap()),
        );
        db.create_record(&record).unwrap();

        let apex_record = make_record(
            zone.id,
            "@",
            RecordData::A("10.0.0.2".parse().unwrap()),
        );
        db.create_record(&apex_record).unwrap();

        let results = db.query_fqdn("www.example.com", RecordType::A).unwrap();
        assert_eq!(results.len(), 1);

        let results = db.query_fqdn("example.com", RecordType::A).unwrap();
        assert_eq!(results.len(), 1);

        let results = db.query_fqdn("nope.example.com", RecordType::A).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_upsert_zone() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");

        // Insert via upsert
        db.upsert_zone(&zone).unwrap();
        let fetched = db.get_zone_by_name("example.com").unwrap().unwrap();
        assert_eq!(fetched.id, zone.id);

        // Update via upsert (same name)
        let mut updated = zone.clone();
        updated.default_ttl = 600;
        db.upsert_zone(&updated).unwrap();
        let fetched = db.get_zone_by_name("example.com").unwrap().unwrap();
        assert_eq!(fetched.default_ttl, 600);

        // Update via upsert (name change)
        let mut renamed = updated.clone();
        renamed.name = "renamed.com".to_string();
        db.upsert_zone(&renamed).unwrap();
        assert!(db.get_zone_by_name("example.com").unwrap().is_none());
        let fetched = db.get_zone_by_name("renamed.com").unwrap().unwrap();
        assert_eq!(fetched.id, zone.id);
    }

    #[test]
    fn test_replace_zone_records() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");
        db.create_zone("example.com", &zone).unwrap();

        // Create initial records
        let r1 = make_record(zone.id, "www", RecordData::A("10.0.0.1".parse().unwrap()));
        let r2 = make_record(zone.id, "mail", RecordData::A("10.0.0.2".parse().unwrap()));
        db.create_record(&r1).unwrap();
        db.create_record(&r2).unwrap();
        assert_eq!(db.list_records(&zone.id).unwrap().len(), 2);

        // Replace with new set
        let r3 = make_record(zone.id, "api", RecordData::A("10.0.0.3".parse().unwrap()));
        db.replace_zone_records(&zone.id, &[r3.clone()]).unwrap();
        let records = db.list_records(&zone.id).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "api");

        // Old records should be gone
        assert!(db.get_record(&r1.id).unwrap().is_none());
        assert!(db.get_record(&r2.id).unwrap().is_none());
    }

    #[test]
    fn test_replication_meta_crud() {
        use crate::types::ReplicationMeta;

        let (db, _dir) = test_db();
        let zone_id = Uuid::new_v4();

        let meta = ReplicationMeta {
            zone_id,
            zone_name: "example.com".to_string(),
            source_peer_id: "peer-1".to_string(),
            last_synced: Utc::now(),
            source_serial: 2024010100,
        };

        // Set
        db.set_replication_meta(&meta).unwrap();

        // Get
        let fetched = db.get_replication_meta(&zone_id).unwrap().unwrap();
        assert_eq!(fetched.zone_name, "example.com");
        assert_eq!(fetched.source_peer_id, "peer-1");

        // List
        let all = db.list_replication_meta().unwrap();
        assert_eq!(all.len(), 1);

        // Get zones for peer
        let peer_zones = db.get_zones_for_peer("peer-1").unwrap();
        assert_eq!(peer_zones.len(), 1);
        let other_zones = db.get_zones_for_peer("peer-2").unwrap();
        assert_eq!(other_zones.len(), 0);

        // Delete
        db.delete_replication_meta(&zone_id).unwrap();
        assert!(db.get_replication_meta(&zone_id).unwrap().is_none());
    }

    #[test]
    fn test_increment_soa_serial() {
        let (db, _dir) = test_db();
        let zone = make_zone("example.com");
        db.create_zone("example.com", &zone).unwrap();

        let before = db.get_zone(&zone.id).unwrap().unwrap().soa.serial;
        db.increment_soa_serial(&zone.id).unwrap();
        let after = db.get_zone(&zone.id).unwrap().unwrap().soa.serial;
        assert!(after > before);
    }
}
