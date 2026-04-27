// folder_tags.rs – Folder-to-Tag Reconciliation & Flag Sync
//
// This is where the hamster turns an ancient folder tree into a tidy
// set of tags, and also keeps the Maildir flags in harmony with those
// tags – without letting them fight over the same turf.
//
// Architecture note (post-renovation):
//
//   The `sync` function used to have two passes:
//     Pass 1: scan all Tantivy docs, compute tag diffs, collect paths.
//     Pass 2: for each path, re-read the .eml file, re-parse the email,
//             rebuild the Tantivy document from scratch.
//
//   Pass 2 was doing enormous amounts of unnecessary disk I/O. All those
//   fields (from, to, subject, body, date) are already STORED in Tantivy.
//   The hamster had already chewed these acorns once during indexing.
//
//   Now both passes are collapsed into one:
//     Pass 1: scan all Tantivy docs, compute tag diffs, collect StoredFields.
//     (no pass 2: write directly from StoredFields)
//
//   This eliminates all the fs::read and MessageParser calls from sync,
//   removes the batch-commit-with-the-wrong-counter bug, and makes the
//   whole operation dramatically faster for large mailboxes.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use tantivy::query::TermQuery;
use tantivy::schema::Value;
use tantivy::{TantivyDocument, Term};

use crate::diff;
use crate::index_access::{IndexAccess, StoredFields};
use crate::setup::{FolderRule, FolderTagsConfig, HamsterConfig};

// ── Compiled rules ────────────────────────────────────────────────────────────
//
// We compile glob patterns once and reuse them across thousands of
// documents. Compiling inside the hot loop would make the hamster cry.

pub struct CompiledRule {
    pub pattern: glob::Pattern,
    pub tags: Vec<String>,
    pub inherit: bool,
}

pub(crate) fn compile_rules(rules: &[FolderRule]) -> Result<Vec<CompiledRule>> {
    rules
        .iter()
        .map(|rule| {
            // Patterns are matched case-insensitively by lowercasing both.
            let pattern_lower = rule.pattern.to_lowercase();
            let pattern = glob::Pattern::new(&pattern_lower).map_err(|e| {
                anyhow::anyhow!(
                    "Invalid glob pattern '{}': {}. \
                     Even a hamster needs proper glob syntax.",
                    rule.pattern,
                    e
                )
            })?;
            Ok(CompiledRule {
                pattern,
                tags: rule.tags.clone(),
                inherit: rule.inherit,
            })
        })
        .collect()
}

// ── Helper: extract folder name from a full file path ────────────────────────
//
// Example: "/home/you/mail/INBOX/cur/1234.eml" with root "/home/you/mail"
// → "INBOX"
//
// Returns None if the path can't be stripped or doesn't have the expected
// depth (maildir_root / folder / {cur|new} / filename).

pub(crate) fn folder_from_path(path: &str, mail_root: &Path) -> Option<String> {
    let full = Path::new(path);
    let rel = full.strip_prefix(mail_root).ok()?;
    rel.parent()?
        .parent()
        .map(|p| p.to_string_lossy().to_string())
}

// ── Helper: determine the full set of "managed" tags ─────────────────────────
//
// The hamster only touches tags it is responsible for. User-applied tags
// (not in the managed set) are left strictly alone. This is the contract
// that makes manual tagging and folder-sync coexist peacefully.
//
// If `managed_tags` is explicitly configured, we use that. Otherwise, we
// infer it from the rules (all tags that appear in any rule, plus flag tags
// if sync_flags is enabled).

pub(crate) fn determine_managed_tags(cfg: &FolderTagsConfig) -> HashSet<String> {
    if !cfg.managed_tags.is_empty() {
        return cfg.managed_tags.iter().cloned().collect();
    }

    let mut managed = HashSet::new();

    for rule in &cfg.rules {
        for tag in &rule.tags {
            // Tags prefixed with `-` are forced-removes; the bare name is managed.
            let name = tag.strip_prefix('-').unwrap_or(tag.as_str());
            managed.insert(name.to_string());
        }
    }

    if cfg.sync_flags {
        for &(_, tag) in crate::index_maildir::FLAG_TO_TAG {
            managed.insert(tag.to_string());
        }
        managed.insert("unread".to_string());
    }

    managed
}

// ── Helper: merge Maildir flag tags into the desired-add/remove sets ──────────
//
// When sync_flags is enabled, Maildir flags (S = seen, F = flagged, etc.)
// have authority over the corresponding tags. This function adds/removes
// the appropriate tags based on what flags are actually present on disk.
//
// We deliberately skip tags that are already accounted for by folder rules –
// rules take precedence over flags for overlapping tags.

