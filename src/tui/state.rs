// tui/state.rs – The Hamster's mental map (v7 – batched writes, unified tag rewriter,
// message_id-based preview, terminal-safe pager, capped history)
//
// This is the complete thinking apparatus. It holds the search query,
// results, tag manipulation, archive, yank, explain, query history,
// interactive filter tree, focus management, and a lazy index writer.
// No file I/O, no MIME parsing – everything comes from Tantivy's stored fields.
// The hamster is very proud of its clean little brain.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::Value;
use tantivy::{IndexWriter, TantivyDocument, Term};

use crate::diff;
use crate::folder_tags;
use crate::index_access::{IndexAccess, StoredFields};
use crate::setup::HamsterConfig;

// How many results the hamster will collect to keep the UI snappy.
pub const MAX_SEARCH_RESULTS: usize = 200;

// ── A single row in the message list ──────────────────────────────────────

#[derive(Clone)]
pub struct MailRow {
    pub subject: String,
    pub from: String,
    pub date_ts: i64, // Unix timestamp – the hamster understands epochs.
    pub tags: Vec<String>,
    pub path: String,       // where the seed is stored on disk
    pub message_id: String, // permanent key for preview fetch
}

impl MailRow {
    /// Does this little seed have an "unread" sticker?
    pub fn is_unread(&self) -> bool {
        self.tags.iter().any(|t| t == "unread")
    }

    /// Format the date in a human‑friendly, relative way.
    pub fn date_display(&self) -> String {
        if self.date_ts == 0 {
            return " ".to_string(); // unknown date – blank column
        }
        let now = Local::now();
        let dt = Local.timestamp_opt(self.date_ts, 0).single().unwrap_or(now);
        let delta = now.signed_duration_since(dt);
        if delta.num_days() > 365 {
            dt.format("%Y-%m-%d").to_string()
        } else if delta.num_days() >= 1 {
            format!("{:>3}d ago ", delta.num_days())
        } else if delta.num_hours() >= 1 {
            format!("{:>3}h ago ", delta.num_hours())
        } else {
            format!("{:>3}m ago ", delta.num_minutes().max(0))
        }
    }
}

// ── What the hamster is doing with the tag input box ──────────────────────

#[derive(PartialEq)]
pub enum InputMode {
    Normal,
    AddTag,
    RemoveTag,
}

// ── Which pane has the keyboard's attention ───────────────────────────────

#[derive(PartialEq, Clone)]
pub enum Focus {
    Search, // typing in the search bar
    Filter, // navigating the filter tree
    List,   // message list (implicit, arrow keys work)
}

// ── A single item in the filter tree ──────────────────────────────────────

pub struct FilterItem {
    pub label: String,             // what we display
    pub tag_value: Option<String>, // None = header / "All messages", Some("inbox") = selectable
    pub depth: usize,              // indentation level
}

