use hickory_proto::rr::LowerName;
use microdns_core::db::Db;
use std::str::FromStr;

/// Manages the set of zones this server is authoritative for.
/// Checks against the database to determine if we should answer queries.
pub struct ZoneCatalog {
    db: Db,
}

impl ZoneCatalog {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Check if this server is authoritative for the given name
    pub fn is_authoritative(&self, name: &LowerName) -> bool {
        let fqdn = name.to_string();
        let fqdn = fqdn.trim_end_matches('.');

        matches!(self.db.find_zone_for_fqdn(fqdn), Ok(Some(_)))
    }

    /// Get zone names from the database
    pub fn zone_names(&self) -> Vec<LowerName> {
        match self.db.list_zones() {
            Ok(zones) => zones
                .iter()
                .filter_map(|z| {
                    let fqdn = if z.name.ends_with('.') {
                        z.name.clone()
                    } else {
                        format!("{}.", z.name)
                    };
                    LowerName::from_str(&fqdn).ok()
                })
                .collect(),
            Err(e) => {
                tracing::error!("failed to list zones: {e}");
                Vec::new()
            }
        }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }
}
