use chrono::Utc;
use microdns_core::db::Db;
use microdns_core::error::Result;
use microdns_core::reverse;
use microdns_core::types::{Record, RecordData, RecordType};
use std::net::{Ipv4Addr, Ipv6Addr};
use tracing::{debug, warn};
use uuid::Uuid;

/// Auto-registers DNS records (A/AAAA/PTR) when DHCP leases are created.
pub struct DnsRegistrar {
    db: Db,
    forward_zone: String,
    default_ttl: u32,
}

impl DnsRegistrar {
    pub fn new(
        db: Db,
        forward_zone: &str,
        _reverse_zone_v4: &str,
        _reverse_zone_v6: &str,
        default_ttl: u32,
    ) -> Self {
        Self {
            db,
            forward_zone: forward_zone.to_string(),
            default_ttl,
        }
    }

    /// Strip domain suffix from hostname if already present (e.g. "cap01.gw.lo" → "cap01").
    /// Prevents double-suffix records like "cap01.gw.lo.gw.lo".
    fn sanitize_hostname<'a>(&self, hostname: &'a str) -> &'a str {
        let suffix_dot = format!(".{}.", self.forward_zone);
        let suffix = format!(".{}", self.forward_zone);
        if let Some(stripped) = hostname.strip_suffix(&suffix_dot) {
            stripped
        } else if let Some(stripped) = hostname.strip_suffix(&suffix) {
            stripped
        } else {
            hostname
        }
    }

    /// Register forward (A) and reverse (PTR) records for a DHCPv4 lease.
    /// Deduplicates: skips if identical record exists, updates IP if hostname moved.
    /// Auto-creates reverse zone if it doesn't exist.
    pub fn register_v4(&self, hostname: &str, ip: Ipv4Addr) -> Result<()> {
        let hostname = self.sanitize_hostname(hostname);
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

        let desired = RecordData::A(ip);

        // Check existing A records for this hostname
        let existing = self.db.query_records(&zone.id, hostname, RecordType::A)?;

        // If exact record already exists, skip
        if existing.iter().any(|r| r.data == desired) {
            debug!("A record already exists: {hostname}.{} -> {ip}", self.forward_zone);
            return Ok(());
        }

        // Remove stale DHCP-registered A records for this hostname (IP changed)
        for rec in &existing {
            self.db.delete_record(&rec.id)?;
            debug!("removed stale A record: {hostname}.{} -> {:?}", self.forward_zone, rec.data);
        }

        // Create A record
        let a_record = Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: hostname.to_string(),
            ttl: self.default_ttl,
            data: desired,
            enabled: true,
            health_check: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.db.create_record(&a_record)?;
        debug!("registered A record: {hostname}.{} -> {ip}", self.forward_zone);

        // Auto-sync reverse PTR (creates reverse zone if needed)
        if let Err(e) = reverse::sync_ptr_for_a(&self.db, hostname, &self.forward_zone, ip, self.default_ttl) {
            warn!("reverse PTR sync failed for {hostname} -> {ip}: {e}");
        }

        self.db.increment_soa_serial(&zone.id)?;
        Ok(())
    }

    /// Register forward (AAAA) and reverse (PTR) records for a DHCPv6 lease.
    /// Auto-creates reverse zone if it doesn't exist.
    pub fn register_v6(&self, hostname: &str, ip: Ipv6Addr) -> Result<()> {
        let hostname = self.sanitize_hostname(hostname);
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

        let desired = RecordData::AAAA(ip);
        let existing = self.db.query_records(&zone.id, hostname, RecordType::AAAA)?;

        if existing.iter().any(|r| r.data == desired) {
            debug!("AAAA record already exists: {hostname}.{} -> {ip}", self.forward_zone);
            return Ok(());
        }

        for rec in &existing {
            self.db.delete_record(&rec.id)?;
        }

        let aaaa_record = Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: hostname.to_string(),
            ttl: self.default_ttl,
            data: desired,
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

        // Auto-sync reverse PTR (creates reverse zone if needed)
        if let Err(e) = reverse::sync_ptr_for_aaaa(&self.db, hostname, &self.forward_zone, ip, self.default_ttl) {
            warn!("reverse PTR sync failed for {hostname} -> {ip}: {e}");
        }

        self.db.increment_soa_serial(&zone.id)?;
        Ok(())
    }

    /// Remove DNS records for a released lease.
    pub fn unregister(&self, hostname: &str) -> Result<()> {
        let hostname = self.sanitize_hostname(hostname);
        let zone = match self.db.get_zone_by_name(&self.forward_zone)? {
            Some(z) => z,
            None => return Ok(()),
        };

        // Find and remove A/AAAA records for this hostname, cleaning up reverse PTRs
        let records = self.db.list_records(&zone.id)?;
        for record in &records {
            if record.name == hostname {
                // Clean up reverse PTR before deleting forward record
                if let Err(e) = reverse::delete_reverse_record(
                    &self.db,
                    &record.name,
                    &self.forward_zone,
                    &record.data,
                ) {
                    warn!("reverse PTR cleanup failed for {hostname}: {e}");
                }
                self.db.delete_record(&record.id)?;
                debug!("unregistered DNS record: {hostname}.{}", self.forward_zone);
            }
        }

        Ok(())
    }
}
