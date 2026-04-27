// address_cache.rs – The Hamster's Little Black Book
//
// Once upon a time, `hamster address` would trudge through every single
// document in the entire index on every single invocation. For a mailbox
// of 50 000 messages that is 50 000 reasons to feel bad about yourself.
// For 200 000 messages, do the maths. The hamster didn't like this either.
//
// The fix: a slim JSON notebook called `hamster_addresses.json` that lives
// next to the index. It is rebuilt automatically at the end of each
// `hamster index` run and then just *sits there*, ready for instant lookup.
//
// Format: a flat map from normalised (lowercase, trimmed) email address
// to a small record containing the best-known display name and a frequency
// counter. Frequent contacts float to the top of search results because
// the hamster rewards loyalty.
//
// The cache is a full rebuild each time (not incremental). This sounds
// wasteful, but it's just one Tantivy segment scan reading two string
// fields – no disk I/O, no MIME parsing. It completes in milliseconds
// even for very large mailboxes. The hamster has done the maths.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ── Data types ───────────────────────────────────────────────────────────────

// Everything the hamster remembers about one email address.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AddressCacheEntry {
    // Best known display name, e.g. "Hamster McWheel".
    // Set the first time a non-empty name is seen; not overwritten later.
    // If you've been sending email to "boss@example.com" for years and
    // the first email had no name, well. That's on you.
    pub name: String,

    // How many emails this address has appeared in (from + to combined).
    // Used for ranking: the more the hamster has seen someone, the higher
    // they float in fuzzy-search results.
    pub count: usize,
}

// The complete address book. Serialised to JSON and stored next to the
// Tantivy index as `hamster_addresses.json`.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AddressCache {
    // Keyed by normalised email address (lowercase, trimmed).
    pub entries: HashMap<String, AddressCacheEntry>,
}

// ── Persistence ──────────────────────────────────────────────────────────────

impl AddressCache {
    // Load the address book from the burrow.
    //
    // Returns an empty cache gracefully when the file does not yet exist –
    // the hamster is patient; it will build the book on the next index run.
    pub fn load(index_dir: &Path) -> Result<Self> {
        let path = Self::cache_path(index_dir);
        if !path.exists() {
            // No book yet. Could be a fresh install or the hamster is just
            // waking up. Either way, return empty rather than blowing up.
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read address cache at {:?}", path))?;
        serde_json::from_str(&data).with_context(|| {
            format!(
                "Address cache at {:?} appears to be corrupt. \
                 Run 'hamster index' to rebuild it. \
                 (The hamster is embarrassed but it happens.)",
                path
            )
        })
    }

    // Write the address book back to the burrow, overwriting the previous
    // version atomically. Well, as atomically as `std::fs::write` gets.
    pub fn save(&self, index_dir: &Path) -> Result<()> {
        let path = Self::cache_path(index_dir);
        let data = serde_json::to_string(self).context(
            "Failed to serialise address cache. \
             This really should not happen. The hamster is confused and mildly offended.",
        )?;
        std::fs::write(&path, data)
            .with_context(|| format!("Failed to write address cache to {:?}", path))?;
        Ok(())
    }
    // Returns the most frequent contact and count of addresses without names.
    pub fn top_contact(&self) -> (Option<(String, usize)>, usize) {
        let mut nameless = 0usize;

        let top = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                if entry.name.is_empty() {
                    nameless += 1;
                }
                true
            })
            .max_by_key(|(_, entry)| entry.count)
            .map(|(email, entry)| (email.clone(), entry.count));

        (top, nameless)
    }

    // Canonical location of the address notebook, relative to the index dir.
    fn cache_path(index_dir: &Path) -> std::path::PathBuf {
        index_dir.join("hamster_addresses.json")
    }

    // ── Ingestion ────────────────────────────────────────────────────────

    // Record a sighting of `email` (with optional display `name`).
    //
    // Silently ignores anything that doesn't look like an email address –
    // the hamster has seen some truly creative header values and has
    // developed strong opinions about what counts as an address.
    pub fn ingest(&mut self, email: &str, name: &str) {
        let email = email.trim().to_lowercase();

        // The @ check is load-bearing. You'd be surprised what arrives in
        // a From: header if you let your guard down.
        if email.is_empty() || !email.contains('@') {
            return;
        }

        let entry = self
            .entries
            .entry(email)
            .or_insert_with(AddressCacheEntry::default);

        entry.count += 1;

        // First non-empty name wins. The hamster doesn't forget the name
        // of a friend, and it doesn't let later (potentially worse) sightings
        // overwrite a good one. "John Doe" beats "" beats nothing.
        if !name.is_empty() && entry.name.is_empty() {
            entry.name = name.trim().to_string();
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_ingest_basic() {
        let mut cache = AddressCache::default();
        cache.ingest("hamster@wheel.com", "Hamster McWheel");
        let entry = cache.entries.get("hamster@wheel.com").unwrap();
        assert_eq!(entry.name, "Hamster McWheel");
        assert_eq!(entry.count, 1);
    }

    #[test]
    fn test_ingest_normalises_email() {
        let mut cache = AddressCache::default();
        cache.ingest("HAMSTER@WHEEL.COM", "");
        assert!(cache.entries.contains_key("hamster@wheel.com"));
    }

    #[test]
    fn test_ingest_ignores_non_email() {
        let mut cache = AddressCache::default();
        cache.ingest("not-an-email", "Some Name");
        cache.ingest("", "Empty");
        cache.ingest("   ", "Whitespace");
        assert!(
            cache.entries.is_empty(),
            "Non-emails should be silently ignored"
        );
    }

    #[test]
    fn test_ingest_increments_count() {
        let mut cache = AddressCache::default();
        cache.ingest("a@b.com", "");
        cache.ingest("a@b.com", "");
        cache.ingest("a@b.com", "");
        assert_eq!(cache.entries["a@b.com"].count, 3);
    }

    #[test]
    fn test_ingest_first_name_wins() {
        let mut cache = AddressCache::default();
        cache.ingest("a@b.com", "First Name");
        cache.ingest("a@b.com", "Should Not Overwrite");
        assert_eq!(cache.entries["a@b.com"].name, "First Name");
    }

    #[test]
    fn test_ingest_empty_name_then_real_name() {
        let mut cache = AddressCache::default();
        cache.ingest("a@b.com", "");
        cache.ingest("a@b.com", "Real Name");
        // Empty name doesn't win – the first *non-empty* name does.
        assert_eq!(cache.entries["a@b.com"].name, "Real Name");
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut cache = AddressCache::default();
        cache.ingest("wheel@hamster.dev", "Wheel Runner");
        cache.ingest("wheel@hamster.dev", "");
        cache.ingest("acorn@hamster.dev", "Acorn Collector");

        cache.save(dir.path()).expect("save failed");

        let loaded = AddressCache::load(dir.path()).expect("load failed");
        let wheel = loaded.entries.get("wheel@hamster.dev").unwrap();
        assert_eq!(wheel.name, "Wheel Runner");
        assert_eq!(wheel.count, 2);

        let acorn = loaded.entries.get("acorn@hamster.dev").unwrap();
        assert_eq!(acorn.name, "Acorn Collector");
        assert_eq!(acorn.count, 1);
    }

    #[test]
    fn test_load_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let cache = AddressCache::load(dir.path()).expect("should return empty, not error");
        assert!(cache.entries.is_empty());
    }
}
