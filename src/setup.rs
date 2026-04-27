// setup.rs – The Hamster's welcoming ritual (now aware of past visits)
//
// If you've never met, the hamster guides you through a friendly first
// setup.  But if you return, it remembers you and offers to tweak any
// setting – no need to start over.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index;
use crate::index_maildir; // shared folder discovery

// ── Hamster’s master config ───────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HamsterConfig {
    pub name: String,
    pub primary_email: String,
    pub maildir: String,
    #[serde(default)]
    pub folder_tags: FolderTagsConfig,
    // Paths – set by the CLI layer so core code doesn’t touch $HOME.
    #[serde(skip)]
    pub config_file: PathBuf,
    #[serde(skip)]
    pub index_dir: PathBuf,
}

impl HamsterConfig {
    // Minimal config with explicit paths – perfect for tests.
    #[cfg(test)]
    pub fn with_paths(
        name: &str,
        email: &str,
        maildir: &str,
        config_file: PathBuf,
        index_dir: PathBuf,
    ) -> Self {
        Self {
            name: name.to_string(),
            primary_email: email.to_string(),
            maildir: maildir.to_string(),
            folder_tags: FolderTagsConfig::default(),
            config_file,
            index_dir,
        }
    }

    // Derive default paths from $HOME.
    pub fn default_paths() -> (PathBuf, PathBuf) {
        let home = directories::BaseDirs::new()
            .expect("Could not determine home directory")
            .home_dir()
            .to_path_buf();
        (home.join(".hamster.toml"), home.join(".hamster_index"))
    }
}

impl Default for HamsterConfig {
    fn default() -> Self {
        let (config_file, index_dir) = Self::default_paths();
        Self {
            name: "Hamster User".into(),
            primary_email: "hamster@example.com".into(),
            maildir: "./mail".into(),
            folder_tags: FolderTagsConfig::default(),
            config_file,
            index_dir,
        }
    }
}

// ── Folder‑tag configuration ──────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct FolderTagsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub managed_tags: Vec<String>,
    #[serde(default)]
    pub sync_flags: bool,
    #[serde(default)]
    pub rules: Vec<FolderRule>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FolderRule {
    pub pattern: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub inherit: bool,
}

// ── Public entry point ─────────────────────────────────────────────────────

// If `existing` is provided, we enter tweak mode.  Otherwise, a friendly
// first‑time walkthrough is shown.
pub fn run(existing: Option<HamsterConfig>) -> Result<()> {
    if let Some(mut cfg) = existing {
        tweak_existing(&mut cfg)?;
    } else {
        print_welcome();

        let mut config = minimal_setup()?;
        confirm_config(&config)?;
        save_config(&config)?;
        first_success(&config)?;
        optional_enhancements(&mut config)?;
        print_goodbye();
    }
    Ok(())
}

// ── Fresh‑install steps ────────────────────────────────────────────────────

fn print_welcome() {
    println!(
        r#"
🐹 Welcome to hamster.

I'll help you turn your Maildir into a fast, searchable system.
This should take about 30 seconds (depending on your emotional readiness).
"#
    );
}

fn minimal_setup() -> Result<HamsterConfig> {
    println!("🐹 Let's start with the basics.\n");

    let name = prompt("Your name")?;
    let email = prompt("Primary email")?;
    let maildir = prompt_existing_path("Maildir path")?;

    let (config_file, index_dir) = HamsterConfig::default_paths();

    Ok(HamsterConfig {
        name,
        primary_email: email,
        maildir: maildir.canonicalize()?.to_string_lossy().into(),
        folder_tags: FolderTagsConfig::default(),
        config_file,
        index_dir,
    })
}

fn confirm_config(config: &HamsterConfig) -> Result<()> {
    println!(
        r#"
🐹 Looks good to me:

Name:   {}
Email:  {}
Maildir:{}

Proceed? [Y/n]
"#,
        config.name, config.primary_email, config.maildir
    );

    if !confirm()? {
        bail!("Setup aborted by user (the hamster respects your decision)");
    }

    Ok(())
}

fn save_config(config: &HamsterConfig) -> Result<()> {
    let path = &config.config_file;
    let toml = toml::to_string_pretty(config)?;
    fs::write(path, toml)
        .with_context(|| format!("Failed to write config to {}", path.display()))?;

    println!("🐹 Saved config to {}\n", path.display());

    Ok(())
}