pub(crate) fn merge_flag_tags(
    path: &str,
    desired_add: &mut HashSet<String>,
    forced_remove: &mut HashSet<String>,
) {
    let flags = crate::index_maildir::parse_maildir_flags(path);

    for &(flag_char, tag) in crate::index_maildir::FLAG_TO_TAG {
        // Don't override a tag that folder rules already decided on.
        if desired_add.contains(tag) || forced_remove.contains(tag) {
            continue;
        }
        if flags.contains(&flag_char) {
            desired_add.insert(tag.to_string());
            forced_remove.remove(tag);
        } else {
            forced_remove.insert(tag.to_string());
            desired_add.remove(tag);
        }
    }

    // Handle `unread` separately: it is the *absence* of the `S` (Seen) flag.
    if !desired_add.contains("unread") && !forced_remove.contains("unread") {
        if flags.contains(&'S') {
            forced_remove.insert("unread".to_string());
            desired_add.remove("unread");
        } else {
            desired_add.insert("unread".to_string());
            forced_remove.remove("unread");
        }
    }
}

// ── Helper: compute desired tag sets for a given folder ──────────────────────

pub(crate) fn folder_tags_for_path(
    folder: &str,
    rules: &[CompiledRule],
) -> (HashSet<String>, HashSet<String>) {
    let mut add_set = HashSet::new();
    let mut remove_set = HashSet::new();

    let folder_lower = folder.to_lowercase();

    for rule in rules {
        let matches = rule.pattern.matches(&folder_lower);

        // `inherit` makes a rule apply to subfolders too. E.g. a rule for
        // "Archive" with inherit=true also matches "Archive/2024".
        let inherits = rule.inherit && {
            let base = rule
                .pattern
                .as_str()
                .trim_end_matches(|c: char| c == '*' || c == '?' || c == '/')
                .to_lowercase();
            !base.is_empty()
                && (folder_lower.starts_with(&format!("{}/", base)) || folder_lower == base)
        };

        if matches || inherits {
            for tag in &rule.tags {
                if let Some(stripped) = tag.strip_prefix('-') {
                    remove_set.insert(stripped.to_string());
                } else {
                    add_set.insert(tag.clone());
                }
            }
        }
    }

    (add_set, remove_set)
}

// ── Pending update (used inside sync) ────────────────────────────────────────
//
// We carry StoredFields through instead of re-reading files.
// Everything we need is already in Tantivy; no reason to go back to disk.

struct PendingUpdate {
    fields: StoredFields,
    new_tags: HashSet<String>,
}

// How many documents we write before issuing an intermediate commit.
// Larger = fewer commits = faster, but more RAM held in the writer buffer.
// 500 is a comfortable middle ground.
const SYNC_COMMIT_EVERY: usize = 500;

// ── Main sync command ─────────────────────────────────────────────────────────

