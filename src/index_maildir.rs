// index_maildir.rs – Maildir scanning, flag parsing, and tag conversion.
//
// Also home to the shared folder‑discovery helper, because hamsters
// hate duplication.
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// Map of Maildir flag characters to tag names.
// 🐹 The hamster’s secret decoder ring.
pub const FLAG_TO_TAG: &[(char, &str)] = &[
    ('D', "draft"),
    ('F', "flagged"),
    ('P', "passed"),
    ('R', "replied"),
    ('T', "trashed"),
];

// Parse Maildir flags from a file path (the part after `:2,`).
pub fn parse_maildir_flags(path: &str) -> HashSet<char> {
    let mut flags = HashSet::new();
    if path.contains("/cur/") {
        if let Some(colon_idx) = path.rfind(':') {
            let flag_part = &path[colon_idx + 1..];
            if let Some(comma_idx) = flag_part.find(',') {
                for ch in flag_part[comma_idx + 1..].chars() {
                    flags.insert(ch);
                }
            }
        }
    }
    flags
}

// Extract just the flag suffix (e.g. "FS") from a Maildir path.
pub fn flag_suffix(path: &str) -> String {
    if path.contains("/cur/") {
        if let Some(colon_idx) = path.rfind(':') {
            let flag_part = &path[colon_idx + 1..];
            if let Some(comma_idx) = flag_part.find(',') {
                return flag_part[comma_idx + 1..].to_string();
            }
        }
    }
    String::new()
}

// Turn Maildir flags into tag strings.
pub fn flags_to_tags(flags: &HashSet<char>) -> Vec<String> {
    let mut tags = Vec::new();
    for (flag_char, tag_name) in FLAG_TO_TAG {
        if flags.contains(flag_char) {
            tags.push(tag_name.to_string());
        }
    }
    if !flags.contains(&'S') {
        tags.push("unread".to_string());
    }
    tags
}

// Check if a directory entry should be skipped (hidden files, notmuch cache).
pub fn should_skip_entry(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name();
    if name == ".notmuch" {
        return true;
    }
    if name.to_str().map(|s| s.starts_with('.')).unwrap_or(false) {
        return true;
    }
    false
}

// Discover all Maildir folders (relative paths) under `root`.
// This is the one true folder‑sniffing function; everyone should use it.
pub fn discover_maildir_folders(root: &Path) -> Result<Vec<PathBuf>> {
    let mut folders = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_dir() && path.join("cur").is_dir() && path.join("new").is_dir() {
            let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
            folders.push(rel);
        }
    }
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_maildir_flags_empty() {
        let flags = parse_maildir_flags("/maildir/new/123456.eml");
        assert!(flags.is_empty());
    }

    #[test]
    fn test_parse_maildir_flags_seen() {
        let flags = parse_maildir_flags("/maildir/cur/123456.eml:2,S");
        assert!(flags.contains(&'S'));
        assert!(!flags.contains(&'F'));
    }

    #[test]
    fn test_parse_maildir_flags_multiple() {
        let flags = parse_maildir_flags("/maildir/cur/123456.eml:2,FS");
        assert!(flags.contains(&'S'));
        assert!(flags.contains(&'F'));
    }

    #[test]
    fn test_flag_suffix_cur() {
        assert_eq!(flag_suffix("/maildir/cur/msg:2,FS"), "FS");
    }

    #[test]
    fn test_flag_suffix_new() {
        assert_eq!(flag_suffix("/maildir/new/msg"), "");
    }

    #[test]
    fn test_flags_to_tags_unread() {
        let flags: HashSet<char> = HashSet::new();
        let tags = flags_to_tags(&flags);
        assert!(tags.contains(&"unread".to_string()));
    }

    #[test]
    fn test_flags_to_tags_seen() {
        let flags: HashSet<char> = ['S'].iter().cloned().collect();
        let tags = flags_to_tags(&flags);
        assert!(!tags.contains(&"unread".to_string()));
    }

    #[test]
    fn test_flags_to_tags_flagged() {
        let flags: HashSet<char> = ['S', 'F'].iter().cloned().collect();
        let tags = flags_to_tags(&flags);
        assert!(tags.contains(&"flagged".to_string()));
        assert!(!tags.iter().any(|t| t == "replied"));
    }
}
