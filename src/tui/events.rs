// tui/events.rs – Keyboard event handling (v7 – modal overlays, no external ia)
//
// All methods that used to take an IndexAccess borrow now live on App.
// This file simply delegates to App's methods – no Tantivy touching needed.
//
// Overlays are now truly modal: when help, query help, explain, or the tag
// prompt is active, no normal key handling leaks through. The hamster believes
// in focus.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;

use super::state::{App, Focus, InputMode};
use crate::setup::HamsterConfig;

pub fn handle_key(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    key: KeyEvent,
    config: &HamsterConfig,
) -> Result<bool> {
    // ── Layer 0: Tag‑input mode (modal popup) ─────────────────────────
    // This is the innermost modal. Nothing else gets through.
    if app.input_mode != InputMode::Normal {
        match key.code {
            KeyCode::Char(c) => {
                app.tag_input.push(c);
            }
            KeyCode::Backspace => {
                app.tag_input.pop();
            }
            KeyCode::Enter => {
                let tag = app.tag_input.clone();
                let add = app.input_mode == InputMode::AddTag;
                app.input_mode = InputMode::Normal;
                app.tag_input.clear();
                if let Err(e) = app.apply_tag(add, &tag) {
                    app.status_message = Some(format!("⚠ {}", e));
                }
            }
            KeyCode::Esc => {
                app.input_mode = InputMode::Normal;
                app.tag_input.clear();
            }
            _ => {}
        }
        return Ok(false);
    }

    // ── Layer 1: Overlays (help, query help, explain) ───────────────
    // Any keypress dismisses the overlay. Nothing else happens.
    if app.show_help || app.show_query_help || app.explain_text.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('g') => {
                // Dismiss whatever is open
                app.show_help = false;
                app.show_query_help = false;
                app.explain_text = None;
            }
            _ => {
                // Any other key also dismisses – the hamster is decisive
                app.show_help = false;
                app.show_query_help = false;
                app.explain_text = None;
            }
        }
        return Ok(false);
    }

    // ── Layer 2: Global shortcuts (work from any focus) ─────────────
    match key.code {
        KeyCode::Char('q') if key.modifiers == KeyModifiers::CONTROL => {
            return Ok(true);
        }
        KeyCode::Tab => {
            app.cycle_focus();
            return Ok(false);
        }
        _ => {}
    }

    // ── Layer 3: Focus‑specific handling ────────────────────────────
    match app.focus {
        Focus::Filter => handle_filter_focus(app, terminal, key),
        Focus::Search => handle_search_focus(app, terminal, key, config),
        Focus::List => handle_list_focus(app, terminal, key, config),
    }
}

// ── Filter pane ──────────────────────────────────────────────────────

