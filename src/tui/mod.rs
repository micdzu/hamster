// tui/mod.rs – Hamster TUI entry point (v6 – lazy writer, StoredFields)
//
// This is where the hamster starts the interactive thinking space.
// It opens the index once, creates an App that owns the IndexAccess,
// runs the event loop, and ensures the writer is properly committed on exit.
// No file I/O, no MIME parsing – everything goes through Tantivy's stored fields.

mod events;
mod state;
mod ui;

use anyhow::Result;
use crossterm::event;
use std::io;
use std::time::Duration;

use crate::index_access::IndexAccess;
use crate::setup::HamsterConfig;
use state::App;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

pub fn run(config: &HamsterConfig) -> Result<()> {
    // Open the index. This is the only place we call IndexAccess::open in the TUI.
    let ia = IndexAccess::open(&config.index_dir)?;

    state::TerminalGuard::enter()?;
    let _guard = state::TerminalGuard;

    let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.clear()?;

    // App now owns the IndexAccess (and later the lazy writer).
    let mut app = App::new(ia);
    app.refresh_filter_items(config);
    // Run initial search to populate the list.
    app.search()?;

    // Main event loop
    loop {
        terminal.draw(|f| ui::render(f, &mut app))?;

        let timeout = app
            .pending_search
            .map(|t| SEARCH_DEBOUNCE.saturating_sub(t.elapsed()))
            .unwrap_or(Duration::from_secs(10));

        if event::poll(timeout)? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind == crossterm::event::KeyEventKind::Press {
                    let quit = events::handle_key(&mut app, &mut terminal, key, config)?;
                    if quit {
                        break;
                    }
                }
            }
        }

        if app
            .pending_search
            .map(|t| t.elapsed() >= SEARCH_DEBOUNCE)
            .unwrap_or(false)
        {
            app.search()?;
        }
    }

    // Commit the writer before we leave. The hamster likes tidy granaries.
    app.commit_and_close_writer()?;

    Ok(())
}
