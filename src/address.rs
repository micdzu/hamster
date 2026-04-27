// address.rs – The Hamster's Fuzzy Address Lookup
//
// The feature that started it all. You type a few letters, the hamster
// finds the email address you meant, ranks them by how often you've talked
// to that person, and outputs in whatever format your mail client demands.
// Typos are forgiven. The hamster is generous.
//
// Architecture (revised edition):
//
// Previously, `run()` would open the full Tantivy index and scan every
// single document to build a contact list. For large mailboxes this was
// embarassingly slow and also completely unnecessary since `hamster index`
// already has all that information.
//
// Now we read from `hamster_addresses.json` – a small JSON file that
// `hamster index` maintains automatically. Address lookup is instant:
// open file → score → print. The hamster is pleased with this arrangement.
//
// `extract_contacts` is now `pub` so that `index.rs` can call it during
// the address cache rebuild without duplicating the parsing logic.

use anyhow::Result;
use colored::Colorize;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

use crate::address_cache::AddressCache;
use crate::setup::HamsterConfig;

// ── Contact (internal display helper) ────────────────────────────────────────
//
// A thin wrapper around an AddressCacheEntry that knows how to format
// itself for output. Kept separate from AddressCacheEntry so the cache
// module doesn't need to know about display concerns.

#[derive(Debug, Clone)]
struct Contact {
    name: String,
    mail: String,
    occurrences: usize,
}

impl Contact {
    fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.mail.clone()
        } else {
            format!("{} <{}>", self.name, self.mail)
        }
    }
}

// ── Address normalisation ────────────────────────────────────────────────────

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

pub fn normalize_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Strip surrounding quotes: "John Doe" -> John Doe
    let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner.replace("\\\"", "\"")
    } else {
        trimmed.to_string()
    };

    // Strip trailing punctuation that sometimes bleeds out of headers.
    let stripped =
        unquoted.trim_end_matches(|c: char| c == '.' || c == ',' || c == ';' || c == '-');

    if stripped.is_empty() {
        return unquoted;
    }

    // If the name is entirely lowercase, capitalise the first letter.
    // "john doe" -> "John doe". Not perfect but better than nothing.
    if stripped
        .chars()
        .all(|c| c.is_lowercase() || c.is_whitespace())
    {
        let mut chars = stripped.chars();
        if let Some(first) = chars.next() {
            return first.to_uppercase().collect::<String>() + chars.as_str();
        }
    }

    stripped.to_string()
}

// ── Contact extraction from header values ────────────────────────────────────
//
// Parses a raw From:/To: header value into (name, email) pairs.
// Handles the common `Name <email>` and bare `email` forms.
// Multiple addresses separated by commas are all extracted.
//
// `pub` because `index.rs` uses this during address cache rebuilds.
// The hamster believes in reusing good code rather than copy-pasting it.

pub fn extract_contacts(header_value: &str) -> Vec<(String, String)> {
    let mut contacts = Vec::new();

    for part in header_value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Try "Name <email>" form first.
        if let Some(start) = part.rfind('<') {
            if let Some(end) = part.rfind('>') {
                if start < end {
                    let name = part[..start].trim();
                    let email = &part[start + 1..end];
                    if email.contains('@') {
                        contacts.push((normalize_name(name), normalize_email(email)));
                        continue;
                    }
                }
            }
        }

        // Fall back to bare email.
        if part.contains('@') {
            contacts.push((String::new(), normalize_email(part)));
        }
    }

    contacts
}

// ── Public entry point ───────────────────────────────────────────────────────