/// Build the whole filter tree from the folder‑tag configuration.
/// Top‑level groups appear as bold headers, their tags as radio items.
/// A special "All messages" item clears the filter, and "Untagged" catches
/// messages without any tag.
pub fn build_filter_items(config: &HamsterConfig) -> Vec<FilterItem> {
    let mut items = Vec::new();

    // "All messages" – the mental reset button
    items.push(FilterItem {
        label: "All messages".into(),
        tag_value: None,
        depth: 0,
    });

    if !config.folder_tags.enabled || config.folder_tags.rules.is_empty() {
        // No folder rules defined; still offer Untagged.
        items.push(FilterItem {
            label: "Untagged".into(),
            tag_value: Some("__untagged__".into()),
            depth: 0,
        });
        return items;
    }

    // Build mailbox → tags mapping by extracting the first path component
    let mut groups: HashMap<String, HashSet<String>> = HashMap::new();
    for rule in &config.folder_tags.rules {
        let top = rule
            .pattern
            .split('/')
            .next()
            .unwrap_or(&rule.pattern)
            .to_string();
        if top.is_empty() {
            continue;
        }
        let entry = groups.entry(top).or_insert_with(HashSet::new);
        for tag in &rule.tags {
            let clean = tag.strip_prefix('-').unwrap_or(tag);
            if !clean.is_empty() {
                entry.insert(clean.to_string());
            }
        }
    }

    let mut sorted_groups: Vec<(String, Vec<String>)> = groups
        .into_iter()
        .map(|(mailbox, tags_set)| {
            let mut v: Vec<String> = tags_set.into_iter().collect();
            v.sort();
            (mailbox, v)
        })
        .collect();
    sorted_groups.sort_by(|a, b| a.0.cmp(&b.0));

    for (mailbox, tags) in &sorted_groups {
        // Bold header
        items.push(FilterItem {
            label: format!("📁 {}", mailbox),
            tag_value: None,
            depth: 0,
        });
        // Radio items for each tag
        for tag in tags {
            items.push(FilterItem {
                label: tag.clone(),
                tag_value: Some(tag.clone()),
                depth: 1,
            });
        }
    }

    // Always add Untagged at the bottom
    items.push(FilterItem {
        label: "Untagged".into(),
        tag_value: Some("__untagged__".into()),
        depth: 0,
    });

    items
}

// ── The hamster's big brain ───────────────────────────────────────────────

pub struct App {
    // Index access – lives for the whole TUI session.
    ia: IndexAccess,
    // Lazy writer – opened on first mutation, committed and dropped on exit.
    writer: Option<IndexWriter>,
    // Tracks whether the writer has uncommitted changes.
    pub writer_dirty: bool,

    // Search
    pub search_input: String,
    pub pending_search: Option<Instant>,
    pub results: Vec<MailRow>,
    pub selected: usize,    // index of the highlighted message
    pub list_offset: usize, // scroll offset of the message list

    // Preview
    pub preview: Option<String>,
    pub preview_offset: u16,
    pub preview_visible: bool, // toggled with Ctrl‑e

    // Tag input mode
    pub input_mode: InputMode,
    pub tag_input: String,

    // Overlays & status
    pub show_help: bool,
    pub status_message: Option<String>,
    pub explain_text: Option<String>, // shown instead of preview when active

    // Query UX
    pub parsed_query: Option<String>, // e.g. "Searching: from:boss AND ..."
    pub query_error: Option<String>,  // e.g. "Invalid query: ..."
    pub query_history: Vec<String>,
    pub history_index: Option<usize>, // None if not cycling
    pub show_query_help: bool,

    // Interactive filter pane
    pub focus: Focus,
    pub filter_items: Vec<FilterItem>,
    pub filter_selected: usize, // index of the highlighted filter item
    pub filter_offset: usize,   // scroll offset of the filter pane
    pub active_filter: Option<String>, // None = no filter, Some("inbox") or "__untagged__"
}

impl App {
    pub fn new(ia: IndexAccess) -> Self {
        Self {
            ia,
            writer: None,
            writer_dirty: false,

            search_input: String::new(),
            pending_search: None,
            results: Vec::new(),
            selected: 0,
            list_offset: 0,
            preview: None,
            preview_offset: 0,
            preview_visible: true,
            input_mode: InputMode::Normal,
            tag_input: String::new(),
            show_help: false,
            status_message: None,
            explain_text: None,

            parsed_query: None,
            query_error: None,
            query_history: Vec::new(),
            history_index: None,
            show_query_help: false,

            focus: Focus::Search,
            filter_items: Vec::new(),
            filter_selected: 0,
            filter_offset: 0,
            active_filter: None,
        }
    }

    // ── Lazy writer management ──────────────────────────────────────────

    /// Returns a mutable reference to the index writer, opening it if needed.
    fn writer(&mut self) -> Result<&mut IndexWriter> {
        if self.writer.is_none() {
            self.writer = Some(self.ia.writer()?);
        }
        Ok(self.writer.as_mut().unwrap())
    }

