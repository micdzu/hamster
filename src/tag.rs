// tag.rs – The Hamster's Label Maker
//
// This is where the hamster carefully peels and sticks tags onto messages.
// It uses the stable message_id as the primary key, so labels never fall
// off when a file moves. Invalid tags are gently refused.
//
// Renovation: tag updates no longer re-read and re-parse the .eml file.
// Everything we need (from, to, subject, body, date, path) is already
// stored in Tantivy. We extract it with StoredFields::from_doc() and
// write it straight back. Fast, clean, correct.
//
// The hamster is pleased to have eliminated several kilobytes of
// mail-parser and fs::read calls from this module.

use anyhow::{Context, Result};
use colored::Colorize;
use tantivy::collector::DocSetCollector;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{TantivyDocument, Term};

use crate::index_access::{IndexAccess, StoredFields};
use crate::setup::HamsterConfig;

pub fn run(config: &HamsterConfig, tag_changes: Vec<String>, query_str: String) -> Result<()> {
    // ── Validate ALL tag changes before touching a single acorn ──────────
    //
    // We validate everything up front rather than discovering a bad tag
    // halfway through a large batch. Partial updates are worse than no
    // updates, and the hamster believes in integrity.
    for change in &tag_changes {
        let tag = change
            .strip_prefix('+')
            .or_else(|| change.strip_prefix('-'))
            .unwrap_or(change.as_str());
        if let Err(e) = crate::validation::validate_tag(tag) {
            return Err(anyhow::anyhow!(
                "Invalid tag '{}': {}. \
                 The hamster refuses to apply any tags until all of them are valid.",
                tag,
                e
            ));
        }
    }

    let ia = IndexAccess::open(&config.index_dir)?;
    let searcher = ia.searcher()?;

    // Open the writer once, up front. We'll commit at the end.
    // No point opening and closing it per-document.
    let mut writer = ia.writer()?;

    let query_parser = QueryParser::for_index(
        ia.index(),
        vec![ia.from, ia.to, ia.subject, ia.body, ia.tags],
    );

    let query = query_parser
        .parse_query(&query_str)
        .with_context(|| format!("Failed to parse query: '{}'", query_str))?;

    let matching_docs = searcher
        .search(&query, &DocSetCollector)
        .context("Failed to search for matching documents")?;

    if matching_docs.is_empty() {
        println!("{} No messages matched the query.", "🔍".yellow());
        return Ok(());
    }

    let mut updated = 0usize;
    let mut errors = 0usize;

    for doc_address in matching_docs {
        let doc: TantivyDocument = match searcher.doc(doc_address) {
            Ok(d) => d,
            Err(e) => {
                errors += 1;
                log::warn!(
                    "Failed to read document at {:?}: {}. \
                     The hamster skips this acorn and moves on.",
                    doc_address,
                    e
                );
                continue;
            }
        };

        // Lift everything we need from the stored document – no disk I/O.
        // StoredFields gives us from, to, subject, body, date, path, and
        // message_id all at once. The hamster already chewed this once.
        let fields = StoredFields::from_doc(&doc, &ia);

        if fields.path.is_empty() {
            // This shouldn't happen in a healthy index, but let's be defensive.
            // A document without a path can't be opened anyway, so skip it.
            errors += 1;
            log::warn!(
                "Document at {:?} has no stored path. Corrupted entry? Skipping.",
                doc_address
            );
            continue;
        }

        // Collect current tags from the document.
        let mut tags: Vec<String> = doc
            .get_all(ia.tags)
            .flat_map(|v| v.as_str())
            .map(String::from)
            .collect();

        // Apply tag additions and removals.
        for change in &tag_changes {
            if let Some(tag) = change.strip_prefix('+') {
                // No duplicates – the hamster is tidy.
                if !tags.contains(&tag.to_string()) {
                    tags.push(tag.to_string());
                }
            } else if let Some(tag) = change.strip_prefix('-') {
                tags.retain(|t| t != tag);
            }
        }

        // Replace the old document. We key on message_id so this works
        // correctly even if the file has been moved since the last index run.
        writer.delete_term(Term::from_field_text(ia.message_id, &fields.message_id));

        let mut new_doc = TantivyDocument::new();
        fields.write_into(&mut new_doc, &ia);
        for tag in &tags {
            new_doc.add_text(ia.tags, tag);
        }

        if let Err(e) = writer.add_document(new_doc) {
            errors += 1;
            log::warn!("Failed to add updated document for {}: {}", fields.path, e);
            continue;
        }

        updated += 1;
    }

    writer.commit().context("Failed to commit tag changes")?;

    // Status line. If there were errors, mention them so the user knows
    // to check the logs. The hamster is transparent about its mistakes.
    if errors > 0 {
        println!(
            "{} Tags updated for {} message(s). ({} error(s) – check logs for details.)",
            "✨".green(),
            updated,
            errors
        );
    } else {
        println!("{} Tags updated for {} message(s).", "✨".green(), updated);
    }

    Ok(())
}
