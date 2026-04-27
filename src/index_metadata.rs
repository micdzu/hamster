// index_metadata.rs – The hamster’s little notebook.
// It remembers the last time it saw every acorn, so it doesn’t
// re‑chew unchanged ones.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// A single acorn’s vital stats.
#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct FileMeta {
    pub mtime: u64,
    pub flags: String,
    #[serde(default)] // for old notebooks that don’t have this yet
    pub message_id: String, // the acorn’s eternal name
}

/// The whole notebook, mapping file paths to their stats.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct IndexMeta {
    pub last_indexed: u64,
    pub file_states: HashMap<String, FileMeta>,
}

impl IndexMeta {
    /// Read the notebook from the burrow.
    pub fn load(index_path: &Path) -> Result<Self> {
        let meta_path = index_path.join("hamster_meta.json");
        if meta_path.exists() {
            let data = std::fs::read_to_string(&meta_path).with_context(|| {
                format!("Failed to read hamster_meta.json from {:?}", meta_path)
            })?;
            serde_json::from_str(&data)
                .context("Failed to parse hamster_meta.json - index may be corrupted")
        } else {
            Ok(IndexMeta::default())
        }
    }

    /// Write the notebook back to the burrow.
    pub fn save(&self, index_path: &Path) -> Result<()> {
        let meta_path = index_path.join("hamster_meta.json");
        let data = serde_json::to_string_pretty(self).context("Failed to serialize metadata")?;
        std::fs::write(&meta_path, data)
            .with_context(|| format!("Failed to write hamster_meta.json to {:?}", meta_path))?;
        Ok(())
    }

    /// Should the hamster re‑chew this acorn?
    /// Returns true if the file is new, modified, or its flags changed.
    pub fn needs_reindex(&self, path: &str, mtime: u64, flags: &str) -> bool {
        match self.file_states.get(path) {
            None => true,
            Some(prev) => prev.mtime != mtime || prev.flags != flags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_needs_reindex_new_file() {
        let meta = IndexMeta::default();
        assert!(meta.needs_reindex("/some/path", 100, "S"));
    }

    #[test]
    fn test_needs_reindex_unchanged() {
        let mut meta = IndexMeta::default();
        meta.file_states.insert(
            "/some/path".to_string(),
            FileMeta {
                mtime: 100,
                flags: "S".to_string(),
                message_id: "abc123".to_string(),
            },
        );
        assert!(!meta.needs_reindex("/some/path", 100, "S"));
    }

    #[test]
    fn test_needs_reindex_mtime_changed() {
        let mut meta = IndexMeta::default();
        meta.file_states.insert(
            "/some/path".to_string(),
            FileMeta {
                mtime: 100,
                flags: "S".to_string(),
                message_id: String::new(),
            },
        );
        assert!(meta.needs_reindex("/some/path", 101, "S"));
    }

    #[test]
    fn test_needs_reindex_flags_changed_same_mtime() {
        let mut meta = IndexMeta::default();
        meta.file_states.insert(
            "/maildir/cur/msg:2,".to_string(),
            FileMeta {
                mtime: 100,
                flags: String::new(),
                message_id: String::new(),
            },
        );
        assert!(meta.needs_reindex("/maildir/cur/msg:2,S", 100, "S"));
    }

    #[test]
    fn test_meta_save_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut meta = IndexMeta::default();
        meta.last_indexed = 12345;
        meta.file_states.insert(
            "/mail/cur/msg:2,S".to_string(),
            FileMeta {
                mtime: 999,
                flags: "S".to_string(),
                message_id: "hamster-001".to_string(),
            },
        );

        meta.save(dir.path()).expect("save failed");
        let loaded = IndexMeta::load(dir.path()).expect("load failed");

        assert_eq!(loaded.last_indexed, 12345);
        let state = loaded.file_states.get("/mail/cur/msg:2,S").unwrap();
        assert_eq!(state.mtime, 999);
        assert_eq!(state.flags, "S");
        assert_eq!(state.message_id, "hamster-001");
    }

    #[test]
    fn test_meta_uses_hamster_meta_filename() {
        let dir = TempDir::new().unwrap();
        let meta = IndexMeta::default();
        meta.save(dir.path()).expect("save failed");
        assert!(dir.path().join("hamster_meta.json").exists());
        assert!(!dir.path().join("meta.json").exists());
    }
}
