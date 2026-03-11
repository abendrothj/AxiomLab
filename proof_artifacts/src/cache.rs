use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofCacheEntry {
    pub key: String,
    pub value_hash: String,
    pub unix_secs: u64,
    pub theorem_count: Option<u32>,
    pub sorry_count: Option<u32>,
    pub status: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProofCache {
    pub entries: BTreeMap<String, ProofCacheEntry>,
}

impl ProofCache {
    pub fn load(path: &Path) -> Result<Self, CacheError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, path: &Path) -> Result<(), CacheError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw)?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&ProofCacheEntry> {
        self.entries.get(key)
    }

    pub fn upsert(&mut self, entry: ProofCacheEntry) {
        self.entries.insert(entry.key.clone(), entry);
    }

    pub fn unchanged(&self, key: &str, value_hash: &str) -> bool {
        self.get(key)
            .map(|entry| entry.value_hash == value_hash)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_unchanged() {
        let mut c = ProofCache::default();
        c.upsert(ProofCacheEntry {
            key: "k".into(),
            value_hash: "h".into(),
            unix_secs: 1,
            theorem_count: Some(1),
            sorry_count: Some(0),
            status: Some("passed".into()),
        });
        assert!(c.unchanged("k", "h"));
        assert!(!c.unchanged("k", "other"));
    }
}
