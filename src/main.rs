// main.rs – The Hamster Triumvirate (now with explicit paths, resilient loading)

mod address;
mod address_cache;
mod diff;
mod folder_tags;
mod index;
mod index_access;
mod index_core;
mod index_maildir;
mod index_metadata;
mod index_text;
mod search;
mod setup;
mod stats;
mod tag;
mod tui;
mod validation;

#[cfg(test)]
mod tests_correctness;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use setup::HamsterConfig;

#[derive(Parser, Debug)]
#[command(
    name = "hamster",
    about = "🐹 A fast, fuzzy, furry Notmuch replacement.",
    version = env!("CARGO_PKG_VERSION")
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Setup,
    Index {
        #[arg(short, long)]
        maildir: Option<String>,
    },
    Search {
        #[arg(short, long, default_value = "pretty")]
        format: String,
        query: Vec<String>,
    },
    Tag {
        #[arg(required = true)]
        tag_changes: Vec<String>,
        query: Vec<String>,
    },
    Edit,
    Stats,
    Address {
        #[arg(short, long, default_value = "default")]
        format: String,
        search_terms: Vec<String>,
    },
    Tui,
    #[command(subcommand)]
    Folder(FolderAction),
}

#[derive(Subcommand, Debug)]
enum FolderAction {
    Sync {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        quiet: bool,
    },
    Structure {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        quiet: bool,
    },
    Explain {
        identifier: String,
    },
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    // Load or create config using the default paths.
    // We always compute the index and config file locations from $HOME,
    // even if the config file exists – those paths shouldn’t be saved.
    let default_config = HamsterConfig::default();
    let cfg_path = default_config.config_file.clone();
    let config: HamsterConfig = if cfg_path.exists() {
        let contents = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("Failed to read config from {:?}", cfg_path))?;
        let mut loaded: HamsterConfig = toml::from_str(&contents)?;
        // Restore the runtime‑computed paths that serde skipped.
        loaded.config_file = default_config.config_file;
        loaded.index_dir = default_config.index_dir;
        loaded
    } else {
        default_config
    };

    match cli.command {
        Commands::Setup => {
            // If a config file already exists, hand it over so setup can tweak it.
            // Otherwise, we start fresh.
            if cfg_path.exists() {
                setup::run(Some(config))
            } else {
                setup::run(None)
            }
        }
        Commands::Index { maildir } => index::run(&config, maildir),
        Commands::Search { format, query } => search::run(&config, query.join(" "), &format),
        Commands::Stats => stats::run(&config),
        Commands::Tag { tag_changes, query } => tag::run(&config, tag_changes, query.join(" ")),
        Commands::Address {
            format,
            search_terms,
        } => address::run(&config, &format, search_terms.join(" ")),
        Commands::Tui => tui::run(&config),
        Commands::Edit => {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            std::process::Command::new(editor)
                .arg(&cfg_path)
                .status()
                .context("Failed to launch editor")?;
            Ok(())
        }
        Commands::Folder(action) => match action {
            FolderAction::Sync { dry_run, quiet } => folder_tags::sync(&config, dry_run, quiet),
            FolderAction::Structure { dry_run, quiet } => {
                folder_tags::sync_structure(&config, dry_run, quiet)
            }
            FolderAction::Explain { identifier } => folder_tags::explain(&config, &identifier),
        },
    }
}
