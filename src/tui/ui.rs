// tui/ui.rs – The Hamster’s Display Pen (v6 – Polished, Span-based, Syntax Highlighted)
//
// The hamster has learned to paint. Keywords glow, operators dim, and
// every character is placed with intention. No more monolithic format!()
// strings—everything is built from Spans for precise, beautiful control.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::state::{App, Focus, InputMode, MAX_SEARCH_RESULTS};

pub fn render(f: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // search bar + info
            Constraint::Min(0),
            Constraint::Length(1), // status (tags / messages)
            Constraint::Length(1), // footer hints
        ])
        .split(f.area());

    render_search_pane(f, app, root[0]);

    let (left_pct, center_pct, right_pct) = if app.preview_visible {
        (15, 30, 55)
    } else {
        (15, 85, 0)
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_pct),
            Constraint::Percentage(center_pct),
            Constraint::Percentage(right_pct),
        ])
        .split(root[1]);

    render_filter_pane(f, app, columns[0]);
    render_list_pane(f, app, columns[1]);

    if app.explain_text.is_some() {
        render_explain_pane(f, app, columns[2]);
    } else {
        render_preview_pane(f, app, columns[2]);
    }

    render_status_bar(f, app, root[2]);
    render_footer(f, app, root[3]);

    // Overlays
    if app.show_help {
        render_help_overlay(f, app, f.area());
    }
    if app.show_query_help {
        render_query_help_overlay(f, f.area());
    }
    if app.input_mode != InputMode::Normal {
        render_tag_prompt(f, app, f.area());
    }
}

// ── Search pane (with syntax highlighting) ────────────────────────────

fn render_search_pane(f: &mut Frame, app: &App, area: Rect) {
    let focus_style = if app.focus == Focus::Search {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 🐹 Search ")
        .style(focus_style);
    let inner = block.inner(area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    // Determine base color for text
    let base_style = if app.query_error.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Yellow)
    };

    // Build highlighted spans + cursor
    let mut input_spans = highlight_query(&app.search_input, base_style);

    if app.focus == Focus::Search {
        // Blinking block cursor at the end
        input_spans.push(Span::styled(
            "█".to_string(),
            Style::default().fg(Color::White),
        ));
    } else {
        // Invisible placeholder to keep layout stable
        input_spans.push(Span::styled(" ".to_string(), Style::default()));
    }

    let input_para = Paragraph::new(Line::from(input_spans));
    f.render_widget(input_para, rows[0]);

    let info = if let Some(ref err) = app.query_error {
        Span::styled(err, Style::default().fg(Color::Red))
    } else if let Some(ref parsed) = app.parsed_query {
        Span::styled(parsed, Style::default().fg(Color::DarkGray))
    } else {
        Span::styled("", Style::default())
    };
    let info_para = Paragraph::new(Line::from(info));
    f.render_widget(info_para, rows[1]);
    f.render_widget(block, area);
}

// ── Filter pane (scrollable) ──────────────────────────────────────────

fn render_filter_pane(f: &mut Frame, app: &App, area: Rect) {
    let focus_style = if app.focus == Focus::Filter {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Filters ")
        .style(focus_style);
    let inner = block.inner(area);

    let visible_height = inner.height as usize;
    let start = app.filter_offset;
    let end = (start + visible_height).min(app.filter_items.len());
    let visible_items = &app.filter_items[start..end];

    let mut lines: Vec<Line> = Vec::new();
    for (i, item) in visible_items.iter().enumerate() {
        let idx = start + i;
        let is_selected = idx == app.filter_selected;
        let is_active = match &item.tag_value {
            Some(tag) => app.active_filter.as_deref() == Some(tag.as_str()),
            None => false,
        };

        let prefix = if item.tag_value.is_some() {
            if is_active {
                "◉"
            } else {
                "○"
            }
        } else {
            ""
        };

        let indent = "  ".repeat(item.depth);
        let text = format!("{}{} {}", indent, prefix, item.label);

        let mut style = if is_active {
            Style::default().fg(Color::Green)
        } else if is_selected && app.focus == Focus::Filter {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan)
        } else {
            Style::default()
        };
        if item.tag_value.is_none() {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::styled(text, style));
    }

    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(para, area);
}

// ── Message list pane (Polished Spans) ────────────────────────────────

