use dashmap::DashMap;
use hickory_proto::op::Message;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A cached DNS response with expiry tracking.
struct CacheEntry {
    /// Serialized DNS response message (without the original query ID).
    response_bytes: Vec<u8>,
    /// When this entry was inserted.
    inserted_at: Instant,
    /// TTL from the response records (minimum across all answer records).
    ttl: Duration,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.inserted_at.elapsed() >= self.ttl
    }
}

/// Thread-safe DNS response cache with TTL expiration and size limits.
pub struct DnsCache {
    entries: DashMap<CacheKey, CacheEntry>,
    max_size: usize,
    hit_count: AtomicUsize,
    miss_count: AtomicUsize,
}

/// Cache key: (lowercased qname, qtype, qclass)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub name: String,
    pub rtype: u16,
    pub rclass: u16,
}

impl CacheKey {
    pub fn from_query(name: &str, rtype: u16, rclass: u16) -> Self {
        Self {
            name: name.to_lowercase(),
            rtype,
            rclass,
        }
    }
}

impl DnsCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: DashMap::with_capacity(max_size.min(4096)),
            max_size,
            hit_count: AtomicUsize::new(0),
            miss_count: AtomicUsize::new(0),
        }
    }

    /// Look up a cached response. Returns the response bytes if found and not expired.
    pub fn get(&self, key: &CacheKey) -> Option<Vec<u8>> {
        let entry = match self.entries.get(key) {
            Some(e) => e,
            None => {
                self.miss_count.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        if entry.is_expired() {
            drop(entry);
            self.entries.remove(key);
            self.miss_count.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        self.hit_count.fetch_add(1, Ordering::Relaxed);
        Some(entry.response_bytes.clone())
    }

    /// Insert a response into the cache.
    /// The `min_ttl` is the minimum TTL across all answer records.
    pub fn insert(&self, key: CacheKey, response_bytes: Vec<u8>, ttl_secs: u32) {
        if ttl_secs == 0 {
            return;
        }

        // Evict expired entries if we're at capacity
        if self.entries.len() >= self.max_size {
            self.evict_expired();
        }

        // If still at capacity, skip insertion (simple eviction policy)
        if self.entries.len() >= self.max_size {
            return;
        }

        self.entries.insert(
            key,
            CacheEntry {
                response_bytes,
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(ttl_secs as u64),
            },
        );
    }

    /// Remove expired entries.
    fn evict_expired(&self) {
        self.entries.retain(|_, entry| !entry.is_expired());
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn hit_count(&self) -> usize {
        self.hit_count.load(Ordering::Relaxed)
    }

    pub fn miss_count(&self) -> usize {
        self.miss_count.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.entries.clear();
        self.hit_count.store(0, Ordering::Relaxed);
        self.miss_count.store(0, Ordering::Relaxed);
    }
}

/// Extract the minimum TTL from a DNS response message's answer section.
pub fn min_ttl_from_response(msg: &Message) -> u32 {
    msg.answers()
        .iter()
        .map(|r| r.ttl())
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_insert_and_get() {
        let cache = DnsCache::new(100);
        let key = CacheKey::from_query("example.com", 1, 1);
        let data = vec![1, 2, 3, 4];

        cache.insert(key.clone(), data.clone(), 300);
        let result = cache.get(&key);
        assert_eq!(result, Some(data));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.hit_count(), 1);
    }

    #[test]
    fn test_cache_miss() {
        let cache = DnsCache::new(100);
        let key = CacheKey::from_query("example.com", 1, 1);
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.miss_count(), 1);
    }

    #[test]
    fn test_cache_zero_ttl_not_cached() {
        let cache = DnsCache::new(100);
        let key = CacheKey::from_query("example.com", 1, 1);
        cache.insert(key.clone(), vec![1, 2, 3], 0);
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_max_size() {
        let cache = DnsCache::new(2);
        cache.insert(
            CacheKey::from_query("a.com", 1, 1),
            vec![1],
            300,
        );
        cache.insert(
            CacheKey::from_query("b.com", 1, 1),
            vec![2],
            300,
        );
        // At capacity - this should be silently dropped
        cache.insert(
            CacheKey::from_query("c.com", 1, 1),
            vec![3],
            300,
        );
        assert!(cache.len() <= 2);
    }

    #[test]
    fn test_cache_clear() {
        let cache = DnsCache::new(100);
        cache.insert(
            CacheKey::from_query("a.com", 1, 1),
            vec![1],
            300,
        );
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.hit_count(), 0);
    }
}
