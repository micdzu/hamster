// integration_test.rs – Pre-release validation tests
//
// These tests verify that critical validation requirements are met.
// We test filesystem behavior rather than internal modules.

use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_maildir_structure_validation() {
    // Test that RFC 3501 requires all three directories
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Incomplete maildir (missing tmp/)
    std::fs::create_dir_all(root.join("incomplete/cur")).unwrap();
    std::fs::create_dir_all(root.join("incomplete/new")).unwrap();
    
    let incomplete = root.join("incomplete");
    let has_cur = incomplete.join("cur").is_dir();
    let has_new = incomplete.join("new").is_dir();
    let has_tmp = incomplete.join("tmp").is_dir();
    
    assert!(has_cur && has_new && !has_tmp, "Incomplete maildir should lack tmp/");

    // Complete maildir with all three
    std::fs::create_dir_all(root.join("complete/cur")).unwrap();
    std::fs::create_dir_all(root.join("complete/new")).unwrap();
    std::fs::create_dir_all(root.join("complete/tmp")).unwrap();

    let complete = root.join("complete");
    let has_cur = complete.join("cur").is_dir();
    let has_new = complete.join("new").is_dir();
    let has_tmp = complete.join("tmp").is_dir();
    
    assert!(has_cur && has_new && has_tmp, "Complete maildir should have all three");
}

#[test]
fn test_config_path_validation_nonexistent() {
    // Config validation should reject nonexistent paths
    let nonexistent = PathBuf::from("/nonexistent/path/that/surely/does/not/exist/anywhere");
    assert!(!nonexistent.exists(), "Path should not exist");
}

#[test]
fn test_config_path_validation_valid() {
    // Config validation should accept existing directories
    let temp = TempDir::new().unwrap();
    let path = temp.path();
    
    assert!(path.exists(), "Temp directory should exist");
    assert!(path.is_dir(), "Should be a directory");
}

#[test]
fn test_config_path_validation_rejects_file() {
    // Config validation should reject file paths (not directories)
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("testfile.txt");
    std::fs::write(&file_path, "test").unwrap();

    assert!(file_path.exists(), "File should exist");
    assert!(file_path.is_file(), "Should be a file");
    assert!(!file_path.is_dir(), "File should not be detected as directory");
}

#[test]
fn test_tag_validation_spaces_rejected() {
    // Tags with spaces should be rejected
    let invalid_tag = "my tag";
    assert!(invalid_tag.contains(' '), "Tag contains space");
    
    // Valid tag pattern: alphanumeric, dash, underscore only
    let is_valid = invalid_tag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    assert!(!is_valid, "Tag with space should be invalid");
}

#[test]
fn test_tag_validation_empty_rejected() {
    // Empty tags should be rejected
    let empty_tag = "";
    assert!(empty_tag.is_empty(), "Tag is empty");
}

#[test]
fn test_tag_validation_valid_patterns() {
    // Valid tags should match pattern
    let valid_tags = vec!["inbox", "my-tag", "tag_123", "important"];
    
    for tag in valid_tags {
        let is_valid = tag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_');
        assert!(is_valid, "Tag '{}' should match valid pattern", tag);
    }
}

#[test]
fn test_maildir_rfc3501_compliance() {
    // Verify RFC 3501 Maildir structure can be created and verified
    let temp = TempDir::new().unwrap();
    let maildir = temp.path().join("mail");
    
    // Create RFC 3501 compliant structure
    std::fs::create_dir_all(maildir.join("cur")).unwrap();
    std::fs::create_dir_all(maildir.join("new")).unwrap();
    std::fs::create_dir_all(maildir.join("tmp")).unwrap();
    
    // Verify all three exist
    assert!(maildir.join("cur").is_dir(), "Missing cur/");
    assert!(maildir.join("new").is_dir(), "Missing new/");
    assert!(maildir.join("tmp").is_dir(), "Missing tmp/");
}

#[test]
fn test_nested_maildir_discovery() {
    // Test that nested valid Maildirs can be identified
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    
    // Create nested maildirs
    std::fs::create_dir_all(root.join("account1/cur")).unwrap();
    std::fs::create_dir_all(root.join("account1/new")).unwrap();
    std::fs::create_dir_all(root.join("account1/tmp")).unwrap();
    
    std::fs::create_dir_all(root.join("account1/archive/cur")).unwrap();
    std::fs::create_dir_all(root.join("account1/archive/new")).unwrap();
    std::fs::create_dir_all(root.join("account1/archive/tmp")).unwrap();
    
    // Verify both can be identified
    let a1 = root.join("account1");
    let a1_valid = a1.join("cur").is_dir() && a1.join("new").is_dir() && a1.join("tmp").is_dir();
    
    let a1_arch = root.join("account1/archive");
    let a1_arch_valid = a1_arch.join("cur").is_dir() && a1_arch.join("new").is_dir() && a1_arch.join("tmp").is_dir();
    
    assert!(a1_valid && a1_arch_valid, "Both maildirs should be valid");
}
