// index_access.rs – The Hamster's Master Key to the Granary (v2 – schema migration)
//
// Opening the index is surprisingly fiddly. You need a path, a schema,
// a dozen field handles, and you haven't even started searching yet.
// Rather than making every command fumble with all that separately,
// we open the index once here and hand out a tidy `IndexAccess` struct
// that contains everything anyone could want.
//
// Renovations since the last burrow-refactor:
//
// 1. `index` is now PRIVATE. Nobody should be reaching around the back
//    of this struct to grab a writer. Use `ia.writer()` like a civilised
//    rodent. This closes the leaky abstraction that was letting callers
//    bypass the whole point of having this struct.
//
// 2. `StoredFields` lives here now. Because every field in the Tantivy
//    schema is `STORED`, we can reconstruct a complete document from a
//    search hit without ever touching the `.eml` file on disk. This is
//    the magic trick that makes tag-only updates fast:
//    read once during indexing, reconstruct from cache, write back.
//    No disk I/O, no parsing, no drama. The hamster approves.
//
// 3. Schema mismatches are caught and reported with a helpful rebuild hint.
//    The hamster believes in graceful failure and clear next steps.

use anyhow::{Context, Result};
use std::path::Path;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter, Searcher, TantivyDocument};

// ── IndexAccess ─────────────────────────────────────────────────────────────

pub struct IndexAccess {
    // The actual Tantivy index. Private: go through `writer()` and
    // `searcher()` like everyone else. The hamster does not negotiate.
    index: Index,
    pub from: Field,
    pub to: Field,
    pub subject: Field,
    pub body: Field,
    pub date: Field,
    pub tags: Field,
    pub path: Field,
    pub message_id: Field,
}

impl IndexAccess {
    // Open an existing index. The index must already exist – run
    // `hamster index` first if you're getting errors here.
    pub fn open(index_dir: &Path) -> Result<Self> {
        let index = match Index::open_in_dir(index_dir) {
            Ok(i) => i,
            Err(tantivy::TantivyError::SchemaError(msg)) => {
                return Err(anyhow::anyhow!(
                    "Index schema mismatch: {}

                    The index format has changed in this version of hamster.
                    Your existing index is incompatible and must be rebuilt.

                    To fix this, run:
                      hamster index

                    This will re-scan your Maildir and build a fresh index.
                    (Your emails are safe – only the search index is rebuilt.)",
                    msg
                ));
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to open index at {:?}: {}

                    Did you run 'hamster index' yet? The hamster can't search what it hasn't chewed.
                    If the index is corrupted, you can remove it and rebuild:
                      rm -rf {:?}
                      hamster index",
                    index_dir,
                    e,
                    index_dir
                ));
            }
        };

        let schema = index.schema();

        let from = get_field(&schema, "from")?;
        let to = get_field(&schema, "to")?;
        let subject = get_field(&schema, "subject")?;
        let body = get_field(&schema, "body")?;
        let date = get_field(&schema, "date")?;
        let tags = get_field(&schema, "tags")?;
        let path = get_field(&schema, "path")?;
        let message_id = get_field(&schema, "message_id")?;

        Ok(IndexAccess {
            index,
            from,
            to,
            subject,
            body,
            date,
            tags,
            path,
            message_id,
        })
    }

    // Cheap, read-only access to the underlying Tantivy index.
    // Only for query parsers – the writer must go through `writer()`.
    pub fn index(&self) -> &Index {
        &self.index
    }

    // Create a searcher. Cheap – readers share an underlying arc and a
    // memory-mapped file. Feel free to call this frequently.
    pub fn searcher(&self) -> Result<Searcher> {
        let reader = self
            .index
            .reader()
            .context("Failed to create index reader")?;
        Ok(reader.searcher())
    }

    // Open an exclusive index writer with a 50 MB in-memory heap.
    //
    // Tantivy only allows ONE live writer at a time. If you open a second
    // before closing the first, you will get a very unhappy error. The
    // hamster does not tolerate squatters in its granary. Commit and drop
    // promptly.
    pub fn writer(&self) -> Result<IndexWriter> {
        self.index.writer(50_000_000).context(
            "Failed to create index writer.             Another writer may still be open – did a previous command crash?             If so, try running 'hamster index' to clean up.",
        )
    }
}

fn get_field(schema: &Schema, name: &str) -> Result<Field> {
    schema.get_field(name).with_context(|| {
        format!(
            "The schema is missing the '{}' field.             The index may be corrupted or from an older version of hamster.             Rebuild it with 'hamster index' (sorry, yes, all of it).",
            name
        )
    })
}

// ── StoredFields ────────────────────────────────────────────────────────────
//
// Tag updates used to work like this:
// 1. Read the .eml file off disk.
// 2. Parse the entire MIME message.
// 3. Extract from, to, subject, body, date.
// 4. Write a new Tantivy document with updated tags.
//
// This was correct but deeply unnecessary. The hamster already did steps
// 1-3 during indexing and stored the results *right there in Tantivy*.
//
// `StoredFields` is the fast path: lift the already-chewed fields out of
// an existing document, change the tags, write it back. No disk I/O,
// no parsing, no re-reading a 10 MB email just to flip one tag.
//
// If a field is absent (corrupted document, schema migration), you get an
// empty string or epoch-zero date. Better than a panic over a stray acorn.

pub struct StoredFields {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub date: tantivy::DateTime,
    pub path: String,
    pub message_id: String,
}

impl StoredFields {
    // Lift all non-tag fields out of an existing Tantivy document.
    // This is zero I/O – everything comes straight from Tantivy's
    // in-memory segment cache.
    pub fn from_doc(doc: &TantivyDocument, ia: &IndexAccess) -> Self {
        let get_str = |field| {
            doc.get_first(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        StoredFields {
            from: get_str(ia.from),
            to: get_str(ia.to),
            subject: get_str(ia.subject),
            body: get_str(ia.body),
            // Dates get special treatment: Tantivy stores them as DateTime,
            // not as a string, so we can't use get_str here.
            date: doc
                .get_first(ia.date)
                .and_then(|v| v.as_datetime())
                .unwrap_or_else(|| tantivy::DateTime::from_timestamp_secs(0)),
            path: get_str(ia.path),
            message_id: get_str(ia.message_id),
        }
    }

    // Stuff all non-tag fields into a (pre-created) document builder.
    // The caller is responsible for adding tags on top.
    // We deliberately don't touch tags here – that's the whole point.
    pub fn write_into(&self, doc: &mut TantivyDocument, ia: &IndexAccess) {
        doc.add_text(ia.from, &self.from);
        doc.add_text(ia.to, &self.to);
        doc.add_text(ia.subject, &self.subject);
        doc.add_text(ia.body, &self.body);
        doc.add_date(ia.date, self.date);
        doc.add_text(ia.path, &self.path);
        doc.add_text(ia.message_id, &self.message_id);
    }
}