pub fn sync(config: &HamsterConfig, dry_run: bool, quiet: bool) -> Result<()> {
    let tags_cfg = &config.folder_tags;
    if !tags_cfg.enabled {
        anyhow::bail!(
            "Folder tags are disabled in your config. \
             The hamster is taking a legally mandated nap. \
             Enable them in ~/.hamster.toml to proceed."
        );
    }

    let mail_root = PathBuf::from(&config.maildir);
    let compiled_rules = compile_rules(&tags_cfg.rules)?;
    let managed_tags = determine_managed_tags(tags_cfg);

    let ia = IndexAccess::open(&config.index_dir)?;
    let searcher = ia.searcher()?;

    let all_docs = searcher
        .search(
            &tantivy::query::AllQuery,
            &tantivy::collector::DocSetCollector,
        )
        .context("Failed to retrieve all documents for folder sync")?;

    let total = all_docs.len();

    let pb = if quiet {
        ProgressBar::hidden()
    } else {
        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] \
                     {bar:40.cyan/blue} {pos}/{len} messages",
                )?
                .progress_chars("#>-"),
        );
        pb
    };

    // ── Scan pass ────────────────────────────────────────────────────────
    //
    // For each document, compute the desired tag diff and collect the ones
    // that actually need updating. We carry StoredFields instead of paths
    // so the write pass never needs to touch disk.

    let mut pending: Vec<PendingUpdate> = Vec::new();
    let mut dry_run_count = 0usize;
    for (i, doc_addr) in all_docs.iter().enumerate() {
        pb.set_position(i as u64);

        let doc = searcher
            .doc::<TantivyDocument>(*doc_addr)
            .with_context(|| format!("Failed to read doc {:?} during folder sync", doc_addr))?;

        let path = doc
            .get_first(ia.path)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if path.is_empty() {
            continue;
        }

        let folder = match folder_from_path(path, &mail_root) {
            Some(f) => f,
            None => continue,
        };

        let (mut desired_add, mut forced_remove) = folder_tags_for_path(&folder, &compiled_rules);

        if config.folder_tags.sync_flags {
            merge_flag_tags(path, &mut desired_add, &mut forced_remove);
        }

        let current_tags: HashSet<String> = doc
            .get_all(ia.tags)
            .flat_map(|v| v.as_str())
            .map(String::from)
            .collect();

        let (to_add, to_remove) =
            diff::compute_tag_diff(&desired_add, &forced_remove, &current_tags, &managed_tags);

        // No diff → nothing to do. The hamster can skip this one.
        if to_add.is_empty() && to_remove.is_empty() {
            continue;
        }

        // Compute what the new tag set will look like.
        let mut new_tags = current_tags.clone();
        new_tags.extend(to_add.iter().cloned());
        for tag in &to_remove {
            new_tags.remove(tag);
        }

        if dry_run {
            dry_run_count += 1;
            // Dry-run: print what would happen, collect nothing.
            if !quiet {
                println!(
                    "{}: +[{}] -[{}] → [{}]",
                    path,
                    to_add.iter().cloned().collect::<Vec<_>>().join(", "),
                    to_remove.iter().cloned().collect::<Vec<_>>().join(", "),
                    new_tags.iter().cloned().collect::<Vec<_>>().join(", "),
                );
            }
        } else {
            // Lift the stored fields out of the Tantivy document right now,
            // while we have the searcher open. No file reads needed.
            pending.push(PendingUpdate {
                fields: StoredFields::from_doc(&doc, &ia),
                new_tags,
            });
        }
    }

    pb.finish();

    // ── Nothing to do? ───────────────────────────────────────────────────

    if dry_run {
        if pending.is_empty() {
            // In dry-run mode we didn't collect pending, so check diff count differently.
            // If we printed nothing and are quiet, just report.
        }
        println!(
            "Dry-run: {} message(s) would be updated. (No seeds harmed.)",
            dry_run_count
        );
        return Ok(());
    }

    if pending.is_empty() {
        println!(
            "✅ All folder tags and flags are already in sync. \
             The hamster can nap guilt-free."
        );
        return Ok(());
    }

    // ── Write pass ───────────────────────────────────────────────────────
    //
    // Open the writer once. For each pending update, delete the old document
    // by message_id, then add the new one with the updated tags.
    // Commit periodically to keep memory pressure reasonable.
    //
    // No fs::read. No MessageParser. The hamster is fast now.

    let total_pending = pending.len();
    let mut writer = ia.writer()?;

    for (i, update) in pending.iter().enumerate() {
        // Swap out the old document for the new one (keyed by message_id).
        writer.delete_term(Term::from_field_text(
            ia.message_id,
            &update.fields.message_id,
        ));

        let mut doc = TantivyDocument::new();
        update.fields.write_into(&mut doc, &ia);
        for tag in &update.new_tags {
            doc.add_text(ia.tags, tag);
        }
        writer.add_document(doc)?;

        // Intermediate commits keep the writer's in-memory buffer from
        // growing too large. The hamster has learned this lesson the hard way.
        if (i + 1) % SYNC_COMMIT_EVERY == 0 {
            writer.commit()?;
            if !quiet {
                println!("  🐹 Committed {}/{} updates...", i + 1, total_pending);
            }
        }
    }

    // Final commit for the last (possibly partial) batch.
    writer.commit()?;

    println!(
        "✅ Folder tags & flags synchronised: {} message(s) updated. \
         The hamster deserves a sunflower seed.",
        total_pending
    );

    Ok(())
}

// ── Explain command ───────────────────────────────────────────────────────────
//
// Shows exactly why a specific message has (or would get) the tags it has.
// Useful for debugging rules. The hamster believes in transparency.

