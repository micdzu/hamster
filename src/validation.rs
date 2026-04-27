// src/validation.rs – Tag name policing, notmuch‑style.

pub fn validate_tag(tag: &str) -> Result<(), String> {
    let actual = tag.strip_prefix('-').unwrap_or(tag);
    if actual.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }
    if actual.starts_with('-') {
        return Err("Tag cannot start with a hyphen after optional removal prefix".to_string());
    }
    if actual.contains(' ') {
        return Err("Tag cannot contain spaces".to_string());
    }
    if !actual
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Tag can only contain alphanumeric characters, hyphens, and underscores".to_string(),
        );
    }
    Ok(())
}

pub fn validate_tags(tags: &[String]) -> Result<(), String> {
    for tag in tags {
        validate_tag(tag)?;
    }
    Ok(())
}
