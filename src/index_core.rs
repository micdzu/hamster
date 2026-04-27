// index_core.rs – The Hamster's central granary logic (v2 – STRING tags, parsed Message-ID,
// schema migration handling)
//
// Identity is the Message-ID header. Path is still stored
// for display ("open in pager", folder explain), but the hamster
// now recognises a seed even if it rolls to a different corner.
// Messages without a Message-ID fall back to using the file path
// so nothing is lost – but those edge cases are rare.
//
// Renovation: tags are now STRING (not TEXT) so exact tag queries work.
// Message-ID extraction now uses mail-parser instead of hand-rolled header
// scanning. Schema mismatches are caught and reported with a helpful
// rebuild hint. The hamster is less prone to folding-related confusion
// and more helpful when things go wrong.

use anyhow::{Context, Result};
use mail_parser::{Address, MessageParser};
use std::collections::HashSet;
use std::path::Path;
use tantivy::directory::MmapDirectory;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

use crate::index_maildir::flags_to_tags;
use crate::index_text::collect_text_parts;

// A job that the writer thread will chew on.
pub struct IndexJob {
    pub path: String,
    pub flags: HashSet<char>,
    pub raw_data: Vec<u8>,
    pub message_id: String, // the acorn's eternal fingerprint
}

#[allow(dead_code)]
pub struct HamsterIndex {
    pub index: Index,
    writer: IndexWriter,
    pub schema: Schema,
    pub from_field: Field,
    pub to_field: Field,
    pub subject_field: Field,
    pub body_field: Field,
    pub date_field: Field,
    pub tags_field: Field,
    pub path_field: Field,
    pub message_id_field: Field, // the permanent acorn label
}

impl HamsterIndex {
    // Open or create the index at `index_path`. The hamster digs a fresh
    // burrow if none exists.
    pub fn new(index_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_path)
            .with_context(|| format!("Failed to create index directory: {:?}", index_path))?;

        let dir = MmapDirectory::open(index_path)
            .with_context(|| format!("Failed to open index directory: {:?}", index_path))?;

        let index = match Index::open_or_create(dir, Self::build_schema()) {
            Ok(i) => i,
            Err(tantivy::TantivyError::SchemaError(msg)) => {
                return Err(anyhow::anyhow!(
                    "Index schema mismatch: {}

                    The tags field format changed in this version of hamster.
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
                    "Failed to open or create index: {}

                    If the index directory is corrupted, try removing it:
                      rm -rf {:?}
                    Then run: hamster index",
                    e,
                    index_path
                ));
            }
        };

        let writer = index
            .writer(50_000_000)
            .context("Failed to create index writer")?;

        let schema = index.schema();
        let from_field = schema
            .get_field("from")
            .context("Schema missing 'from' field - rebuild index required")?;
        let to_field = schema
            .get_field("to")
            .context("Schema missing 'to' field - rebuild index required")?;
        let subject_field = schema
            .get_field("subject")
            .context("Schema missing 'subject' field - rebuild index required")?;
        let body_field = schema
            .get_field("body")
            .context("Schema missing 'body' field - rebuild index required")?;
        let date_field = schema
            .get_field("date")
            .context("Schema missing 'date' field - rebuild index required")?;
        let tags_field = schema
            .get_field("tags")
            .context("Schema missing 'tags' field - rebuild index required")?;
        let path_field = schema
            .get_field("path")
            .context("Schema missing 'path' field - rebuild index required")?;
        let message_id_field = schema
            .get_field("message_id")
            .context("Schema missing 'message_id' field - rebuild index required")?;

