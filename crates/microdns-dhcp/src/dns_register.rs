use chrono::Utc;
use microdns_core::db::Db;
use microdns_core::error::Result;
use microdns_core::types::{Record, RecordData};
use std::net::{Ipv4Addr, Ipv6Addr};
use tracing::{debug, warn};
use uuid::Uuid;

/// Auto-registers DNS records (A/AAAA/PTR) when DHCP leases are created.
pub struct DnsRegistrar {
    db: Db,
    forward_zone: String,
    reverse_zone_v4: String,
    _reverse_zone_v6: String,
    default_ttl: u32,
}

impl DnsRegistrar {
    pub fn new(
        db: Db,
        forward_zone: &str,
        reverse_zone_v4: &str,
        reverse_zone_v6: &str,
        default_ttl: u32,
    ) -> Self {
        Self {
            db,
            forward_zone: forward_zone.to_string(),
            reverse_zone_v4: reverse_zone_v4.to_string(),
            _reverse_zone_v6: reverse_zone_v6.to_string(),
            default_ttl,
        }
    }

    /// Register forward (A) and reverse (PTR) records for a DHCPv4 lease.
    pub fn register_v4(&self, hostname: &str, ip: Ipv4Addr) -> Result<()> {
        let zone = match self.db.get_zone_by_name(&self.forward_zone)? {
            Some(z) => z,
            None => {
                warn!(
                    "DNS registration: forward zone {} not found, skipping",
                    self.forward_zone
                );
                return Ok(());
            }
        };

        // Create A record
        let a_record = Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: hostname.to_string(),
            ttl: self.default_ttl,
            data: RecordData::A(ip),
            enabled: true,
            health_check: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.db.create_record(&a_record)?;
        debug!("registered A record: {hostname}.{} -> {ip}", self.forward_zone);

        // Create PTR record in reverse zone
        if let Some(rev_zone) = self.db.get_zone_by_name(&self.reverse_zone_v4)? {
            let octets = ip.octets();
            // For a /24, the PTR name is just the last octet
            let ptr_name = octets[3].to_string();
            let ptr_target = format!("{hostname}.{}.", self.forward_zone);

            let ptr_record = Record {
                id: Uuid::new_v4(),
                zone_id: rev_zone.id,
                name: ptr_name.clone(),
                ttl: self.default_ttl,
                data: RecordData::PTR(ptr_target.clone()),
                enabled: true,
                health_check: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            self.db.create_record(&ptr_record)?;
            debug!("registered PTR record: {ptr_name}.{} -> {ptr_target}", self.reverse_zone_v4);
        }

        self.db.increment_soa_serial(&zone.id)?;
        Ok(())
    }

    /// Register forward (AAAA) and reverse (PTR) records for a DHCPv6 lease.
    pub fn register_v6(&self, hostname: &str, ip: Ipv6Addr) -> Result<()> {
        let zone = match self.db.get_zone_by_name(&self.forward_zone)? {
            Some(z) => z,
            None => {
                warn!(
                    "DNS registration: forward zone {} not found, skipping",
                    self.forward_zone
                );
                return Ok(());
            }
        };

        // Create AAAA record
        let aaaa_record = Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: hostname.to_string(),
            ttl: self.default_ttl,
            data: RecordData::AAAA(ip),
            enabled: true,
            health_check: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.db.create_record(&aaaa_record)?;
        debug!(
            "registered AAAA record: {hostname}.{} -> {ip}",
            self.forward_zone
        );

        self.db.increment_soa_serial(&zone.id)?;
        Ok(())
    }

    /// Remove DNS records for a released lease.
    pub fn unregister(&self, hostname: &str) -> Result<()> {
        let zone = match self.db.get_zone_by_name(&self.forward_zone)? {
            Some(z) => z,
            None => return Ok(()),
        };

        // Find and remove A/AAAA records for this hostname
        let records = self.db.list_records(&zone.id)?;
        for record in &records {
            if record.name == hostname {
                self.db.delete_record(&record.id)?;
                debug!("unregistered DNS record: {hostname}.{}", self.forward_zone);
            }
        }

        Ok(())
    }
}
