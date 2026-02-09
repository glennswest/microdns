use std::collections::HashSet;
use std::net::Ipv4Addr;

/// Manages a pool of IPv4 addresses for DHCP allocation.
pub struct Ipv4Pool {
    pub range_start: Ipv4Addr,
    pub range_end: Ipv4Addr,
    pub subnet_mask: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub dns_servers: Vec<Ipv4Addr>,
    pub domain: String,
    pub lease_time_secs: u32,
    /// Addresses currently allocated
    allocated: HashSet<Ipv4Addr>,
}

impl Ipv4Pool {
    pub fn new(
        range_start: Ipv4Addr,
        range_end: Ipv4Addr,
        subnet_mask: Ipv4Addr,
        gateway: Ipv4Addr,
        dns_servers: Vec<Ipv4Addr>,
        domain: String,
        lease_time_secs: u32,
    ) -> Self {
        Self {
            range_start,
            range_end,
            subnet_mask,
            gateway,
            dns_servers,
            domain,
            lease_time_secs,
            allocated: HashSet::new(),
        }
    }

    /// Allocate the next available IP address.
    pub fn allocate(&mut self) -> Option<Ipv4Addr> {
        let start: u32 = self.range_start.into();
        let end: u32 = self.range_end.into();

        for ip_u32 in start..=end {
            let ip = Ipv4Addr::from(ip_u32);
            if !self.allocated.contains(&ip) {
                self.allocated.insert(ip);
                return Some(ip);
            }
        }

        None // Pool exhausted
    }

    /// Try to allocate a specific IP address.
    pub fn allocate_specific(&mut self, addr: Ipv4Addr) -> bool {
        if !self.contains(addr) {
            return false;
        }
        if self.allocated.contains(&addr) {
            return false;
        }
        self.allocated.insert(addr);
        true
    }

    /// Release an allocated IP address.
    pub fn release(&mut self, addr: &Ipv4Addr) {
        self.allocated.remove(addr);
    }

    /// Check if an address is within this pool's range.
    pub fn contains(&self, addr: Ipv4Addr) -> bool {
        let start: u32 = self.range_start.into();
        let end: u32 = self.range_end.into();
        let ip: u32 = addr.into();
        ip >= start && ip <= end
    }

    /// Mark an address as allocated (e.g., from restored leases).
    pub fn mark_allocated(&mut self, addr: Ipv4Addr) {
        if self.contains(addr) {
            self.allocated.insert(addr);
        }
    }

    pub fn available_count(&self) -> u32 {
        let start: u32 = self.range_start.into();
        let end: u32 = self.range_end.into();
        let total = end - start + 1;
        total - self.allocated.len() as u32
    }

    pub fn total_count(&self) -> u32 {
        let start: u32 = self.range_start.into();
        let end: u32 = self.range_end.into();
        end - start + 1
    }
}

/// Parse a subnet string (e.g., "10.0.10.0/24") to get the subnet mask.
pub fn subnet_mask_from_prefix(prefix_len: u8) -> Ipv4Addr {
    if prefix_len == 0 {
        return Ipv4Addr::new(0, 0, 0, 0);
    }
    if prefix_len >= 32 {
        return Ipv4Addr::new(255, 255, 255, 255);
    }
    let mask: u32 = !0u32 << (32 - prefix_len);
    Ipv4Addr::from(mask)
}

/// Extract prefix length from a subnet string like "10.0.10.0/24".
pub fn prefix_len_from_subnet(subnet: &str) -> Option<u8> {
    subnet
        .split('/')
        .nth(1)
        .and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_allocation() {
        let mut pool = Ipv4Pool::new(
            "10.0.10.100".parse().unwrap(),
            "10.0.10.102".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "10.0.10.1".parse().unwrap(),
            vec!["10.0.10.2".parse().unwrap()],
            "example.com".to_string(),
            3600,
        );

        assert_eq!(pool.total_count(), 3);
        assert_eq!(pool.available_count(), 3);

        let ip1 = pool.allocate().unwrap();
        assert_eq!(ip1, "10.0.10.100".parse::<Ipv4Addr>().unwrap());

        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip2, "10.0.10.101".parse::<Ipv4Addr>().unwrap());

        let ip3 = pool.allocate().unwrap();
        assert_eq!(ip3, "10.0.10.102".parse::<Ipv4Addr>().unwrap());

        // Pool exhausted
        assert!(pool.allocate().is_none());

        // Release and reallocate
        pool.release(&ip2);
        assert_eq!(pool.available_count(), 1);
        let ip4 = pool.allocate().unwrap();
        assert_eq!(ip4, ip2);
    }

    #[test]
    fn test_allocate_specific() {
        let mut pool = Ipv4Pool::new(
            "10.0.10.100".parse().unwrap(),
            "10.0.10.200".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "10.0.10.1".parse().unwrap(),
            vec![],
            "example.com".to_string(),
            3600,
        );

        assert!(pool.allocate_specific("10.0.10.150".parse().unwrap()));
        assert!(!pool.allocate_specific("10.0.10.150".parse().unwrap())); // Already allocated
        assert!(!pool.allocate_specific("10.0.10.50".parse().unwrap())); // Out of range
    }

    #[test]
    fn test_subnet_mask() {
        assert_eq!(subnet_mask_from_prefix(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(subnet_mask_from_prefix(16), Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(subnet_mask_from_prefix(8), Ipv4Addr::new(255, 0, 0, 0));
        assert_eq!(subnet_mask_from_prefix(32), Ipv4Addr::new(255, 255, 255, 255));
    }
}