pub fn explain(config: &HamsterConfig, file_path: &str) -> Result<()> {
    let tags_cfg = &config.folder_tags;
    if !tags_cfg.enabled {
        anyhow::bail!("Folder tags are disabled in config.");
    }

    let mail_root = PathBuf::from(&config.maildir);
    let compiled_rules = compile_rules(&tags_cfg.rules)?;
    let managed_tags = determine_managed_tags(tags_cfg);
    let ia = IndexAccess::open(&config.index_dir)?;
    let searcher = ia.searcher()?;

    let term = Term::from_field_text(ia.path, file_path);
    let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
    let top_docs = searcher.search(&query, &tantivy::collector::TopDocs::with_limit(1))?;

    let doc = match top_docs.first() {
        Some((_score, doc_addr)) => searcher.doc::<TantivyDocument>(*doc_addr)?,
        None => anyhow::bail!(
            "No message found in the index with path: {}\n\
             \n\
             Has it been indexed? Try running 'hamster index' first.",
            file_path
        ),
    };

    let path = doc
        .get_first(ia.path)
        .and_then(|v| v.as_str())
        .unwrap_or(file_path);

    let folder =
        folder_from_path(path, &mail_root).unwrap_or_else(|| "(unknown folder)".to_string());

    println!("📁 Folder: {}", folder);
    println!("📄 Path:   {}", path);

    let current_tags: HashSet<String> = doc
        .get_all(ia.tags)
        .flat_map(|v| v.as_str())
        .map(String::from)
        .collect();

    let mut sorted_current: Vec<&String> = current_tags.iter().collect();
    sorted_current.sort();
    println!(
        "\n🏷️  Current tags: [{}]",
        sorted_current
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let (mut desired_add, mut forced_remove) = folder_tags_for_path(&folder, &compiled_rules);

    if config.folder_tags.sync_flags {
        merge_flag_tags(path, &mut desired_add, &mut forced_remove);
    }

    let (to_add, to_remove) =
        diff::compute_tag_diff(&desired_add, &forced_remove, &current_tags, &managed_tags);

    println!("\n📋 Matched rules:");
    let folder_lower = folder.to_lowercase();
    for rule in &compiled_rules {
        let matches = rule.pattern.matches(&folder_lower);
        let inherits = rule.inherit && {
            let base = rule
                .pattern
                .as_str()
                .trim_end_matches(|c: char| c == '*' || c == '?' || c == '/')
                .to_lowercase();
            !base.is_empty()
                && (folder_lower.starts_with(&format!("{}/", base)) || folder_lower == base)
        };
        if matches || inherits {
            let kind = if matches { "exact" } else { "inherited" };
            println!(
                "  • '{}' ({}) → [{}]",
                rule.pattern.as_str(),
                kind,
                rule.tags.join(", ")
            );
        }
    }

    if config.folder_tags.sync_flags {
        let flags = crate::index_maildir::parse_maildir_flags(path);
        println!("\n🚩 Maildir flags: {:?}", flags);
    }

    let mut da: Vec<&String> = desired_add.iter().collect();
    da.sort();
    let mut dr: Vec<&String> = forced_remove.iter().collect();
    dr.sort();
    let mut mt: Vec<&String> = managed_tags.iter().collect();
    mt.sort();

    println!(
        "\n🎯 Desired add:    [{}]",
        da.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    );
    println!(
        "🎯 Desired remove: [{}]",
        dr.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    );
    println!(
        "🧰 Managed tags:   [{}]",
        mt.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    );

    if to_add.is_empty() && to_remove.is_empty() {
        println!("\n✅ Already in sync – no changes needed.");
    } else {
        println!("\n➕ Would add:    {}", to_add.join(", "));
        println!("➖ Would remove: {}", to_remove.join(", "));
    }

    Ok(())
}

// ── Sync-structure command ────────────────────────────────────────────────────
//
// Checks rules against the current folder layout, finds orphaned rules and
// untagged folders, and offers to fix them interactively.
// This function is only lightly modified – it manages config file I/O,
// not index I/O, so the StoredFields changes don't apply here.

use dialoguer::{theme::ColorfulTheme, Confirm, Input, MultiSelect, Select};

pub fn sync_structure(config: &HamsterConfig, dry_run: bool, quiet: bool) -> Result<()> {
    let mut config = config.clone();
    let mail_root = PathBuf::from(&config.maildir);
    let current_folders_rel = crate::index_maildir::discover_maildir_folders(&mail_root)?;
    let current_paths: HashSet<String> = current_folders_rel
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    let mut rules_changed = false;
    let mut new_rules = Vec::new();

    // Orphaned rules: patterns that no longer match any folder on disk.
    // The hamster treats a broken glob as an orphan (it can never match).
    let orphaned_rules: Vec<&FolderRule> = config
        .folder_tags
        .rules
        .iter()
        .filter(|rule| {
            !glob::Pattern::new(&rule.pattern)
                .map(|p| current_paths.iter().any(|f| p.matches(&f.to_lowercase())))
                .unwrap_or(false)
        })
        .collect();

    // Untagged folders: folders on disk that no rule covers.
    let untagged_folders: Vec<String> = current_paths
        .iter()
        .filter(|f| {
            !config.folder_tags.rules.iter().any(|rule| {
                glob::Pattern::new(&rule.pattern)
                    .map(|p| p.matches(&f.to_lowercase()))
                    .unwrap_or(false)
            })
        })
        .cloned()
        .collect();

    let theme = ColorfulTheme::default();

    // ── Handle orphaned rules ─────────────────────────────────────────────

    if !orphaned_rules.is_empty() {
        if quiet {
            println!(
                "Note: {} orphaned rule(s) found (no matching folder). \
                 Run without --quiet to fix interactively.",
                orphaned_rules.len()
            );
            new_rules.extend(config.folder_tags.rules.iter().cloned());
        } else {
            println!(
                "\n⚠️  {} rule(s) have no matching folder:",
                orphaned_rules.len()
            );
            for (idx, rule) in orphaned_rules.iter().enumerate() {
                println!(
                    "  {}. pattern: '{}' → tags: [{}]",
                    idx + 1,
                    rule.pattern,
                    rule.tags.join(", ")
                );
            }

            let choices = &["Delete rule", "Update pattern", "Keep as-is"];
            for &rule in &orphaned_rules {
                let selection = Select::with_theme(&theme)
                    .with_prompt(&format!("Rule '{}' →", rule.pattern))
                    .items(choices)
                    .default(0)
                    .interact()?;

                match selection {
                    0 => {
                        if !dry_run {
                            println!("  → Deleting rule '{}'", rule.pattern);
                            rules_changed = true;
                            // Don't push to new_rules – it's deleted.
                        } else {
                            println!("[dry-run] Would delete rule '{}'", rule.pattern);
                        }
                    }
                    1 => {
                        let suggestions = find_similar_folders(&rule.pattern, &current_paths, 5);
                        let new_pattern = if !suggestions.is_empty() {
                            let mut items: Vec<&str> =
                                suggestions.iter().map(|s| s.as_str()).collect();
                            items.push("<type a custom pattern>");
                            let sel = Select::with_theme(&theme)
                                .with_prompt("Pick a similar folder or enter custom:")
                                .items(&items)
                                .default(0)
                                .interact()?;
                            if sel == items.len() - 1 {
                                Input::<String>::with_theme(&theme)
                                    .with_prompt("New pattern")
                                    .default(rule.pattern.clone())
                                    .interact_text()?
                            } else {
                                suggestions[sel].clone()
                            }
                        } else {
                            Input::<String>::with_theme(&theme)
                                .with_prompt("New pattern")
                                .default(rule.pattern.clone())
                                .interact_text()?
                        };

                        if let Err(e) = glob::Pattern::new(&new_pattern) {
                            eprintln!(
                                "⚠️  '{}' is not a valid glob pattern: {}. \
                                 Keeping original.",
                                new_pattern, e
                            );
                            if !dry_run {
                                new_rules.push(rule.clone());
                            }
                            continue;
                        }

                        if !dry_run {
                            new_rules.push(FolderRule {
                                pattern: new_pattern.clone(),
                                tags: rule.tags.clone(),
                                inherit: rule.inherit,
                            });
                            rules_changed = true;
                            println!("  → Updated to '{}'", new_pattern);
                        } else {
                            println!(
                                "[dry-run] Would update '{}' → '{}'",
                                rule.pattern, new_pattern
                            );
                        }
                    }
                    _ => {
                        // Keep as-is.
                        if !dry_run {
                            new_rules.push(rule.clone());
                        }
                    }
                }
            }
        }
    } else {
        // No orphans – keep existing rules as the starting point for new_rules.
        new_rules.extend(config.folder_tags.rules.iter().cloned());
    }

    // ── Handle untagged folders ───────────────────────────────────────────

    if !untagged_folders.is_empty() {
        if quiet {
            println!(
                "Note: {} folder(s) have no matching rule. \
                 Run without --quiet to assign tags interactively.",
                untagged_folders.len()
            );
        } else {
            println!(
                "\n📁 {} folder(s) have no matching rule:",
                untagged_folders.len()
            );
            for (idx, folder) in untagged_folders.iter().enumerate().take(10) {
                println!("  {}. {}", idx + 1, folder);
            }
            if untagged_folders.len() > 10 {
                println!("  ... and {} more", untagged_folders.len() - 10);
            }

            if Confirm::with_theme(&theme)
                .with_prompt("Create rules for these folders?")
                .interact()?
            {
                for folder in &untagged_folders {
                    println!("\n--- New rule for: {} ---", folder);
                    let suggestions = suggest_tags_for_folder(folder);

                    let chosen_tags = if !suggestions.is_empty() {
                        let selections = MultiSelect::with_theme(&theme)
                            .with_prompt("Select suggested tags (space to toggle)")
                            .items(&suggestions)
                            .interact()?;
                        selections
                            .into_iter()
                            .map(|i| suggestions[i].clone())
                            .collect()
                    } else {
                        Vec::new()
                    };

                    let mut tags = chosen_tags;

                    if Confirm::with_theme(&theme)
                        .with_prompt("Add custom tags?")
                        .interact()?
                    {
                        let custom: String = Input::with_theme(&theme)
                            .with_prompt("Tags separated by spaces")
                            .interact_text()?;
                        let custom_tags: Vec<String> =
                            custom.split_whitespace().map(String::from).collect();
                        if let Err(e) = crate::validation::validate_tags(&custom_tags) {
                            eprintln!("⚠️  Invalid tag(s): {}. Skipping custom tags.", e);
                        } else {
                            tags.extend(custom_tags);
                        }
                    }

                    if tags.is_empty() {
                        println!("  (no tags – skipping this folder)");
                        continue;
                    }

                    if let Err(e) = crate::validation::validate_tags(&tags) {
                        eprintln!("⚠️  Invalid tags: {}. Skipping folder.", e);
                        continue;
                    }

                    let inherit = Confirm::with_theme(&theme)
                        .with_prompt("Apply to subfolders (inherit)?")
                        .default(false)
                        .interact()?;

                    if !dry_run {
                        new_rules.push(FolderRule {
                            pattern: folder.clone(),
                            tags,
                            inherit,
                        });
                        rules_changed = true;
                    } else {
                        println!("[dry-run] Would add rule for '{}'", folder);
                    }
                }
            }
        }
    }

    // ── Persist updated config ────────────────────────────────────────────

    if rules_changed && !dry_run {
        config.folder_tags.rules = new_rules;
        let toml_str = toml::to_string_pretty(&config)?;
        fs::write(&config.config_file, &toml_str)
            .with_context(|| format!("Failed to write config to {:?}", config.config_file))?;
        println!(
            "\n✅ Config updated. Run `hamster folder sync` to apply the new rules. \
             The hamster is ready."
        );
    } else if rules_changed && dry_run {
        println!("\n[dry-run] Config would be updated.");
    } else {
        println!("\n✅ Folder structure already in sync. The hamster is satisfied.");
    }

    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Find folders with names similar to `pattern` using Levenshtein distance.
/// Used to suggest alternatives when fixing an orphaned rule.
fn find_similar_folders(
    pattern: &str,
    current_folders: &HashSet<String>,
    limit: usize,
) -> Vec<String> {
    let mut scored: Vec<(String, usize)> = current_folders
        .iter()
        .map(|f| (f.clone(), strsim::levenshtein(pattern, f)))
        .collect();
    scored.sort_by_key(|(_, dist)| *dist);
    scored.into_iter().take(limit).map(|(f, _)| f).collect()
}

/// Suggest tags based on well-known folder name keywords.
/// The hamster has strong opinions about what "Trash" means.
fn suggest_tags_for_folder(folder: &str) -> Vec<String> {
    let lower = folder.to_lowercase();
    let mut tags = std::collections::HashSet::new();
    if lower.contains("inbox") {
        tags.insert("inbox");
    }
    if lower.contains("sent") {
        tags.insert("sent");
    }
    if lower.contains("draft") {
        tags.insert("draft");
    }
    if lower.contains("trash") || lower.contains("deleted") || lower.contains("bin") {
        tags.insert("deleted");
    }
    if lower.contains("archive") {
        tags.insert("archive");
    }
    if lower.contains("spam") || lower.contains("junk") {
        tags.insert("spam");
    }

    let mut v: Vec<String> = tags.into_iter().map(String::from).collect();
    v.sort();
    v
}