fn render_list_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let focus_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let list_height = area.height.saturating_sub(2) as usize;
    let max_offset = app.results.len().saturating_sub(list_height).max(0);

    // NOTE: Mutating state in render is generally a sin, but doing this
    // specifically for terminal resize events prevents a visual crash
    // where the list scrolls into the void. The hamster forgives itself.
    if app.list_offset > max_offset {
        app.list_offset = max_offset;
    }

    let items: Vec<ListItem> = app
        .results
        .iter()
        .skip(app.list_offset)
        .take(list_height)
        .enumerate()
        .map(|(i, row)| {
            let real_idx = app.list_offset + i;
            let selected = real_idx == app.selected;
            let date_str = row.date_display();

            // Distinct styles for distinct columns
            let indicator_style = if row.is_unread() {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let date_style = Style::default().fg(Color::DarkGray);

            let text_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if row.is_unread() {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };

            // Assemble using Spans for perfect alignment and zero format! bugs
            let line = Line::from(vec![
                Span::styled(if row.is_unread() { "● " } else { "  " }, indicator_style),
                Span::styled(format!("{} ", date_str), date_style),
                Span::styled(truncate(&row.from, 20), text_style),
                Span::styled("  ", Style::default()),
                Span::styled(row.subject.clone(), text_style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let count = app.results.len();
    let total_hint = if count >= MAX_SEARCH_RESULTS {
        format!("(showing top {})", MAX_SEARCH_RESULTS)
    } else {
        format!("{} total", count)
    };
    let list_title = format!(" Messages {}/{} {} ", app.selected + 1, count, total_hint);
    let list_block = Block::default()
        .borders(Borders::ALL)
        .title(list_title)
        .style(focus_style);
    let list = List::new(items).block(list_block);
    f.render_widget(list, area);
}

// ── Preview pane ──────────────────────────────────────────────────────

fn render_preview_pane(f: &mut Frame, app: &App, area: Rect) {
    if !app.preview_visible || area.width == 0 {
        return;
    }
    let header = if let Some(row) = app.results.get(app.selected) {
        let tag_str = if row.tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", row.tags.join(" "))
        };
        format!(" {} {}", row.subject, tag_str)
    } else {
        " Preview ".to_string()
    };
    let body = app.preview.as_deref().unwrap_or("(no message selected)");
    let preview_block = Block::default()
        .borders(Borders::ALL)
        .title(header)
        .title_style(Style::default().fg(Color::Cyan));
    let preview_para = Paragraph::new(Text::from(body))
        .block(preview_block)
        .wrap(Wrap { trim: false })
        .scroll((app.preview_offset, 0));
    f.render_widget(preview_para, area);
}

// ── Explain pane ───────────────────────────────────────────────────────

fn render_explain_pane(f: &mut Frame, app: &App, area: Rect) {
    let explain = app.explain_text.as_deref().unwrap_or("");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 🧠 Explain ")
        .title_style(Style::default().fg(Color::Magenta));
    let para = Paragraph::new(Text::from(explain))
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((0, 0));
    f.render_widget(para, area);
}

// ── Status bar ────────────────────────────────────────────────────────

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let msg = if let Some(ref s) = app.status_message {
        s.clone()
    } else if let Some(row) = app.results.get(app.selected) {
        if row.tags.is_empty() {
            " (no tags)".to_string()
        } else {
            format!(" tags: {}", row.tags.join("  "))
        }
    } else {
        String::new()
    };

    let style = if app.status_message.is_some() {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(msg).style(style), area);
}

// ── Footer with key hints ──────────────────────────────────────────────

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let hint = match app.focus {
        Focus::Search => {
            "🔍 type to search  Enter save  Ctrl‑h history  Ctrl‑g syntax  Tab → Filter"
        }
        Focus::Filter => {
            "🗂️  j/k navigate  Enter toggle  Esc clear  Tab → List"
        }
        Focus::List => {
            "📧 Ctrl‑j/k navigate  Ctrl‑t tag  Ctrl‑d read  Ctrl‑a archive  Ctrl‑y copy  Tab → Search"
        }
    };
    let style = Style::default().fg(Color::DarkGray);
    let para = Paragraph::new(hint).style(style);
    f.render_widget(para, area);
}

// ── Overlays ──────────────────────────────────────────────────────────

fn render_tag_prompt(f: &mut Frame, app: &App, area: Rect) {
    let w: u16 = 40;
    let h: u16 = 3;
    let x = area.width.saturating_sub(w) / 2;
    let y = area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);

    let verb = if app.input_mode == InputMode::AddTag {
        "Add tag"
    } else {
        "Remove tag"
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} (Enter confirm, Esc cancel) ", verb))
        .style(Style::default().fg(Color::Yellow));
    let para = Paragraph::new(app.tag_input.as_str()).block(block);

    f.render_widget(Clear, popup);
    f.render_widget(para, popup);
}

fn render_help_overlay(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = match app.focus {
        Focus::Search => vec![
            Line::from(vec![Span::styled(
                " Search Pane Help ",
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from("  • Type to search   • Enter saves query"),
            Line::from("  • Ctrl‑h history   • Ctrl‑g syntax"),
            Line::from("  • Esc clear field  • Tab → Filter"),
        ],
        Focus::Filter => vec![
            Line::from(vec![Span::styled(
                " Filter Pane Help ",
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from("  • j/k  move up/down"),
            Line::from("  • Enter  apply/clear filter"),
            Line::from("  • Esc  clear filter"),
            Line::from("  • Tab  → List"),
        ],
        Focus::List => vec![
            Line::from(vec![Span::styled(
                " Message List Help ",
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from("  • Ctrl‑j/k   navigate   • Ctrl‑f/b   page"),
            Line::from("  • Ctrl‑t/r   add/remove tag"),
            Line::from("  • Ctrl‑d     toggle read/unread"),
            Line::from("  • Ctrl‑a     archive    • Ctrl‑y     yank id"),
            Line::from("  • Ctrl‑o     open pager • Ctrl‑e     toggle preview"),
            Line::from("  • Ctrl‑x     explain    • Tab        → Search"),
        ],
    };

    let w: u16 = 52;
    let h: u16 = lines.len() as u16 + 2;
    let x = area.width.saturating_sub(w) / 2;
    let y = area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w.min(area.width), h.min(area.height));

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::White));
    let para = Paragraph::new(Text::from(lines)).block(block);
    f.render_widget(Clear, popup);
    f.render_widget(para, popup);
}

fn render_query_help_overlay(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(vec![Span::styled(
            " Query Syntax ",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from("  Simple terms:        hamster"),
        Line::from("  Field search:       from:boss@example.com"),
        Line::from("  Combine:            from:boss AND subject:wheels"),
        Line::from("  Tag filter:         tags:unread"),
        Line::from("  Phrase:             \"meeting notes\""),
        Line::from("  Wildcard:           ham*"),
        Line::from("  Negation:           -spam"),
        Line::from(""),
        Line::from(" Press Ctrl‑g or Esc to close"),
    ];

    let w: u16 = 50;
    let h: u16 = lines.len() as u16 + 2;
    let x = area.width.saturating_sub(w) / 2;
    let y = area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w.min(area.width), h.min(area.height));

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));
    let para = Paragraph::new(Text::from(lines)).block(block);
    f.render_widget(Clear, popup);
    f.render_widget(para, popup);
}

// ── Utilities ──────────────────────────────────────────────────────────

/// A pure-Span syntax highlighter for the search bar.
/// Colors keywords (from:, tags:) magenta, and dims operators (AND, OR).
fn highlight_query(input: &str, base_style: Style) -> Vec<Span<'static>> {
    if input.is_empty() {
        return vec![Span::styled(" ".to_string(), base_style)];
    }

    let keyword_style = base_style.fg(Color::Magenta).add_modifier(Modifier::BOLD);
    let op_style = base_style.fg(Color::DarkGray);
    let mut spans = Vec::new();

    for word in input.split_whitespace() {
        let upper = word.to_uppercase();

        if upper == "AND" || upper == "OR" || upper == "NOT" {
            spans.push(Span::styled(format!("{} ", word), op_style));
        } else if let Some(colon_pos) = word.find(':') {
            // Highlight the keyword part (e.g. "from:")
            spans.push(Span::styled(
                word[..colon_pos + 1].to_string(),
                keyword_style,
            ));
            // Keep the value part normal (e.g. "boss")
            let remainder = &word[colon_pos + 1..];
            if !remainder.is_empty() {
                spans.push(Span::styled(remainder.to_string(), base_style));
            }
            spans.push(Span::styled(" ".to_string(), base_style));
        } else {
            spans.push(Span::styled(format!("{} ", word), base_style));
        }
    }
    spans
}

/// Truncates a string to a maximum character width, appending an ellipsis.
/// Uses pure char boundaries to prevent panics on multibyte unicode.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", end)
    }
}
