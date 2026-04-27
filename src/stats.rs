// stats.rs – The Hamster's Census
//
// The hamster takes stock of its empire. This module gathers statistics
// about the index, the address book, and tag usage, then presents them
// in a clear, colorful format.
//
// All statistics are computed from existing data sources:
//   - Index stats come from the Tantivy reader
//   - Address stats come from the AddressCache
//   - Last indexed time comes from the IndexMeta
//   - Tag stats are computed by scanning stored fields
//
// The hamster does not create or modify anything here. It simply observes.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use colored::Colorize;
use serde_json;
use std::collections::HashMap;
use tantivy::collector::DocSetCollector;
use tantivy::schema::Value;

use crate::address_cache::AddressCache;
use crate::index_access::IndexAccess;
use crate::setup::HamsterConfig;

// A census record for the hamster's domain.
#[derive(Debug, Default)]
pub struct HamsterStats {
    // Total messages indexed in the Tantivy index.
    pub indexed_messages: u64,

    // Size of the index directory on disk (bytes).
    pub index_size_bytes: u64,

    // Number of unique contacts in the address book.
    pub unique_addresses: usize,

    // Most frequent contact (email, count).
    pub top_contact: Option<(String, usize)>,

    // Unix timestamp of the last successful index run.
    pub last_indexed: Option<u64>,

    // Total number of files in the Maildir (as tracked in meta).
    #[allow(dead_code)]
    pub tracked_files: usize,

    // Tag frequency counts.
    pub tag_counts: Vec<(String, usize)>,

    // Email addresses with no associated name.
    pub nameless_addresses: usize,
}

impl HamsterStats {
    // Gather all statistics for the given configuration.
    pub fn gather(config: &HamsterConfig) -> Result<Self> {
        let ia =
            IndexAccess::open(&config.index_dir).context("Failed to open index for statistics")?;

        let cache =
            AddressCache::load(&config.index_dir).context("Failed to load address cache")?;

        let searcher = ia.searcher().context("Failed to create searcher")?;

        // Count all documents in the index
        let indexed_messages = searcher
            .search(&tantivy::query::AllQuery, &DocSetCollector)
            .context("Failed to count indexed messages")?
            .len() as u64;

        // Compute index directory size
        let index_size_bytes = compute_dir_size(&config.index_dir).unwrap_or(0);

        // Find top contact
        let (top_contact, nameless_addresses) = cache.top_contact();

        // Gather tag statistics by scanning documents
        let tag_counts = gather_tag_stats(&searcher, ia.tags);

        // Load last indexed time from meta (best effort)
        let last_indexed = load_last_indexed(&config.index_dir);

        Ok(HamsterStats {
            indexed_messages,
            index_size_bytes,
            unique_addresses: cache.entries.len(),
            top_contact,
            last_indexed,
            tracked_files: 0, // Would need meta loading
            tag_counts,
            nameless_addresses,
        })
    }

    // Format index size as human-readable string.
    pub fn formatted_size(&self) -> String {
        format_bytes(self.index_size_bytes)
    }

    // Format last indexed time as human-readable string.
    pub fn formatted_last_indexed(&self) -> String {
        match self.last_indexed {
            Some(ts) => {
                let dt = Local.timestamp_opt(ts as i64, 0).single();
                match dt {
                    Some(dt) => {
                        let now = Local::now();
                        let delta = now.signed_duration_since(dt);
                        if delta.num_days() > 365 {
                            format!(
                                "{} ({} years ago)",
                                dt.format("%Y-%m-%d"),
                                delta.num_days() / 365
                            )
                        } else if delta.num_days() > 30 {
                            format!(
                                "{} ({} months ago)",
                                dt.format("%Y-%m-%d"),
                                delta.num_days() / 30
                            )
                        } else if delta.num_days() > 0 {
                            let d = delta.num_days();
                            format!(
                                "{} ({} day{} ago)",
                                dt.format("%Y-%m-%d"),
                                d,
                                if d == 1 { "" } else { "s" }
                            )
                        } else if delta.num_hours() > 0 {
                            let h = delta.num_hours();
                            format!(
                                "{} ({} hour{} ago)",
                                dt.format("%H:%M"),
                                h,
                                if h == 1 { "" } else { "s" }
                            )
                        } else {
                            format!("{} (just now)", dt.format("%H:%M"))
                        }
                    }
                    None => "unknown".to_string(),
                }
            }
            None => "never".to_string(),
        }
    }
}

