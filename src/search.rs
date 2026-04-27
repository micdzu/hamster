// search.rs – The Hamster’s All‑Seeing Snout (CLI backend)
//
// This is the pipe‑friendly backend. It sniffs out messages and returns
// structured results (json, ids, paths, or plain). Now with real BM25
// scores, not dates masquerading as scores.

use anyhow::{Context, Result};
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, Query, QueryParser};
use tantivy::schema::Value;
use tantivy::TantivyDocument;

use crate::index_access::IndexAccess;
use crate::setup::HamsterConfig;

/// A tasty morsel returned by a search.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub from: String,
    pub subject: String,
    pub date_ts: i64, // Unix timestamp
    pub message_id: String,
    pub path: String,
    pub tags: Vec<String>,
    pub score: f32, // real relevance score (BM25)
}

/// Run a search and return a vector of results, sorted by date descending.
pub fn search_messages(config: &HamsterConfig, query_str: &str) -> Result<Vec<SearchResult>> {
    let ia = IndexAccess::open(&config.index_dir)?;
    let searcher = ia.searcher()?;

    let query_parser = QueryParser::for_index(
        ia.index(),
        vec![ia.from, ia.to, ia.subject, ia.body, ia.tags],
    );

    let query: Box<dyn Query> = if query_str.trim().is_empty() {
        Box::new(AllQuery)
    } else {
        query_parser.parse_query(query_str).map_err(|e| {
            anyhow::anyhow!(
                "Invalid query: '{}'\n\n\
                 Hints:\n\
                 - Try simple terms: hamster\n\
                 - Search a field: from:hamster@example.com\n\
                 - Combine fields: from:hamster AND subject:wheel\n\
                 - List tags: tags:unread\n\n\
                 Error: {}",
                query_str,
                e
            )
        })?
    };

    // Get proper BM25 scores, then sort by date ourselves.
    let collector = TopDocs::with_limit(200);
    let top_docs = searcher
        .search(&query, &collector)
        .context("Search failed")?;

    let mut results = Vec::new();
    for (score, doc_addr) in top_docs {
        let doc: TantivyDocument = searcher
            .doc(doc_addr)
            .with_context(|| format!("Failed to retrieve document at {:?}", doc_addr))?;

        let from = doc
            .get_first(ia.from)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let subject = doc
            .get_first(ia.subject)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let date_ts = doc
            .get_first(ia.date)
            .and_then(|v| v.as_datetime())
            .map(|dt| dt.into_timestamp_secs())
            .unwrap_or(0);
        let message_id = doc
            .get_first(ia.message_id)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let path = doc
            .get_first(ia.path)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tags: Vec<String> = doc
            .get_all(ia.tags)
            .flat_map(|v| v.as_str())
            .map(String::from)
            .collect();

        results.push(SearchResult {
            from,
            subject,
            date_ts,
            message_id,
            path,
            tags,
            score,
        });
    }

    // The hamster likes the freshest seeds first.
    results.sort_by(|a, b| b.date_ts.cmp(&a.date_ts));

    Ok(results)
}

/// CLI entry point – receives a config reference and the already‑joined query.
pub fn run(config: &HamsterConfig, query_str: String, format: &str) -> Result<()> {
    let results = search_messages(config, &query_str)?;

    if results.is_empty() {
        println!("🐹 No messages found.");
        return Ok(());
    }

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        "ids" => {
            for r in &results {
                println!("{}", r.message_id);
            }
        }
        "paths" => {
            for r in &results {
                println!("{}", r.path);
            }
        }
        _ => {
            // Plain, pipe‑friendly output: one line per message.
            for r in &results {
                let tag_list = if r.tags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", r.tags.join(", "))
                };
                println!("{}\t{}\t{}{}", r.date_ts, r.from, r.subject, tag_list);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_result_serialization() {
        let result = SearchResult {
            from: "hamster@example.com".into(),
            subject: "Wheels".into(),
            date_ts: 0,
            message_id: "abc123".into(),
            path: "/tmp/mail".into(),
            tags: vec!["inbox".to_string()],
            score: 2.5,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("hamster@example.com"));
        assert!(json.contains("Wheels"));
        assert!(json.contains("2.5"));
    }
}
