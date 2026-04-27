// index.rs – The Grand Hamster Sweep
//
// This is where the hamster scampers through your Maildir, sniffs every
// file, and decides which seeds need re-chewing. It does everything
// transactionally: no notebook entry is updated until the seed is safely
// tucked into the granary. If a read fails, the hamster leaves the old
// entry alone – no lost acorns.
//
// New addition: after each successful index run (i.e., when new messages
// were actually processed), the address cache is rebuilt. This is a single
// Tantivy segment scan reading just the `from` and `to` fields, so it's
// fast. The result is a small JSON file that `hamster address` can read
// without touching the index at all. Much faster, very satisfying.

use anyhow::{Context, Result};
use colored::Colorize;
use crossbeam::channel::bounded;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::thread;
use tantivy::schema::Value;
use walkdir::WalkDir;

use crate::address::extract_contacts;
use crate::address_cache::AddressCache;
use crate::index_access::IndexAccess;
use crate::index_core::{HamsterIndex, IndexJob};
use crate::index_maildir::{parse_maildir_flags, should_skip_entry};
use crate::index_metadata::{FileMeta, IndexMeta};
use crate::setup::HamsterConfig;

// How many seeds the hamster stuffs into its cheek pouches before
// committing to the granary. Larger = fewer commits = faster overall,
// but more memory used while the batch is in flight.
const COMMIT_BATCH_SIZE: usize = 1000;

// ── The big sweep ────────────────────────────────────────────────────────────