    /// Commit and drop the writer. Call on TUI exit.
    pub fn commit_and_close_writer(&mut self) -> Result<()> {
        if let Some(mut w) = self.writer.take() {
            if self.writer_dirty {
                w.commit().context("Failed to commit writer on exit")?;
                self.writer_dirty = false;
            }
        }
        Ok(())
    }

    // ── Filter pane helpers ─────────────────────────────────────────────

    pub fn refresh_filter_items(&mut self, config: &HamsterConfig) {
        self.filter_items = build_filter_items(config);
        self.filter_selected = 0;
    }

    /// Keep the selected filter item visible by adjusting the scroll offset.
    pub fn scroll_filter_into_view(&mut self, max_visible: usize) {
        if self.filter_selected < self.filter_offset {
            self.filter_offset = self.filter_selected;
        } else if self.filter_selected >= self.filter_offset + max_visible {
            self.filter_offset = self.filter_selected - max_visible + 1;
        }
    }

    // ── Focus management ────────────────────────────────────────────────

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Search => Focus::Filter,
            Focus::Filter => Focus::List,
            Focus::List => Focus::Search,
        };
    }

    // ── Search (with optional tag filter from the filter pane) ──────────

    pub fn search(&mut self) -> Result<()> {
        let searcher = self.ia.searcher()?;
        let query_parser = QueryParser::for_index(
            self.ia.index(),
            vec![
                self.ia.from,
                self.ia.to,
                self.ia.subject,
                self.ia.body,
                self.ia.tags,
            ],
        );

        // Parse the user‑typed query
        let user_query: Box<dyn Query> = if self.search_input.trim().is_empty() {
            self.query_error = None;
            self.parsed_query = None;
            Box::new(AllQuery)
        } else {
            match query_parser.parse_query(&self.search_input) {
                Ok(q) => {
                    self.query_error = None;
                    self.parsed_query = Some(format!("Searching: {}", self.search_input));
                    q
                }
                Err(e) => {
                    self.query_error = Some(format!("Invalid query: {}", e));
                    self.parsed_query = None;
                    self.pending_search = None;
                    return Ok(()); // keep old results, show error
                }
            }
        };

        // If a filter is active, combine it with the user query
        let final_query: Box<dyn Query> = match &self.active_filter {
            None => user_query,
            Some(tag) if tag == "__untagged__" => {
                // We'll fetch more results and then filter client‑side
                user_query
            }
            Some(tag) => {
                let tag_term = Term::from_field_text(self.ia.tags, tag);
                let tag_query = TermQuery::new(tag_term, tantivy::schema::IndexRecordOption::Basic);
                Box::new(BooleanQuery::new(vec![
                    (Occur::Must, user_query),
                    (Occur::Must, Box::new(tag_query)),
                ]))
            }
        };

        // For untagged we need a bigger sample to find enough empty‑tagged messages
        let limit = if self.active_filter.as_deref() == Some("__untagged__") {
            MAX_SEARCH_RESULTS * 5
        } else {
            MAX_SEARCH_RESULTS
        };
        let collector = TopDocs::with_limit(limit);
        let top_docs = searcher.search(&final_query, &collector)?;

        let mut raw_results: Vec<MailRow> = Vec::new();
        for (_score, addr) in top_docs {
            if let Ok(doc) = searcher.doc::<TantivyDocument>(addr) {
                let subject = doc
                    .get_first(self.ia.subject)
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no subject)")
                    .to_string();
                let from = doc
                    .get_first(self.ia.from)
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)")
                    .to_string();
                let date_ts = doc
                    .get_first(self.ia.date)
                    .and_then(|v| v.as_datetime())
                    .map(|dt| dt.into_timestamp_secs())
                    .unwrap_or(0);
                let tags: Vec<String> = doc
                    .get_all(self.ia.tags)
                    .flat_map(|v| v.as_str())
                    .map(String::from)
                    .collect();
                let path = doc
                    .get_first(self.ia.path)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let message_id = doc
                    .get_first(self.ia.message_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                raw_results.push(MailRow {
                    subject,
                    from,
                    date_ts,
                    tags,
                    path,
                    message_id,
                });
            }
        }

        // Client‑side filter for untagged
        if self.active_filter.as_deref() == Some("__untagged__") {
            raw_results.retain(|r| r.tags.is_empty());
        }

        // Sort by date, then trim to the allowed limit
        raw_results.sort_by(|a, b| b.date_ts.cmp(&a.date_ts));
        raw_results.truncate(MAX_SEARCH_RESULTS);

        self.results = raw_results;
        self.selected = 0;
        self.list_offset = 0;
        self.preview = None;
        self.preview_offset = 0;
        self.pending_search = None;

        // If the preview pane is open, immediately load the first message
        if self.preview_visible && !self.results.is_empty() {
            let _ = self.load_preview();
        }
        Ok(())
    }

    /// Fill the preview content from the currently selected message.
    /// Fetches the body on demand via message_id search instead of storing
    /// the full document in every MailRow.
    fn load_preview(&mut self) -> Result<()> {
        let row = match self.results.get(self.selected) {
            Some(r) => r,
            None => return Ok(()),
        };

        let searcher = self.ia.searcher()?;
        let term = Term::from_field_text(self.ia.message_id, &row.message_id);
        let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let docs = searcher.search(&query, &TopDocs::with_limit(1))?;

        let body = if let Some((_, addr)) = docs.first() {
            let doc = searcher.doc::<TantivyDocument>(*addr)?;
            doc.get_first(self.ia.body)
                .and_then(|v| v.as_str())
                .unwrap_or("(body not stored)")
                .to_string()
        } else {
            "(message not found)".to_string()
        };

        // If the body looks like HTML, sanitise it into plain text
        let display = if body.trim().starts_with('<') {
            html_safe_md::sanitize_html(&body)
        } else {
            body
        };
        self.preview = Some(display);
        self.preview_offset = 0;
        Ok(())
    }

    /// Toggle the right preview pane on/off.
    pub fn toggle_preview_visibility(&mut self) {
        self.preview_visible = !self.preview_visible;
        if self.preview_visible {
            let _ = self.load_preview();
        }
    }

    // ── Filter pane actions ────────────────────────────────────────────

    /// Apply the currently highlighted filter item (Enter in filter focus).
    /// If it's already active, toggle it off. Headers or "All messages" clear the filter.
    pub fn apply_filter_from_index(&mut self) -> Result<()> {
        if let Some(item) = self.filter_items.get(self.filter_selected) {
            if let Some(ref tag_value) = item.tag_value {
                if self.active_filter.as_deref() == Some(tag_value.as_str()) {
                    self.active_filter = None; // toggle off
                } else {
                    self.active_filter = Some(tag_value.clone());
                }
            } else {
                self.active_filter = None; // header or "All messages"
            }
            self.pending_search = None;
            self.search()?;
        }
        Ok(())
    }

    /// Clear any active filter (Esc in filter focus).
    pub fn clear_filter(&mut self) -> Result<()> {
        self.active_filter = None;
        self.pending_search = None;
        self.search()
    }

    // ── Message actions (all use StoredFields & lazy writer) ──────────

    /// Open the selected message in the system pager ($PAGER or less).
    /// Uses a scope guard to guarantee terminal restoration even if the
    /// pager command fails.
    pub fn open_in_pager(&self) -> Result<()> {
        let row = match self.results.get(self.selected) {
            Some(r) => r,
            None => return Ok(()),
        };
        if row.path.is_empty() {
            return Ok(());
        }

        // Temporarily leave the TUI to show the pager
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;

        // Scope guard: whatever happens next, we restore the TUI.
        struct PagerGuard;
        impl Drop for PagerGuard {
            fn drop(&mut self) {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::EnterAlternateScreen
                );
                let _ = crossterm::terminal::enable_raw_mode();
            }
        }
        let _guard = PagerGuard;

        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        std::process::Command::new(&pager)
            .arg(&row.path)
            .status()
            .with_context(|| format!("Failed to launch pager '{}'", pager))?;

        // `_guard` drops here, restoring raw mode and alternate screen.
        Ok(())
    }

    /// Unified tag rewriter. Computes the new tag set from `edit_fn`, deletes the
    /// old document, and writes the new one — without committing.
    fn rewrite_message_tags<F>(&mut self, edit_fn: F) -> Result<()>
    where
        F: FnOnce(&mut Vec<String>),
    {
        let row = match self.results.get(self.selected) {
            Some(r) => r,
            None => return Ok(()),
        };
        if row.path.is_empty() {
            return Ok(());
        }

        let mut tags: Vec<String> = row.tags.clone();
        edit_fn(&mut tags);

        // Snapshot all field handles from ia before any borrows
        let message_id_field = self.ia.message_id;
        let tags_field = self.ia.tags;
        let from = self.ia.from;
        let to = self.ia.to;
        let subject = self.ia.subject;
        let body = self.ia.body;
        let date = self.ia.date;
        let path = self.ia.path;

        // Search for the document to get stored fields
        let searcher = self.ia.searcher()?;
        let term = Term::from_field_text(message_id_field, &row.message_id);
        let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let docs = searcher.search(&query, &TopDocs::with_limit(1))?;

        let fields = if let Some((_, addr)) = docs.first() {
            let doc = searcher.doc::<TantivyDocument>(*addr)?;
            let get_str = |field: tantivy::schema::Field| {
                doc.get_first(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            StoredFields {
                from: get_str(from),
                to: get_str(to),
                subject: get_str(subject),
                body: get_str(body),
                date: doc
                    .get_first(date)
                    .and_then(|v| v.as_datetime())
                    .unwrap_or_else(|| tantivy::DateTime::from_timestamp_secs(0)),
                path: get_str(path),
                message_id: get_str(message_id_field),
            }
        } else {
            self.status_message = Some("⚠ Message not found in index".to_string());
            return Ok(());
        };

        // Now we can borrow self mutably for the writer
        let writer = self.writer()?;
        writer.delete_term(Term::from_field_text(message_id_field, &fields.message_id));

        let mut new_doc = TantivyDocument::new();
        new_doc.add_text(from, &fields.from);
        new_doc.add_text(to, &fields.to);
        new_doc.add_text(subject, &fields.subject);
        new_doc.add_text(body, &fields.body);
        new_doc.add_date(date, fields.date);
        new_doc.add_text(path, &fields.path);
        new_doc.add_text(message_id_field, &fields.message_id);
        for t in &tags {
            new_doc.add_text(tags_field, t);
        }
        writer.add_document(new_doc)?;

        self.writer_dirty = true;

        if let Some(row) = self.results.get_mut(self.selected) {
            row.tags = tags;
        }
        Ok(())
    }

    /// Add or remove a tag on the selected message.
    /// Uses StoredFields – no file read, no MIME parsing.
    pub fn apply_tag(&mut self, add: bool, tag: &str) -> Result<()> {
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        if let Err(e) = crate::validation::validate_tag(tag) {
            self.status_message = Some(format!("⛔ {}", e));
            return Ok(());
        }

        let t = tag.to_string();
        self.rewrite_message_tags(|tags| {
            if add {
                if !tags.contains(&t) {
                    tags.push(t.clone());
                }
            } else {
                tags.retain(|x| x != &t);
            }
        })?;

        let verb = if add { "Added" } else { "Removed" };
        self.status_message = Some(format!("✓ {} tag '{}'", verb, tag));
        Ok(())
    }

    /// Toggle the "unread" tag on the selected message.
    pub fn toggle_read_status(&mut self) -> Result<()> {
        let has_unread = self
            .results
            .get(self.selected)
            .map(|r| r.tags.contains(&"unread".to_string()))
            .unwrap_or(false);

        if has_unread {
            self.rewrite_message_tags(|tags| tags.retain(|t| t != "unread"))?;
            self.status_message = Some("✓ Marked as read".to_string());
        } else {
            self.rewrite_message_tags(|tags| {
                if !tags.contains(&"unread".to_string()) {
                    tags.push("unread".to_string());
                }
            })?;
            self.status_message = Some("✓ Marked as unread".to_string());
        }
        Ok(())
    }

    /// Archive the selected message: add "archive", remove "inbox" and "unread".
    pub fn archive_message(&mut self) -> Result<()> {
        let already = self
            .results
            .get(self.selected)
            .map(|r| r.tags.contains(&"archive".to_string()))
            .unwrap_or(false);
        if already {
            self.status_message = Some("Already archived".to_string());
            return Ok(());
        }
        self.rewrite_message_tags(|tags| {
            tags.retain(|t| t != "inbox" && t != "unread");
            if !tags.contains(&"archive".to_string()) {
                tags.push("archive".to_string());
            }
        })?;
        self.status_message = Some("✓ Archived".to_string());
        Ok(())
    }

    /// Copy the selected message's message‑id (or path) to the status bar.
    pub fn yank_message_id(&mut self) {
        if let Some(row) = self.results.get(self.selected) {
            let id = if row.message_id.is_empty() {
                &row.path
            } else {
                &row.message_id
            };
            self.status_message = Some(format!("📋 Yanked: {}", id));
        }
    }

    // ── Explain panel ──────────────────────────────────────────────────

    pub fn toggle_explain(&mut self, config: &HamsterConfig) -> Result<()> {
        if self.explain_text.is_some() {
            self.explain_text = None;
            return Ok(());
        }
        let row = match self.results.get(self.selected) {
            Some(r) => r,
            None => return Ok(()),
        };
        if row.path.is_empty() {
            self.explain_text = Some("(no file path)".to_string());
            return Ok(());
        }
        let text = self.compute_explain(config, &row.path)?;
        self.explain_text = Some(text);
        Ok(())
    }

    fn compute_explain(&self, config: &HamsterConfig, path: &str) -> Result<String> {
        let tags_cfg = &config.folder_tags;
        if !tags_cfg.enabled {
            return Ok("Folder tags are disabled in config.".to_string());
        }
        let mail_root = std::path::PathBuf::from(&config.maildir);
        let compiled_rules =
            folder_tags::compile_rules(&tags_cfg.rules).map_err(|e| anyhow::anyhow!("{}", e))?;
        let managed_tags = folder_tags::determine_managed_tags(tags_cfg);
        let searcher = self.ia.searcher()?;

        let term = tantivy::Term::from_field_text(self.ia.path, path);
        let query = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top_docs = searcher.search(&query, &tantivy::collector::TopDocs::with_limit(1))?;
        let doc = match top_docs.first() {
            Some((_score, doc_addr)) => searcher.doc::<TantivyDocument>(*doc_addr)?,
            None => return Ok("No message found in index for this path.".to_string()),
        };

        let current_tags: HashSet<String> = doc
            .get_all(self.ia.tags)
            .flat_map(|v| v.as_str())
            .map(String::from)
            .collect();

        let folder = folder_tags::folder_from_path(path, &mail_root)
            .unwrap_or_else(|| "(unknown folder)".to_string());

        let (desired_add, forced_remove) =
            folder_tags::folder_tags_for_path(&folder, &compiled_rules);

        let mut desired_add = desired_add;
        let mut forced_remove = forced_remove;
        if config.folder_tags.sync_flags {
            folder_tags::merge_flag_tags(path, &mut desired_add, &mut forced_remove);
        }

        let (to_add, to_remove) =
            diff::compute_tag_diff(&desired_add, &forced_remove, &current_tags, &managed_tags);

        let mut out = String::new();
        out.push_str(&format!("📄 Path: {}\n", path));
        out.push_str(&format!("📁 Folder: {}\n\n", folder));

        out.push_str("🏷️ Current tags: [");
        let mut sorted_tags: Vec<&String> = current_tags.iter().collect();
        sorted_tags.sort();
        out.push_str(
            &sorted_tags
                .into_iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("]\n\n");

        out.push_str("📋 Matched rules:\n");
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
                let kind = if matches { "exact" } else { "inheritance" };
                out.push_str(&format!(
                    " • pattern: '{}' ({kind}) → tags: [{}]\n",
                    rule.pattern.as_str(),
                    rule.tags.join(", ")
                ));
            }
        }

        if config.folder_tags.sync_flags {
            let flags = crate::index_maildir::parse_maildir_flags(path);
            out.push_str(&format!("\n🚩 Maildir flags: {:?}\n", flags));
        }

        out.push_str("\n🎯 Desired add: [");
        let mut da: Vec<&String> = desired_add.iter().collect();
        da.sort();
        out.push_str(
            &da.into_iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("]\n");

        out.push_str("🎯 Desired remove: [");
        let mut dr: Vec<&String> = forced_remove.iter().collect();
        dr.sort();
        out.push_str(
            &dr.into_iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("]\n");

        out.push_str("🧰 Managed tags: [");
        let mut mt: Vec<&String> = managed_tags.iter().collect();
        mt.sort();
        out.push_str(
            &mt.into_iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("]\n");

        if to_add.is_empty() && to_remove.is_empty() {
            out.push_str("\n✅ Already in sync – no changes needed.\n");
        } else {
            out.push_str(&format!("\n➕ Would add: {}\n", to_add.join(", ")));
            out.push_str(&format!("➖ Would remove: {}\n", to_remove.join(", ")));
        }

        Ok(out)
    }

    // ── Query history ──────────────────────────────────────────────────

    pub fn push_to_history(&mut self) {
        let q = self.search_input.trim().to_string();
        if q.is_empty() {
            return;
        }
        if self.query_history.last().map(|s| s == &q).unwrap_or(false) {
            return;
        }
        self.query_history.push(q);
        if self.query_history.len() > 100 {
            self.query_history.remove(0);
        }
        self.history_index = None;
    }

    pub fn history_backward(&mut self) -> Result<()> {
        if self.query_history.is_empty() {
            return Ok(());
        }
        let idx = match self.history_index {
            None => self.query_history.len() - 1,
            Some(i) if i > 0 => i - 1,
            _ => return Ok(()),
        };
        self.history_index = Some(idx);
        self.search_input = self.query_history[idx].clone();
        self.query_error = None;
        self.parsed_query = None;
        self.trigger_search()?;
        Ok(())
    }

    fn trigger_search(&mut self) -> Result<()> {
        self.pending_search = None;
        self.search()
    }

    // ── Navigation helpers ─────────────────────────────────────────────

    pub fn move_down(&mut self, list_height: usize) {
        if self.selected + 1 < self.results.len() {
            self.selected += 1;
            self.adjust_offset(list_height);
        }
    }

    pub fn move_up(&mut self, list_height: usize) {
        if self.selected > 0 {
            self.selected -= 1;
            self.adjust_offset(list_height);
        }
    }

    pub fn adjust_offset(&mut self, list_height: usize) {
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else if self.selected >= self.list_offset + list_height {
            self.list_offset = self.selected - list_height + 1;
        }
    }

    pub fn refresh_preview(&mut self) {
        if self.preview_visible {
            let _ = self.load_preview();
        } else {
            self.preview = None;
        }
    }
}

// ── TerminalGuard: cleans up the terminal when the hamster leaves ────────

pub struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}