        Ok(HamsterIndex {
            index,
            writer,
            schema,
            from_field,
            to_field,
            subject_field,
            body_field,
            date_field,
            tags_field,
            path_field,
            message_id_field,
        })
    }

    // The hamster's blueprint for what to store.
    fn build_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("from", TEXT | STORED);
        schema_builder.add_text_field("to", TEXT | STORED);
        schema_builder.add_text_field("subject", TEXT | STORED);
        schema_builder.add_text_field("body", TEXT | STORED);
        schema_builder.add_date_field("date", INDEXED | STORED | FAST);
        // CRITICAL FIX: tags must be STRING, not TEXT. TEXT tokenizes the tag,
        // breaking exact-match TermQuery lookups. Tags are atomic identifiers.
        schema_builder.add_text_field("tags", STRING | STORED);
        schema_builder.add_text_field("path", STRING | STORED);
        schema_builder.add_text_field("message_id", STRING | STORED); // the permanent acorn ID
        schema_builder.build()
    }

    // Turn a mail-parser address into a tidy "Name <email>" string.
    pub fn extract_address(addr: &Address) -> String {
        fn format_addr(a: &mail_parser::Addr) -> String {
            let email = a.address.as_deref().unwrap_or("").trim();
            let name = a.name.as_deref().unwrap_or("").trim();
            if name.is_empty() || email.is_empty() {
                email.to_string()
            } else {
                format!("{} <{}>", name, email)
            }
        }

        match addr {
            Address::List(list) => list.first().map(format_addr).unwrap_or_default(),
            Address::Group(groups) => groups
                .first()
                .and_then(|g| g.addresses.first())
                .map(format_addr)
                .unwrap_or_default(),
        }
    }

    // Sniff out the Message-ID using the proper MIME parser.
    // Falls back to scanning the raw header only if the parser fails
    // to produce one (extremely rare).
    pub fn extract_message_id(raw: &[u8]) -> String {
        let parser = MessageParser::new();
        if let Some(message) = parser.parse(raw) {
            if let Some(mid) = message.message_id() {
                let id = mid.trim();
                let id = id.trim_start_matches('<').trim_end_matches('>');
                if !id.is_empty() {
                    return id.to_string();
                }
            }
        }

        // Fallback: scan the first 4KB manually. Handles edge cases where
        // mail-parser chokes on a malformed message but we still need
        // some kind of identifier.
        let head = std::str::from_utf8(&raw[..raw.len().min(4096)]).unwrap_or("");
        for line in head.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("message-id:") || lower.starts_with("message-id") {
                if let Some(rest) = line.find(':') {
                    let id = line[rest + 1..].trim();
                    let id = id.trim_start_matches('<').trim_end_matches('>');
                    if !id.is_empty() {
                        return id.to_string();
                    }
                }
            }
            if line.is_empty() {
                break;
            }
        }
        String::new()
    }

    // Chew one email and stuff it into the granary.
    // If a seed with the same message_id already exists, the old one is
    // replaced – no duplicates, no lost acorns.
    pub fn index_message(
        &mut self,
        path: &str,
        raw_mail: &[u8],
        flags: &HashSet<char>,
        message_id: &str,
    ) -> Result<()> {
        let parser = MessageParser::new();
        let message = parser
            .parse(raw_mail)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse email: {}", path))?;

        let from = message
            .from()
            .map(Self::extract_address)
            .unwrap_or_default();
        let to = message.to().map(Self::extract_address).unwrap_or_default();
        let subject = message.subject().unwrap_or_default();

        let body = if message.parts.is_empty() {
            message.body_text(0).unwrap_or_default().to_string()
        } else {
            collect_text_parts(&message)
        };

        let date = message
            .date()
            .map(|d| {
                let ts = d.to_timestamp();
                tantivy::DateTime::from_timestamp_secs(ts)
            })
            .unwrap_or_else(|| tantivy::DateTime::from_timestamp_secs(0));

        let tags = flags_to_tags(flags);

        // Delete any previous seed with the same message_id so we don't get
        // ghost acorns.
        self.writer
            .delete_term(Term::from_field_text(self.message_id_field, message_id));

        let mut doc = TantivyDocument::new();
        doc.add_text(self.from_field, from);
        doc.add_text(self.to_field, to);
        doc.add_text(self.subject_field, subject);
        doc.add_text(self.body_field, body);
        doc.add_date(self.date_field, date);
        doc.add_text(self.path_field, path);
        doc.add_text(self.message_id_field, message_id); // the acorn's name
        for tag in tags {
            doc.add_text(self.tags_field, tag);
        }

        self.writer
            .add_document(doc)
            .with_context(|| format!("Failed to index message: {}", path))?;
        Ok(())
    }

    // Flush all pending changes to the granary.
    pub fn commit(&mut self) -> Result<()> {
        self.writer
            .commit()
            .context("Failed to commit index changes")?;
        Ok(())
    }

    // Remove any acorn whose message_id is in the `deleted_ids` set.
    // This is how the hamster cleans up after a seed has vanished from
    // the Maildir. The caller figures out exactly what's missing from the
    // filesystem, so we just issue targeted deletes. No full-table scans!
    pub fn delete_missing(&mut self, deleted_ids: &HashSet<String>) -> Result<usize> {
        let count = deleted_ids.len();
        for mid in deleted_ids {
            self.writer
                .delete_term(Term::from_field_text(self.message_id_field, mid));
        }
        Ok(count)
    }
}