fn first_success(config: &HamsterConfig) -> Result<()> {
    println!("🐹 Time for the first test run.\n");
    println!("🐹 Indexing your mail...");

    index::run(config, None)?;

    println!("\n✔ Indexing complete.\n");

    println!(
        r#"
🐹 Try this:

    hamster search "from:boss"

(If you don't have a boss, try "subject:hello". If that also fails, we need to talk.)
"#
    );

    Ok(())
}

fn optional_enhancements(config: &mut HamsterConfig) -> Result<()> {
    println!(
        r#"
🐹 Want to improve things further?

[1] Set up folder-based tags (recommended)
[2] Enable Maildir flag sync
[3] Skip for now
"#
    );

    let choice = prompt("Choose [1/2/3]")?;

    match choice.trim() {
        "1" => auto_folder_tags(config)?,
        "2" => enable_flag_sync(config)?,
        _ => {
            println!("🐹 Alright. Keeping things simple. I respect that.\n");
        }
    }

    Ok(())
}

fn print_goodbye() {
    println!(
        r#"
🐹 You're ready.

Useful commands:

  hamster search "..."
  hamster address "..."
  hamster tag +important "query"
  hamster folder sync          ← keep tags aligned with your folders

If something feels wrong:

  hamster folder explain <path>

And remember:
You can rerun `hamster setup` anytime. I won't take it personally.

Now go. Organize your inbox. Or ignore it more efficiently.
"#
    );
}

// ── Tweak‑existing mode ────────────────────────────────────────────────────

fn tweak_existing(config: &mut HamsterConfig) -> Result<()> {
    println!(
        "🐹 It looks like you already have a config at {:?}.",
        config.config_file
    );
    println!("Let's tweak your settings.\n");

    loop {
        // Brief summary
        println!("Current settings:");
        println!("  Name:        {}", config.name);
        println!("  Email:       {}", config.primary_email);
        println!("  Maildir:     {}", config.maildir);
        println!(
            "  Folder tags: {}",
            if config.folder_tags.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "  Flag sync:   {}",
            if config.folder_tags.sync_flags {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!("  Folder rules: {} rule(s)", config.folder_tags.rules.len());

        println!("\nWhat would you like to tweak?");
        println!("  [1] Change name/email");
        println!("  [2] Change Maildir path");
        println!(
            "  [3] Toggle folder tags (currently {})",
            if config.folder_tags.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "  [4] Toggle flag sync (currently {})",
            if config.folder_tags.sync_flags {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!("  [5] Re‑scan folders and auto‑generate rules (replaces existing rules)");
        println!("  [6] Re‑index mail now");
        println!("  [7] Quit and save\n");

        let choice = prompt("Your choice")?;
        match choice.trim() {
            "1" => {
                let name = prompt_with_default("Name", &config.name)?;
                let email = prompt_with_default("Email", &config.primary_email)?;
                config.name = name;
                config.primary_email = email;
                println!("✓ Updated.\n");
            }
            "2" => {
                let new_path = prompt_existing_path("Maildir path")?;
                config.maildir = new_path.to_string_lossy().into();
                println!("✓ Maildir updated.\n");
            }
            "3" => {
                config.folder_tags.enabled = !config.folder_tags.enabled;
                println!(
                    "✓ Folder tags are now {}.\n",
                    if config.folder_tags.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
            }
            "4" => {
                config.folder_tags.sync_flags = !config.folder_tags.sync_flags;
                println!(
                    "✓ Flag sync is now {}.\n",
                    if config.folder_tags.sync_flags {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
            }
            "5" => {
                // Re‑scan folders and replace rules using the same helper
                auto_folder_tags(config)?;
            }
            "6" => {
                println!("🐹 Running index...");
                crate::index::run(config, None)?;
                println!("✔ Indexing complete.\n");
            }
            "7" => {
                save_config(config)?;
                println!("🐹 Config saved. Goodbye!\n");
                break;
            }
            _ => println!("🐹 Hmm, didn't understand that. Try a number between 1 and 7.\n"),
        }
    }
    Ok(())
}

// ── Folder tag helpers (same as before) ────────────────────────────────────

fn auto_folder_tags(config: &mut HamsterConfig) -> Result<()> {
    let mail_root = Path::new(&config.maildir);
    if !mail_root.exists() {
        bail!("Maildir path does not exist – cannot scan folders.");
    }

    let folders = match index_maildir::discover_maildir_folders(mail_root) {
        Ok(f) => f,
        Err(_) => {
            println!("🐹 Couldn't scan your folders. Skipping.");
            return Ok(());
        }
    };

    if folders.is_empty() {
        println!("🐹 No Maildir folders found. Skipping.");
        return Ok(());
    }

    let mut suggested_rules: Vec<FolderRule> = Vec::new();
    for folder_rel in &folders {
        let pattern = folder_rel.to_string_lossy().into_owned();
        let tags = suggest_tags(&pattern);
        if let Err(e) = crate::validation::validate_tags(&tags) {
            println!(
                "🐹 Invalid tag(s) for folder '{}': {}. Skipping.",
                pattern, e
            );
            continue;
        }
        if !tags.is_empty() {
            suggested_rules.push(FolderRule {
                pattern,
                tags,
                inherit: false,
            });
        }
    }

    println!(
        "\n🐹 I found {} folder(s). Here's what I'd suggest:\n",
        folders.len()
    );
    for rule in &suggested_rules {
        println!("  {} → [{}]", rule.pattern, rule.tags.join(", "));
    }

    println!("\nApply these rules? [Y/n]");
    if confirm()? {
        config.folder_tags.rules = suggested_rules;
        config.folder_tags.enabled = true;
        save_config(config)?;
        println!("🐹 Folder rules applied and saved.\n");

        println!("🐹 Would you like to run `hamster folder sync` now? [Y/n]");
        if confirm()? {
            crate::folder_tags::sync(config, false, false)?;
        }
    } else {
        println!("🐹 Skipped. You can set up folder tags later with `hamster folder sync`.\n");
    }

    Ok(())
}

fn enable_flag_sync(config: &mut HamsterConfig) -> Result<()> {
    println!(
        r#"
🐹 I can keep read/unread and flags (like replied, flagged) in sync automatically.
This means if you read a message, the unread tag will disappear.

Enable this? (recommended) [Y/n]
"#
    );

    if confirm()? {
        config.folder_tags.sync_flags = true;
        save_config(config)?;
        println!("🐹 Flag sync enabled and saved.\n");
    } else {
        println!("🐹 Alright, manual control it is.\n");
    }

    Ok(())
}

// ── Input helpers ──────────────────────────────────────────────────────────

fn prompt(label: &str) -> Result<String> {
    print!("{}: ", label);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().to_string())
}

fn prompt_with_default(label: &str, default: &str) -> Result<String> {
    print!("{} [{}]: ", label, default);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed)
    }
}

fn prompt_existing_path(label: &str) -> Result<PathBuf> {
    loop {
        let input = prompt(label)?;
        let path = PathBuf::from(&input);
        if path.exists() {
            return Ok(path);
        }
        println!("🐹 Path does not exist. Try again.");
    }
}

fn confirm() -> Result<bool> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let trimmed = input.trim().to_lowercase();
    Ok(trimmed.is_empty() || trimmed == "y" || trimmed == "yes")
}

// Suggest tags based on folder name (same heuristics as wizard).
fn suggest_tags(folder: &str) -> Vec<String> {
    let lower = folder.to_lowercase();
    let mut tags = Vec::new();
    if lower.contains("inbox") {
        tags.push("inbox".to_string());
    }
    if lower.contains("sent") {
        tags.push("sent".to_string());
    }
    if lower.contains("draft") {
        tags.push("draft".to_string());
    }
    if lower.contains("trash") || lower.contains("deleted") || lower.contains("bin") {
        tags.push("deleted".to_string());
    }
    if lower.contains("archive") {
        tags.push("archive".to_string());
    }
    if lower.contains("spam") || lower.contains("junk") {
        tags.push("spam".to_string());
    }
    use std::collections::HashSet;
    let mut uniq: Vec<String> = tags
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    uniq.sort();
    uniq
}

// ── Tests ─────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_paths() {
        let config = HamsterConfig::default();
        assert!(config
            .config_file
            .to_string_lossy()
            .contains(".hamster.toml"));
        assert!(config
            .index_dir
            .to_string_lossy()
            .contains(".hamster_index"));
    }

    #[test]
    fn test_default_config() {
        let config = HamsterConfig::default();
        assert_eq!(config.name, "Hamster User");
        assert_eq!(config.primary_email, "hamster@example.com");
        assert_eq!(config.maildir, "./mail");
    }

    #[test]
    fn test_with_paths() {
        let config = HamsterConfig::with_paths(
            "Test",
            "test@test.com",
            "/tmp/mail",
            PathBuf::from("/tmp/.hamster.toml"),
            PathBuf::from("/tmp/.hamster_index"),
        );
        assert_eq!(config.name, "Test");
        assert_eq!(config.config_file, PathBuf::from("/tmp/.hamster.toml"));
        assert_eq!(config.index_dir, PathBuf::from("/tmp/.hamster_index"));
    }
}
