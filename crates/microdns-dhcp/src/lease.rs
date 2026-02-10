use chrono::Utc;
use microdns_core::db::Db;
use microdns_core::error::Result;
use microdns_core::types::{Lease, LeaseState};
use redb::{ReadableTable, TableDefinition};
use uuid::Uuid;

/// Leases table: lease_id -> Lease JSON
const LEASES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("leases");

/// MAC to lease index: mac_addr -> lease_id
const MAC_LEASE_INDEX: TableDefinition<&str, &str> = TableDefinition::new("mac_lease_index");

/// IP to lease index: ip_addr -> lease_id
const IP_LEASE_INDEX: TableDefinition<&str, &str> = TableDefinition::new("ip_lease_index");

/// Manages DHCP leases in redb.
pub struct LeaseManager {
    db: Db,
}

impl LeaseManager {
    pub fn new(db: Db) -> Self {
        // Ensure index tables exist
        if let Ok(write_txn) = db.raw().begin_write() {
            let _ = write_txn.open_table(MAC_LEASE_INDEX);
            let _ = write_txn.open_table(IP_LEASE_INDEX);
            let _ = write_txn.commit();
        }
        Self { db }
    }

    pub fn create_lease(
        &self,
        ip_addr: &str,
        mac_addr: &str,
        hostname: Option<&str>,
        lease_time_secs: u32,
        pool_id: &str,
    ) -> Result<Lease> {
        let now = Utc::now();
        let lease = Lease {
            id: Uuid::new_v4(),
            ip_addr: ip_addr.to_string(),
            mac_addr: mac_addr.to_string(),
            hostname: hostname.map(String::from),
            lease_start: now,
            lease_end: now + chrono::Duration::seconds(lease_time_secs as i64),
            pool_id: pool_id.to_string(),
            state: LeaseState::Active,
        };

        let write_txn = self.db.raw().begin_write()?;
        {
            let id_str = lease.id.to_string();
            let json = serde_json::to_string(&lease)?;

            let mut leases = write_txn.open_table(LEASES_TABLE)?;
            leases.insert(id_str.as_str(), json.as_str())?;

            let mut mac_idx = write_txn.open_table(MAC_LEASE_INDEX)?;
            mac_idx.insert(mac_addr, id_str.as_str())?;

            let mut ip_idx = write_txn.open_table(IP_LEASE_INDEX)?;
            ip_idx.insert(ip_addr, id_str.as_str())?;
        }
        write_txn.commit()?;

        Ok(lease)
    }

    pub fn find_lease_by_mac(&self, mac_addr: &str) -> Result<Option<Lease>> {
        let read_txn = self.db.raw().begin_read()?;
        let mac_idx = read_txn.open_table(MAC_LEASE_INDEX)?;

        let lease_id = match mac_idx.get(mac_addr)? {
            Some(v) => v.value().to_string(),
            None => return Ok(None),
        };

        let leases = read_txn.open_table(LEASES_TABLE)?;
        match leases.get(lease_id.as_str())? {
            Some(v) => {
                let lease: Lease = serde_json::from_str(v.value())?;
                if lease.state == LeaseState::Active && lease.lease_end > Utc::now() {
                    Ok(Some(lease))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    pub fn release_lease_by_mac(&self, mac_addr: &str) -> Result<()> {
        let write_txn = self.db.raw().begin_write()?;
        {
            let mac_idx = write_txn.open_table(MAC_LEASE_INDEX)?;
            let lease_id = match mac_idx.get(mac_addr)? {
                Some(v) => v.value().to_string(),
                None => return Ok(()),
            };
            drop(mac_idx);

            let mut leases = write_txn.open_table(LEASES_TABLE)?;
            let lease_json = leases
                .get(lease_id.as_str())?
                .map(|v| v.value().to_string());

            if let Some(json_str) = lease_json {
                let mut lease: Lease = serde_json::from_str(&json_str)?;
                lease.state = LeaseState::Released;
                let json = serde_json::to_string(&lease)?;
                leases.insert(lease_id.as_str(), json.as_str())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_active_leases(&self) -> Result<Vec<Lease>> {
        let read_txn = self.db.raw().begin_read()?;
        let leases = read_txn.open_table(LEASES_TABLE)?;
        let now = Utc::now();

        let mut result = Vec::new();
        let iter = leases.iter()?;
        for entry in iter {
            let entry = entry.map_err(|e| microdns_core::error::Error::Database(e.to_string()))?;
            let lease: Lease = serde_json::from_str(entry.1.value())?;
            if lease.state == LeaseState::Active && lease.lease_end > now {
                result.push(lease);
            }
        }

        Ok(result)
    }

    /// Delete leases that expired more than `retention` ago.
    /// Returns the number of purged leases.
    pub fn purge_expired_leases(&self, retention: chrono::Duration) -> Result<usize> {
        let cutoff = Utc::now() - retention;
        let write_txn = self.db.raw().begin_write()?;
        let count;
        {
            let mut leases = write_txn.open_table(LEASES_TABLE)?;
            let mut mac_idx = write_txn.open_table(MAC_LEASE_INDEX)?;
            let mut ip_idx = write_txn.open_table(IP_LEASE_INDEX)?;

            // Collect expired lease IDs
            let mut to_delete: Vec<(String, String, String)> = Vec::new();
            {
                let iter = leases.iter()?;
                for entry in iter {
                    let entry =
                        entry.map_err(|e| microdns_core::error::Error::Database(e.to_string()))?;
                    let lease: Lease = serde_json::from_str(entry.1.value())?;
                    if lease.lease_end < cutoff
                        && (lease.state == LeaseState::Released
                            || lease.state == LeaseState::Active)
                    {
                        to_delete.push((
                            entry.0.value().to_string(),
                            lease.mac_addr,
                            lease.ip_addr,
                        ));
                    }
                }
            }

            count = to_delete.len();
            for (id, mac, ip) in &to_delete {
                leases.remove(id.as_str())?;
                mac_idx.remove(mac.as_str())?;
                ip_idx.remove(ip.as_str())?;
            }
        }
        write_txn.commit()?;
        Ok(count)
    }

    pub fn db(&self) -> &Db {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (Db, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("test.redb")).unwrap();
        (db, dir)
    }

    #[test]
    fn test_lease_crud() {
        let (db, _dir) = test_db();
        let mgr = LeaseManager::new(db);

        let lease = mgr
            .create_lease("10.0.10.100", "aa:bb:cc:dd:ee:ff", Some("host1"), 3600, "pool1")
            .unwrap();

        assert_eq!(lease.ip_addr, "10.0.10.100");
        assert_eq!(lease.mac_addr, "aa:bb:cc:dd:ee:ff");

        let found = mgr.find_lease_by_mac("aa:bb:cc:dd:ee:ff").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().ip_addr, "10.0.10.100");

        let active = mgr.list_active_leases().unwrap();
        assert_eq!(active.len(), 1);

        mgr.release_lease_by_mac("aa:bb:cc:dd:ee:ff").unwrap();

        // After release, find should return None
        let found = mgr.find_lease_by_mac("aa:bb:cc:dd:ee:ff").unwrap();
        assert!(found.is_none());
    }
}
