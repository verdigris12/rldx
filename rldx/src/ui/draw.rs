use anyhow::Result;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::db::ContactListEntry;

use super::app::{App, PaneField, PaneFocus};
use super::panes::DetailTab;

pub fn render<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> Result<()> {
    terminal.draw(|frame| draw_frame(frame, app))?;
    Ok(())
}

fn draw_frame(frame: &mut Frame<'_>, app: &App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(size);

    draw_header(frame, layout[0], app);
    draw_body(frame, layout[1], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut spans: Vec<Span> = Vec::new();
    if let Some(contact) = &app.current_contact {
        spans.push(Span::raw(format!(
            "VDIR://{}",
            contact.path.to_string_lossy()
        )));

        if !app.languages.is_empty() {
            spans.push(Span::raw("   "));
            let mut first = true;
            for lang in &app.languages {
                if !first {
                    spans.push(Span::raw(" | "));
                }
                first = false;

                let active = contact
                    .lang_pref
                    .as_ref()
                    .map(|pref| pref.eq_ignore_ascii_case(lang))
                    .unwrap_or(false);
                let style = if active {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                spans.push(Span::styled(lang.to_uppercase(), style));
            }
        }
    } else {
        spans.push(Span::raw("No contacts indexed"));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, area);
}

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.show_search {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(32), Constraint::Min(0)])
            .split(area);
        draw_search(frame, chunks[0], app);
        draw_content(frame, chunks[1], app);
    } else {
        draw_content(frame, area, app);
    }
}

fn draw_content(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let upper = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(vertical[0]);

    draw_main_card(frame, upper[0], app);
    draw_image(frame, upper[1], app);
    draw_tabs(frame, vertical[1], app);
}

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let title = search_title(app);
    let active = matches!(app.focused_pane, PaneFocus::Search);
    let border_style = active_border_style(active);

    let items: Vec<ListItem> = if app.contacts.is_empty() {
        vec![ListItem::new(Line::from("No contacts"))]
    } else {
        app.contacts.iter().map(build_search_item).collect()
    };

    let mut state = ListState::default();
    if !app.contacts.is_empty() {
        state.select(Some(app.selected));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_main_card(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let active = matches!(app.focused_pane, PaneFocus::Card);
    let mut lines: Vec<Line> = Vec::new();

    if let Some(contact) = &app.current_contact {
        lines.push(Line::from(Span::styled(
            contact.display_fn.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));

        if !app.card_fields.is_empty() {
            lines.push(Line::from(""));
            for (idx, field) in app.card_fields.iter().enumerate() {
                let highlight = active && idx == app.card_field_index;
                lines.push(field_line(field, highlight));
            }
        }

        if let Some(status) = &app.status {
            lines.push(Line::from(status.clone()));
        }
    } else {
        lines.push(Line::from("No contact selected"));
    }

    let title = "1";
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_border_style(active))
        .title(title);

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_image(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = if let Some(contact) = &app.current_contact {
        if contact.has_photo {
            "Image rendering not implemented"
        } else {
            "NO IMAGE AVAILABLE"
        }
    } else {
        ""
    };

    let block = Block::default().borders(Borders::ALL).title("Image");
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let mut tab_spans: Vec<Span> = Vec::new();
    for (idx, tab) in DetailTab::ALL.iter().enumerate() {
        if idx > 0 {
            tab_spans.push(Span::raw(" | "));
        }
        let text = format!("{}: {}", tab.digit(), tab.title().to_uppercase());
        let style = if *tab == app.tab {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        tab_spans.push(Span::styled(text, style));
    }

    let tab_header =
        Paragraph::new(Line::from(tab_spans)).block(Block::default().borders(Borders::ALL));
    frame.render_widget(tab_header, chunks[0]);

    let tab_index = app.tab.index();
    let fields = &app.tab_fields[tab_index];
    let focused = matches!(app.focused_pane, PaneFocus::Detail(current) if current == app.tab);

    let mut lines: Vec<Line> = Vec::new();
    if fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        for (idx, field) in fields.iter().enumerate() {
            let highlight = focused && idx == app.tab_field_indices[tab_index];
            lines.push(field_line(field, highlight));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(active_border_style(focused));
    frame.render_widget(Paragraph::new(lines).block(block), chunks[1]);
}

fn field_line(field: &PaneField, highlight: bool) -> Line<'static> {
    let (label_style, value_style) = line_styles(highlight);
    let label = format!("{}: ", field.label);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::styled(field.value.clone(), value_style),
    ])
}

fn line_styles(highlight: bool) -> (Style, Style) {
    if highlight {
        let style = Style::default()
            .bg(Color::Yellow)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD);
        (style, style)
    } else {
        (
            Style::default().add_modifier(Modifier::BOLD),
            Style::default(),
        )
    }
}

fn active_border_style(active: bool) -> Style {
    if active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn build_search_item(entry: &ContactListEntry) -> ListItem<'static> {
    let mut lines = Vec::new();
    let title = format!(
        "{} {}",
        contact_icon(entry),
        entry.display_fn.to_uppercase()
    );
    lines.push(Line::from(title));

    if let Some(email) = &entry.primary_email {
        lines.push(Line::from(format!("  {}", email)));
    } else if let Some(org) = &entry.primary_org {
        lines.push(Line::from(format!("  {}", org)));
    }

    ListItem::new(lines)
}

fn contact_icon(entry: &ContactListEntry) -> &'static str {
    if let Some(kind) = entry.kind.as_deref() {
        if kind.eq_ignore_ascii_case("org") || kind.eq_ignore_ascii_case("organization") {
            "🏢"
        } else if kind.eq_ignore_ascii_case("group") {
            "👥"
        } else {
            "🧑"
        }
    } else if entry.primary_org.is_some() {
        "🏢"
    } else {
        "🧑"
    }
}

fn search_title(app: &App) -> String {
    let trimmed = app.query.trim();
    if trimmed.is_empty() {
        "SEARCH".to_string()
    } else {
        format!("SEARCH: {}", trimmed.to_uppercase())
    }
}
