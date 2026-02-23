use std::collections::VecDeque;
use std::sync::Mutex;

/// A single captured log entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub module: String,
    pub message: String,
}

/// Thread-safe ring buffer that holds the most recent log entries.
pub struct LogBuffer {
    entries: Mutex<VecDeque<LogEntry>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Push a new entry, evicting the oldest if at capacity.
    pub fn push(&self, entry: LogEntry) {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Query entries with optional filtering.
    pub fn query(&self, limit: usize, level: Option<&str>, module: Option<&str>) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .rev()
            .filter(|e| {
                if let Some(lvl) = level {
                    if !e.level.eq_ignore_ascii_case(lvl) {
                        return false;
                    }
                }
                if let Some(m) = module {
                    if !e.module.contains(m) {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .cloned()
            .collect()
    }
}
