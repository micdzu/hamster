// tests_correctness.rs – Tier‑1 correctness tests
//
// Every test creates its own tiny Maildir and config using explicit
// paths.  No global $HOME, no set_var, no thread conflicts.

use crate::folder_tags;
use crate::index;
use crate::index_access::IndexAccess;
use crate::setup::{FolderRule, FolderTagsConfig, HamsterConfig};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tantivy::schema::Value;
use tempfile::TempDir;

// ─── Helpers ────────────────────────────────────────────────────────────────

fn create_email(maildir_root: &std::path::Path, folder: &str, msg_id: &str, flags: &str) {
    let dir = maildir_root.join("test").join(folder);
    fs::create_dir_all(&dir).unwrap();

    let file_name = if flags.is_empty() {
        format!("{}.eml", msg_id)
    } else {
        format!("{}:2,{}", msg_id, flags)
    };

    let mut file = fs::File::create(dir.join(file_name)).unwrap();
    writeln!(
        file,
        "From: Sender <sender@example.com>\r\n\
         To: Recipient <recipient@example.com>\r\n\
         Subject: Test {}\r\n\
         Message-ID: <{}>\r\n\
         Date: Mon, 01 Jan 2024 00:00:00 +0000\r\n\
         \r\n\
         Body of the message\r\n",
        msg_id, msg_id
    )
    .unwrap();
}

fn setup_temp_home() -> (TempDir, HamsterConfig) {
    let home = TempDir::new().expect("temp home");
    let maildir = home.path().join("mail");
    fs::create_dir_all(&maildir).expect("create maildir root");

    let config_file = home.path().join(".hamster.toml");
    let index_dir = home.path().join(".hamster_index");

    let mut config = HamsterConfig::with_paths(
        "Test Hamster",
        "test@hamster.dev",
        &maildir.to_string_lossy(),
        config_file,
        index_dir,
    );

    // Write the config so folder_tags can read it later.
    let toml_str = toml::to_string_pretty(&config).unwrap();
    fs::write(&config.config_file, toml_str).unwrap();

    (home, config)
}

fn run_index(config: &HamsterConfig) {
    index::run(config, None).expect("index failed");
}

fn open_index(config: &HamsterConfig) -> IndexAccess {
    IndexAccess::open(&config.index_dir).expect("open index")
}

fn get_tags_for_msg(ia: &IndexAccess, msg_id: &str) -> Vec<String> {
    let term = tantivy::Term::from_field_text(ia.message_id, msg_id);
    let query = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
    let searcher = ia.searcher().unwrap();
    let docs = searcher
        .search(&query, &tantivy::collector::TopDocs::with_limit(1))
        .unwrap();
    if let Some((_, doc_addr)) = docs.first() {
        let doc = searcher.doc::<tantivy::TantivyDocument>(*doc_addr).unwrap();
        doc.get_all(ia.tags)
            .flat_map(|v| v.as_str())
            .map(String::from)
            .collect()
    } else {
        vec![]
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn test_index_single_message() {
    let (_home, config) = setup_temp_home();
    create_email(&PathBuf::from(&config.maildir), "new", "msg1", "");
    run_index(&config);
    let ia = open_index(&config);
    let searcher = ia.searcher().unwrap();
    let all_docs = searcher
        .search(
            &tantivy::query::AllQuery,
            &tantivy::collector::DocSetCollector,
        )
        .unwrap();
    assert_eq!(all_docs.len(), 1);
}

#[test]
fn test_incremental_index_no_duplicates() {
    let (_home, config) = setup_temp_home();
    create_email(&PathBuf::from(&config.maildir), "new", "msg1", "");
    run_index(&config);
    let ia = open_index(&config);
    let count = ia
        .searcher()
        .unwrap()
        .search(
            &tantivy::query::AllQuery,
            &tantivy::collector::DocSetCollector,
        )
        .unwrap()
        .len();
    run_index(&config);
    let ia = open_index(&config);
    let count2 = ia
        .searcher()
        .unwrap()
        .search(
            &tantivy::query::AllQuery,
            &tantivy::collector::DocSetCollector,
        )
        .unwrap()
        .len();
    assert_eq!(count, 1);
    assert_eq!(count, count2);
}

#[test]
fn test_deletion_handling() {
    let (_home, config) = setup_temp_home();
    create_email(&PathBuf::from(&config.maildir), "new", "msg1", "");
    run_index(&config);
    fs::remove_file(PathBuf::from(&config.maildir).join("test/new/msg1.eml")).unwrap();
    run_index(&config);
    let ia = open_index(&config);
    let all_docs = ia
        .searcher()
        .unwrap()
        .search(
            &tantivy::query::AllQuery,
            &tantivy::collector::DocSetCollector,
        )
        .unwrap();
    assert!(all_docs.is_empty());
}

#[test]
fn test_maildir_move_new_to_cur() {
    let (_home, config) = setup_temp_home();
    let maildir = PathBuf::from(&config.maildir);
    create_email(&maildir, "new", "msg1", "");
    run_index(&config);
    let ia = open_index(&config);
    let tags = get_tags_for_msg(&ia, "msg1");
    assert!(tags.iter().any(|t| t == "unread"));

    fs::remove_file(maildir.join("test/new/msg1.eml")).unwrap();
    create_email(&maildir, "cur", "msg1", "S");
    run_index(&config);
    let ia = open_index(&config);
    let tags = get_tags_for_msg(&ia, "msg1");
    assert!(!tags.iter().any(|t| t == "unread"));
}

#[test]
fn test_folder_sync_basic() {
    let (_home, mut config) = setup_temp_home();
    let maildir = PathBuf::from(&config.maildir);

    let inbox_dir = maildir.join("INBOX/new");
    fs::create_dir_all(&inbox_dir).unwrap();
    let mut file = fs::File::create(inbox_dir.join("msg2.eml")).unwrap();
    writeln!(
        file,
        "From: Sender <sender@example.com>\r\n\
         To: Recipient <recipient@example.com>\r\n\
         Subject: Inbox test\r\n\
         Message-ID: <msg2>\r\n\
         Date: Mon, 01 Jan 2024 00:00:00 +0000\r\n\
         \r\n\
         Body\r\n"
    )
    .unwrap();

    run_index(&config);

    // Enable folder_tags
    config.folder_tags = FolderTagsConfig {
        enabled: true,
        managed_tags: vec![],
        sync_flags: false,
        rules: vec![FolderRule {
            pattern: "INBOX".into(),
            tags: vec!["inbox".into()],
            inherit: false,
        }],
    };
    // Save updated config so folder_tags::sync can read it.
    fs::write(
        &config.config_file,
        toml::to_string_pretty(&config).unwrap(),
    )
    .unwrap();

    folder_tags::sync(&config, false, true).expect("folder sync failed");

    let ia = open_index(&config);
    let tags = get_tags_for_msg(&ia, "msg2");
    assert!(tags.iter().any(|t| t == "inbox"));
}