pub fn run(config: &HamsterConfig, maildir_override: Option<String>) -> Result<()> {
    let maildir = maildir_override.unwrap_or_else(|| config.maildir.clone());
    let idx_path = std::sync::Arc::new(config.index_dir.clone());

    println!("{} Indexing {} ...", "🐹".green(), maildir.cyan().bold());
    println!("    Index location: {:?}", idx_path);

    let mut meta = IndexMeta::load(&idx_path).context("Failed to load index metadata")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("System time before epoch? The hamster is temporal confused.")?
        .as_secs();

    // ── Walk the Maildir, collect every file with mtime + flags ──────────
    //
    // We do one full walk up front so the writer thread and the deletion
    // pass both have access to the complete set of known-present paths.
    let all_files: Vec<(String, u64, HashSet<char>, String)> = WalkDir::new(&maildir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !should_skip_entry(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let p = e.path().to_string_lossy().to_string();
            let meta = e.metadata().ok()?;
            let mtime = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            let suffix = crate::index_maildir::flag_suffix(&p);
            let flags = parse_maildir_flags(&p);
            Some((p, mtime, flags, suffix))
        })
        .collect();

    // ── Decide what needs re-chewing ────────────────────────────────────
    //
    // Files whose mtime and flag suffix haven't changed are fine – we skip them.
    // Files that are new or changed go into the job queue.
    let mut jobs: Vec<IndexJob> = Vec::new();
    let mut files_to_update: Vec<(String, u64, String, String)> = Vec::new();

    for (path, mtime, flags, suffix) in &all_files {
        let needs = meta.needs_reindex(path, *mtime, suffix);

        if !needs {
            // Acorn unchanged – skip it.
            continue;
        }

        // Needs re-chewing. Try to read it; skip on failure (don't lose the
        // old index entry just because the filesystem is being difficult).
        let raw = match fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                log::warn!(
                    "Read failed for {}: {}. \
                     Keeping old index entry – the hamster is cautious.",
                    path,
                    e
                );
                continue;
            }
        };

        // Every acorn needs a name. If the email lacks a Message-ID header,
        // we fall back to the file path. Ugly but unambiguous.
        let mid = HamsterIndex::extract_message_id(&raw);
        let mid = if mid.is_empty() { path.clone() } else { mid };

        jobs.push(IndexJob {
            path: path.clone(),
            flags: flags.clone(),
            raw_data: raw,
            message_id: mid.clone(),
        });
        files_to_update.push((path.clone(), *mtime, suffix.clone(), mid));
    }

    let total_jobs = jobs.len();

    // If everything is already up to date and nothing was deleted,
    // the hamster can take a well-earned nap.
    if total_jobs == 0 && meta.file_states.len() == all_files.len() {
        println!(
            "{}",
            "✨ Index is already up to date. Hamster naps.".green()
        );
        return Ok(());
    }

    // ── Spawn the writer thread ──────────────────────────────────────────
    //
    // We send jobs through a bounded channel. The writer thread chews them
    // one by one, committing every COMMIT_BATCH_SIZE seeds. When the sender
    // is dropped (all jobs sent), the thread drains the channel and returns
    // the message_ids of everything it successfully indexed.
    let (tx, rx) = bounded::<IndexJob>(10_000);
    let idx_path_thread = idx_path.clone();
    let writer_handle = thread::spawn(move || {
        let pb = ProgressBar::new(total_jobs as u64);
        let style = ProgressStyle::default_bar()
            .template(
                "{spinner:.green} {msg:.bold} [{elapsed_precise}] \
                 [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("##-");
        pb.set_style(style);
        pb.set_message("🐹 Gnawing on emails...");
        let mut index = match HamsterIndex::new(&*idx_path_thread) {
            Ok(i) => i,
            Err(e) => {
                log::error!("Failed to open index for writing: {}", e);
                return Vec::new();
            }
        };

        let mut succeeded: Vec<String> = Vec::new();
        let mut count = 0usize;

        while let Ok(job) = rx.recv() {
            match index.index_message(&job.path, &job.raw_data, &job.flags, &job.message_id) {
                Ok(_) => {
                    succeeded.push(job.message_id);
                    count += 1;
                    pb.inc(1);
                    if count % 50 == 0 {
                        pb.set_message(format!("🐹 Gnawed {} seeds", count));
                    }
                    if count % COMMIT_BATCH_SIZE == 0 {
                        if let Err(e) = index.commit() {
                            log::error!("Mid-batch commit error: {}", e);
                        }
                    }
                }
                Err(e) => {
                    // Log and skip. One bad acorn doesn't spoil the granary.
                    log::info!("Skipping {}: {}", job.path, e);
                }
            }
        }

        if let Err(e) = index.commit() {
            log::error!("Final commit failed: {}", e);
        }

        pb.finish_with_message(format!(
            "✨ Chewed {} new seeds. Hamster rests.",
            succeeded.len()
        ));
        succeeded
    });

    // ── Feed the jobs through the tunnel ────────────────────────────────
    for job in jobs {
        tx.send(job).ok();
    }
    drop(tx); // close channel so the writer thread knows we're done

    let succeeded_ids = writer_handle.join().map_err(|e| {
        let msg = e
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| e.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        anyhow::anyhow!("Indexer thread panicked: {}", msg)
    })?;

    // ── Update metadata only for seeds that made it ──────────────────────
    //
    // If indexing failed for a file, we keep the old metadata entry so we
    // try again next run instead of silently dropping the document.
    let mut new_states: HashMap<String, FileMeta> = HashMap::new();
    for (path, mtime, suffix, mid) in &files_to_update {
        if succeeded_ids.contains(mid) {
            new_states.insert(
                path.clone(),
                FileMeta {
                    mtime: *mtime,
                    flags: suffix.clone(),
                    message_id: mid.clone(),
                },
            );
        } else if let Some(old) = meta.file_states.get(path.as_str()) {
            // Failed to re-index: preserve old state so we try again next run.
            new_states.insert(path.clone(), old.clone());
        }
    }

    // ── Prune stale notebook entries & calculate deletions ───────────────
    //
    // We compare the old metadata paths against the current filesystem walk.
    // If a path vanished from disk, and its message_id wasn't successfully
    // re-indexed somewhere else (e.g. moved folders), it goes into the bin.
    let existing_paths: HashSet<String> = all_files.iter().map(|(p, ..)| p.clone()).collect();
    let mut deleted_ids: HashSet<String> = HashSet::new();

    for (path, old_state) in &meta.file_states {
        if !existing_paths.contains(path) && !old_state.message_id.is_empty() {
            // If it moved folders, it succeeded under a new path. Don't delete it!
            if !succeeded_ids.contains(&old_state.message_id) {
                deleted_ids.insert(old_state.message_id.clone());
            }
        }
    }

    // Apply the new states and prune missing paths from the notebook.
    meta.file_states.retain(|p, _| existing_paths.contains(p));
    for (p, state) in new_states {
        meta.file_states.insert(p, state);
    }
    meta.last_indexed = now;
    meta.save(&*idx_path)
        .with_context(|| format!("Failed to save metadata to {:?}", idx_path))?;

    // ── Purge deleted acorns from the granary ────────────────────────────
    //
    // We only open a writer if we actually have something to delete.
    // This avoids a full index scan – the hamster knows exactly who moved out.
    let deleted = if deleted_ids.is_empty() {
        0
    } else {
        let mut index = HamsterIndex::new(&*idx_path).with_context(|| {
            format!("Failed to reopen index at {:?} for deletion pass", idx_path)
        })?;
        let count = index
            .delete_missing(&deleted_ids)
            .context("Failed to delete missing documents")?;
        index.commit().context("Failed to commit deletions")?;
        count
    };

    if deleted > 0 {
        println!("{} Removed {} deleted messages.", "🧹".yellow(), deleted);
    }

    log::info!(
        "Indexing finished: {} files processed, {} seeded, {} deleted.",
        total_jobs,
        succeeded_ids.len(),
        deleted
    );

    // ── Rebuild address cache ────────────────────────────────────────────
    //
    // Only bother if we actually processed new messages. If nothing changed,
    // the existing cache is already correct and we can skip the scan.
    //
    // The rebuild is a single pass over the index reading only `from` and
    // `to` fields. For 100 000 messages this takes well under a second.
    // The hamster considers this acceptable.
    if !succeeded_ids.is_empty() {
        println!("🐹 Updating address book...");
        match rebuild_address_cache(&config.index_dir) {
            Ok(count) => println!("📇 Address book updated ({} contacts).", count),
            Err(e) => {
                // Non-fatal. The address command will just fall back to the
                // previous cache (or an empty one). Warn and move on.
                log::warn!("Failed to rebuild address cache: {}", e);
                println!(
                    "⚠  Could not update address book: {}. \
                     Run `hamster index` again if address lookup seems stale.",
                    e
                );
            }
        }
    }

    Ok(())
}

// ── Address cache rebuild ────────────────────────────────────────────────────
//
// Full rebuild every time: simpler than incremental and correct.
// We scan every document in the index, pull out the stored `from` and `to`
// fields, and hand each address to the cache's `ingest` method.
//
// Returns the number of unique contacts found (for the status message).
fn rebuild_address_cache(index_dir: &std::path::Path) -> Result<usize> {
    let ia = IndexAccess::open(index_dir)?;
    let searcher = ia.searcher()?;
    let mut cache = AddressCache::default();

    let all = searcher
        .search(
            &tantivy::query::AllQuery,
            &tantivy::collector::DocSetCollector,
        )
        .context("Failed to scan index for address rebuild")?;

    for doc_addr in all {
        let doc = searcher
            .doc::<tantivy::TantivyDocument>(doc_addr)
            .context("Failed to read document during address rebuild")?;

        // Pull from and to, then hand each parsed address to the cache.
        for field in [ia.from, ia.to] {
            for val in doc.get_all(field).flat_map(|v| v.as_str()) {
                for (name, email) in extract_contacts(val) {
                    cache.ingest(&email, &name);
                }
            }
        }
    }

    let count = cache.entries.len();
    cache.save(index_dir)?;
    Ok(count)
}
