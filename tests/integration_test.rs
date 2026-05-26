// integration_test.rs – Pre-release validation tests
//
// These tests ensure critical validation logic works correctly
// before the first release.

use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_maildir_discovery_requires_all_three_dirs() {
    // Create temp directory
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Create incomplete maildir (missing tmp/)
    std::fs::create_dir_all(root.join("incomplete/cur")).unwrap();
    std::fs::create_dir_all(root.join("incomplete/new")).unwrap();

    let folders = hamster::index_maildir::discover_maildir_folders(root).unwrap();
    assert!(
        folders.is_empty(),
        "Incomplete maildir should not be discovered"
    );

    // Create complete maildir with all three dirs
    std::fs::create_dir_all(root.join("complete/cur")).unwrap();
    std::fs::create_dir_all(root.join("complete/new")).unwrap();
    std::fs::create_dir_all(root.join("complete/tmp")).unwrap();

    let folders = hamster::index_maildir::discover_maildir_folders(root).unwrap();
    let complete_found = folders
        .iter()
        .any(|f| f.file_name().map(|n| n == "complete").unwrap_or(false));
    assert!(complete_found, "Complete maildir should be discovered");
}

#[test]
fn test_config_validation_rejects_nonexistent_path() {
    let config = hamster::setup::HamsterConfig::with_paths(
        "Test",
        "test@test.com",
        "/nonexistent/path/that/does/not/exist",
        PathBuf::from("/tmp/test.toml"),
        PathBuf::from("/tmp/index"),
    );

    let result = hamster::validate_config(&config);
    assert!(
        result.is_err(),
        "Should reject nonexistent maildir path"
    );
}

#[test]
fn test_valid_config_passes_validation() {
    let temp = TempDir::new().unwrap();
    let config = hamster::setup::HamsterConfig::with_paths(
        "Test",
        "test@test.com",
        temp.path().to_str().unwrap(),
        PathBuf::from("/tmp/test.toml"),
        PathBuf::from("/tmp/index"),
    );

    let result = hamster::validate_config(&config);
    assert!(result.is_ok(), "Should accept valid config with existing dir");
}

#[test]
fn test_config_validation_rejects_file_path() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("notadir.txt");
    std::fs::write(&file_path, "test").unwrap();

    let config = hamster::setup::HamsterConfig::with_paths(
        "Test",
        "test@test.com",
        file_path.to_str().unwrap(),
        PathBuf::from("/tmp/test.toml"),
        PathBuf::from("/tmp/index"),
    );

    let result = hamster::validate_config(&config);
    assert!(
        result.is_err(),
        "Should reject file path instead of directory"
    );
}

#[test]
fn test_tag_validation_rejects_spaces() {
    let result = hamster::validation::validate_tag("my tag");
    assert!(
        result.is_err(),
        "Tags with spaces should be rejected"
    );
}

#[test]
fn test_tag_validation_rejects_empty() {
    let result = hamster::validation::validate_tag("");
    assert!(result.is_err(), "Empty tags should be rejected");
}

#[test]
fn test_tag_validation_accepts_valid_tags() {
    assert!(
        hamster::validation::validate_tag("inbox").is_ok(),
        "Valid tag 'inbox' should pass"
    );
    assert!(
        hamster::validation::validate_tag("my-tag").is_ok(),
        "Valid tag 'my-tag' should pass"
    );
    assert!(
        hamster::validation::validate_tag("tag_123").is_ok(),
        "Valid tag 'tag_123' should pass"
    );
}

#[test]
fn test_maildir_flag_parsing_comprehensive() {
    // Test various flag combinations
    let flags = hamster::index_maildir::parse_maildir_flags("/maildir/cur/msg:2,SFR");
    assert!(flags.contains(&'S'), "Should contain Seen flag");
    assert!(flags.contains(&'F'), "Should contain Flagged flag");
    assert!(flags.contains(&'R'), "Should contain Replied flag");
    assert!(!flags.contains(&'D'), "Should not contain Draft flag");
}