fn handle_filter_focus(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    key: KeyEvent,
) -> Result<bool> {
    let term_height = terminal.size().map(|s| s.height).unwrap_or(24) as usize;
    let visible = term_height.saturating_sub(8);

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if app.filter_selected + 1 < app.filter_items.len() {
                app.filter_selected += 1;
                app.scroll_filter_into_view(visible);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.filter_selected > 0 {
                app.filter_selected -= 1;
                app.scroll_filter_into_view(visible);
            }
        }
        KeyCode::Enter => {
            if let Err(e) = app.apply_filter_from_index() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        KeyCode::Esc => {
            if let Err(e) = app.clear_filter() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        _ => {}
    }
    Ok(false)
}

// ── Search pane ───────────────────────────────────────────────────────

fn handle_search_focus(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    key: KeyEvent,
    _config: &HamsterConfig,
) -> Result<bool> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.is_empty() => {
            if c == '?' {
                app.show_help = true;
            } else {
                app.search_input.push(c);
                app.pending_search = Some(std::time::Instant::now());
            }
        }
        KeyCode::Enter => {
            app.push_to_history();
            app.pending_search = None;
            if let Err(e) = app.search() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        KeyCode::Backspace => {
            app.search_input.pop();
            app.pending_search = Some(std::time::Instant::now());
        }
        KeyCode::Esc => {
            if !app.search_input.is_empty() {
                app.search_input.clear();
                app.query_error = None;
                app.parsed_query = None;
                if let Err(e) = app.search() {
                    app.status_message = Some(format!("⚠ {}", e));
                }
            }
        }
        // Navigation keys also work in search focus (convenience)
        KeyCode::Up | KeyCode::Char('k') => {
            let h = list_height(terminal);
            app.move_up(h);
            app.refresh_preview();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let h = list_height(terminal);
            app.move_down(h);
            app.refresh_preview();
        }
        KeyCode::Char('j') if key.modifiers == KeyModifiers::CONTROL => {
            let h = list_height(terminal);
            app.move_down(h);
            app.refresh_preview();
        }
        KeyCode::Char('k') if key.modifiers == KeyModifiers::CONTROL => {
            let h = list_height(terminal);
            app.move_up(h);
            app.refresh_preview();
        }
        KeyCode::PageUp | KeyCode::Char('b') if key.modifiers == KeyModifiers::CONTROL => {
            let page = list_height(terminal);
            app.selected = app.selected.saturating_sub(page);
            app.list_offset = app.list_offset.saturating_sub(page);
            app.refresh_preview();
        }
        KeyCode::PageDown | KeyCode::Char('f') if key.modifiers == KeyModifiers::CONTROL => {
            let page = list_height(terminal);
            let max = app.results.len().saturating_sub(1);
            app.selected = (app.selected + page).min(max);
            app.list_offset = (app.list_offset + page).min(max);
            app.refresh_preview();
        }
        KeyCode::Home => {
            app.selected = 0;
            app.list_offset = 0;
            app.refresh_preview();
        }
        KeyCode::End => {
            let last = app.results.len().saturating_sub(1);
            app.selected = last;
            app.list_offset = last.saturating_sub(list_height(terminal));
            app.refresh_preview();
        }
        KeyCode::Char('h') if key.modifiers == KeyModifiers::CONTROL => {
            if let Err(e) = app.history_backward() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        KeyCode::Char('g') if key.modifiers == KeyModifiers::CONTROL => {
            app.show_query_help = true;
        }
        _ => {}
    }
    Ok(false)
}

// ── List pane ──────────────────────────────────────────────────────────

fn handle_list_focus(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    key: KeyEvent,
    config: &HamsterConfig,
) -> Result<bool> {
    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Char('n') | KeyCode::Down
            if key.modifiers == KeyModifiers::CONTROL || key.modifiers.is_empty() =>
        {
            let h = list_height(terminal);
            app.move_down(h);
            app.refresh_preview();
        }
        KeyCode::Char('k') | KeyCode::Up
            if key.modifiers == KeyModifiers::CONTROL || key.modifiers.is_empty() =>
        {
            let h = list_height(terminal);
            app.move_up(h);
            app.refresh_preview();
        }
        KeyCode::Char('f') | KeyCode::PageDown if key.modifiers == KeyModifiers::CONTROL => {
            let page = list_height(terminal);
            let max = app.results.len().saturating_sub(1);
            app.selected = (app.selected + page).min(max);
            app.list_offset = (app.list_offset + page).min(max);
            app.refresh_preview();
        }
        KeyCode::Char('b') | KeyCode::PageUp if key.modifiers == KeyModifiers::CONTROL => {
            let page = list_height(terminal);
            app.selected = app.selected.saturating_sub(page);
            app.list_offset = app.list_offset.saturating_sub(page);
            app.refresh_preview();
        }
        KeyCode::Home => {
            app.selected = 0;
            app.list_offset = 0;
            app.refresh_preview();
        }
        KeyCode::End => {
            let last = app.results.len().saturating_sub(1);
            app.selected = last;
            app.list_offset = last.saturating_sub(list_height(terminal));
            app.refresh_preview();
        }

        // Tagging
        KeyCode::Char('t') if key.modifiers == KeyModifiers::CONTROL => {
            app.input_mode = InputMode::AddTag;
            app.tag_input.clear();
        }
        KeyCode::Char('r') if key.modifiers == KeyModifiers::CONTROL => {
            let has_tags = app
                .results
                .get(app.selected)
                .map(|r| !r.tags.is_empty())
                .unwrap_or(false);
            if has_tags {
                app.input_mode = InputMode::RemoveTag;
                app.tag_input.clear();
            }
        }
        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
            if let Err(e) = app.toggle_read_status() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        KeyCode::Char('a') if key.modifiers == KeyModifiers::CONTROL => {
            if let Err(e) = app.archive_message() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }

        // Actions
        KeyCode::Char('y') if key.modifiers == KeyModifiers::CONTROL => {
            app.yank_message_id();
        }
        KeyCode::Char('o') if key.modifiers == KeyModifiers::CONTROL => {
            if let Err(e) = app.open_in_pager() {
                app.status_message = Some(format!("⚠ {}", e));
            }
            terminal.clear()?;
        }
        KeyCode::Char('e') if key.modifiers == KeyModifiers::CONTROL => {
            app.toggle_preview_visibility();
        }
        KeyCode::Char('x') if key.modifiers == KeyModifiers::CONTROL => {
            if let Err(e) = app.toggle_explain(config) {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        KeyCode::Char('g') if key.modifiers == KeyModifiers::CONTROL => {
            app.show_query_help = true;
        }
        KeyCode::Char('h') if key.modifiers == KeyModifiers::CONTROL => {
            if let Err(e) = app.history_backward() {
                app.status_message = Some(format!("⚠ {}", e));
            }
        }
        _ => {}
    }
    Ok(false)
}

fn list_height(terminal: &Terminal<CrosstermBackend<io::Stdout>>) -> usize {
    let h = terminal.size().map(|s| s.height).unwrap_or(24) as usize;
    // ui.rs reserves: search(4) + status(1) + footer(1) = 6 rows
    h.saturating_sub(6)
}