pub fn run(config: &HamsterConfig, format: &str, query: String) -> Result<()> {
    // Load the address book. This is a tiny JSON file read – no Tantivy,
    // no index locking, no drama. The hamster is very smug about this.
    let cache = AddressCache::load(&config.index_dir)?;

    if cache.entries.is_empty() {
        println!(
            "{} No contacts in address book.             Run 'hamster index' to build it.",
            "📭".yellow()
        );
        return Ok(());
    }

    // Convert cache entries to Contact structs for scoring and display.
    let contacts: Vec<Contact> = cache
        .entries
        .into_iter()
        .map(|(email, entry)| Contact {
            mail: email,
            name: entry.name,
            occurrences: entry.count,
        })
        .collect();

    let query_trimmed = query.trim();
    let matcher = SkimMatcherV2::default().ignore_case();

    // Score every contact:
    // - Frequency (occurrences) is the primary signal: the more the hamster
    //   has seen you, the higher you rank. Loyalty is rewarded.
    // - Fuzzy match score is a tiebreaker / relevance signal.
    // - Contacts that don't fuzzy-match at all are excluded entirely.
    let mut scored: Vec<(i64, Contact)> = contacts
        .into_iter()
        .filter_map(|c| {
            let haystack = format!("{} <{}>", c.name, c.mail);
            let fuzzy_score = if query_trimmed.is_empty() {
                Some(0)
            } else {
                // filter_map: None drops contacts with zero fuzzy score.
                matcher.fuzzy_match(&haystack, query_trimmed)
            }?;
            let score = (c.occurrences as i64) * 100 + fuzzy_score * 2;
            Some((score, c))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));

    if scored.is_empty() {
        // Don't print anything – mail clients interpret stdout as results.
        // Silence means "no matches" , which is the correct signal.
        return Ok(());
    }

    // ── Output formats ───────────────────────────────────────────────────
    //
    // mutt: tab-separated email, name (mutt's query_command protocol)
    // aerc: "Name email<TAB>email" (aerc's address-book-cmd protocol)
    // default: "Name <email>" (human-readable, also used by the TUI)

    match format {
        "mutt" => {
            for (_, c) in scored {
                println!("{}\t{}", c.mail, c.name);
            }
        }
        "aerc" => {
            for (_, c) in scored {
                // aerc wants: optional-name email TAB email
                // Names containing special characters get quoted.
                let name_part = if c.name.is_empty() {
                    String::new()
                } else if c.name.contains(',') || c.name.contains(';') || c.name.contains('"') {
                    format!("\"{} \" ", c.name.replace('"', "\\\""))
                } else {
                    format!("{} ", c.name)
                };
                println!("{}{}\t{}", name_part, c.mail, c.mail);
            }
        }
        _ => {
            for (_, c) in scored {
                println!("{}", c.display_name());
            }
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_email_lowercases() {
        assert_eq!(normalize_email("TEST@EXAMPLE.COM"), "test@example.com");
    }

    #[test]
    fn test_normalize_email_trims() {
        assert_eq!(normalize_email(" a@b.com "), "a@b.com");
    }

    #[test]
    fn test_normalize_name_strips_quotes() {
        assert_eq!(normalize_name("\"John Doe\""), "John Doe");
    }

    #[test]
    fn test_normalize_name_capitalises_all_lower() {
        assert_eq!(normalize_name("john doe"), "John doe");
    }

    #[test]
    fn test_normalize_name_leaves_mixed_case_alone() {
        assert_eq!(normalize_name("John DOE"), "John DOE");
    }

    #[test]
    fn test_normalize_name_strips_trailing_punctuation() {
        assert_eq!(normalize_name("Hamster McWheel,"), "Hamster McWheel");
    }

    #[test]
    fn test_extract_contacts_name_and_angle_brackets() {
        let contacts = extract_contacts("John Doe <john@example.com>");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].0, "John Doe");
        assert_eq!(contacts[0].1, "john@example.com");
    }

    #[test]
    fn test_extract_contacts_bare_email() {
        let contacts = extract_contacts("bare@example.com");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].0, "");
        assert_eq!(contacts[0].1, "bare@example.com");
    }

    #[test]
    fn test_extract_contacts_multiple() {
        let contacts = extract_contacts("John Doe <john@example.com>, Jane Doe <jane@example.com>");
        assert_eq!(contacts.len(), 2);
    }

    #[test]
    fn test_extract_contacts_ignores_non_email() {
        let contacts = extract_contacts("not an email at all, also not");
        assert!(contacts.is_empty());
    }

    #[test]
    fn test_extract_contacts_normalises_email() {
        let contacts = extract_contacts("UPPER@CASE.COM");
        assert_eq!(contacts[0].1, "upper@case.com");
    }
}