// Compute the total size of a directory and its contents (bytes).
fn compute_dir_size(path: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                total += metadata.len();
            } else if metadata.is_dir() {
                total += compute_dir_size(&entry.path())?;
            }
        }
    }
    Ok(total)
}

// Format bytes as human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// Gather tag statistics by scanning stored fields in all documents.
fn gather_tag_stats(
    searcher: &tantivy::Searcher,
    tags_field: tantivy::schema::Field,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    let all_docs = searcher
        .search(&tantivy::query::AllQuery, &DocSetCollector)
        .unwrap_or_default();

    for doc_addr in all_docs {
        if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(doc_addr) {
            for val in doc.get_all(tags_field).flat_map(|v| v.as_str()) {
                *counts.entry(val.to_string()).or_insert(0) += 1;
            }
        }
    }

    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1)); // Descending by count
    sorted.truncate(20); // Top 20 only
    sorted
}

// Load last_indexed timestamp from index meta (best effort).
fn load_last_indexed(index_dir: &std::path::Path) -> Option<u64> {
    let meta_path = index_dir.join("hamster_meta.json");
    if meta_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&data) {
                return meta.get("last_indexed")?.as_u64();
            }
        }
    }
    None
}

// ── Public Entry Point ────────────────────────────────────────────────────────

pub fn run(config: &HamsterConfig) -> anyhow::Result<()> {
    print_header();

    let stats = HamsterStats::gather(config)?;

    print_index_section(&stats);
    print_contacts_section(&stats);
    print_tags_section(&stats);

    print_footer();
    Ok(())
}

fn print_header() {
    println!();
    println!("{}", "🐹 Hamster Census".green().bold());
    println!("{}", "─".repeat(50).dimmed());
    println!();
}

fn print_index_section(stats: &HamsterStats) {
    println!("{}", "📬 Index".cyan().bold());
    println!();
    println!(
        "  {} messages indexed",
        stats.indexed_messages.to_string().white().bold()
    );
    println!("  {} on disk", stats.formatted_size().white());
    println!("  {} last indexed", stats.formatted_last_indexed().yellow());
    println!();
}

fn print_contacts_section(stats: &HamsterStats) {
    println!("{}", "📇 Address Book".cyan().bold());
    println!();
    println!(
        "  {} unique contacts",
        stats.unique_addresses.to_string().white().bold()
    );

    if stats.nameless_addresses > 0 {
        println!(
            "  {} have no associated name",
            stats.nameless_addresses.to_string().yellow()
        );
    }

    if let Some((email, count)) = &stats.top_contact {
        println!();
        println!("  {} {}", "Most frequent:".dimmed(), email.white());
        println!("  {} {} emails", " ".repeat(15), count.to_string().green());
    }
    println!();
}

fn print_tags_section(stats: &HamsterStats) {
    if stats.tag_counts.is_empty() {
        return;
    }

    println!("{}", "🏷️  Tags".cyan().bold());
    println!();

    // Find max count for bar scaling
    let max_count = stats.tag_counts.first().map(|(_, c)| *c).unwrap_or(1);
    let bar_width = 30usize;

    for (tag, count) in &stats.tag_counts {
        let bar_len = (*count as f64 / max_count as f64 * bar_width as f64) as usize;
        let bar = "█".repeat(bar_len);
        println!(
            "  {:<20} {}{} {}",
            tag.magenta(),
            bar.green(),
            " ".repeat(bar_width - bar_len),
            count.to_string().dimmed()
        );
    }
    println!();
}

fn print_footer() {
    println!("{}", "─".repeat(50).dimmed());
    println!("Run `hamster index` to update the index.");
    println!("Run `hamster folder sync` to update tag assignments.");
    println!();
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes_bytes() {
        assert_eq!(format_bytes(100), "100 B");
    }

    #[test]
    fn test_format_bytes_kilobytes() {
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
    }

    #[test]
    fn test_format_bytes_megabytes() {
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50.0 MB");
    }

    #[test]
    fn test_format_bytes_gigabytes() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    }
}
