use anyhow::Result;
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use serde_json::Value;

use crate::db::PropRow;

use super::app::App;
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
    let header_text = if let Some(contact) = &app.current_contact {
        let path = contact.path.to_string_lossy();
        let mut parts = vec![format!("VDIR://{path}")];
        if !app.languages.is_empty() {
            let langs = app.languages.join(" | ");
            parts.push(format!("[{langs}]"));
        }
        parts.join("   ")
    } else {
        "No contacts indexed".to_string()
    };

    let paragraph = Paragraph::new(header_text)
        .style(Style::default())
        .block(Block::default());
    frame.render_widget(paragraph, area);
}

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(0)])
        .split(area);

    draw_search(frame, chunks[0], app);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(chunks[1]);

    let upper = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(right[0]);

    draw_main_card(frame, upper[0], app);
    draw_image(frame, upper[1], app);
    draw_tabs(frame, right[1], app);
}

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let search_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let query = if app.show_search {
        format!("/{}", app.query)
    } else {
        "/ (press / to search)".to_string()
    };
    let query_paragraph = Paragraph::new(query)
        .block(Block::default().borders(Borders::ALL).title("Search"));
    frame.render_widget(query_paragraph, search_chunks[0]);

    let items: Vec<ListItem> = app
        .contacts
        .iter()
        .map(|contact| {
            let mut lines = Vec::new();
            lines.push(Line::from(contact.display_fn.clone()));
            if let Some(email) = &contact.primary_email {
                lines.push(Line::from(email.clone()));
            } else if let Some(org) = &contact.primary_org {
                lines.push(Line::from(org.clone()));
            }
            ListItem::new(lines)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    if !app.contacts.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, search_chunks[1], &mut state);
}

fn draw_main_card(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(contact) = &app.current_contact {
        lines.push(Line::from(Span::styled(
            contact.display_fn.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if !app.aliases.is_empty() {
            lines.push(Line::from(format!("aka: {}", app.aliases.join(", "))));
        }
        let primary_email = find_first_value(&app.current_props, "EMAIL");
        let primary_phone = find_first_value(&app.current_props, "TEL");
        if let Some(email) = primary_email {
            lines.push(Line::from(format!("Email: {email}")));
        }
        if let Some(phone) = primary_phone {
            lines.push(Line::from(format!("Phone: {phone}")));
        }
        if let Some(rev) = &contact.rev {
            lines.push(Line::from(format!("REV: {rev}")));
        }
    } else {
        lines.push(Line::from("No contact selected"));
    }

    let mut block = Block::default().borders(Borders::ALL).title("Card");
    if let Some(status) = &app.status {
        block = block.title(format!("Card — {status}"));
    }
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
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
    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Image"));
    frame.render_widget(paragraph, area);
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let titles: Vec<Line> = [
        DetailTab::Work,
        DetailTab::Personal,
        DetailTab::Accounts,
        DetailTab::Metadata,
    ]
    .iter()
    .map(|tab| Line::from(tab.title()))
    .collect();

    let tabs = Tabs::new(titles)
        .select(match app.tab {
            DetailTab::Work => 0,
            DetailTab::Personal => 1,
            DetailTab::Accounts => 2,
            DetailTab::Metadata => 3,
        })
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(tabs, chunks[0]);

    let content_lines = tab_content_lines(app);
    let paragraph = Paragraph::new(content_lines)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(paragraph, chunks[1]);
}

fn tab_content_lines(app: &App) -> Vec<Line<'static>> {
    match app.tab {
        DetailTab::Work => filter_props(
            &app.current_props,
            &["ORG", "TITLE", "ROLE", "EMAIL", "TEL", "ADR"],
            Some("work"),
        ),
        DetailTab::Personal => filter_props(
            &app.current_props,
            &["BDAY", "ANNIVERSARY", "EMAIL", "TEL", "ADR"],
            Some("home"),
        ),
        DetailTab::Accounts => filter_props(&app.current_props, &["IMPP", "URL", "RELATED"], None),
        DetailTab::Metadata => app
            .current_props
            .iter()
            .map(|prop| Line::from(format!("{}: {}", prop.field, prop.value)))
            .collect(),
    }
}

fn filter_props(props: &[PropRow], fields: &[&str], type_filter: Option<&str>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for field in fields {
        for prop in props.iter().filter(|p| p.field == *field) {
            if let Some(expected) = type_filter {
                if !prop_type_matches(&prop.params, expected) {
                    continue;
                }
            }
            lines.push(Line::from(format!("{}: {}", field, prop.value)));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("No data"));
    }
    lines
}

fn prop_type_matches(params: &serde_json::Value, expected: &str) -> bool {
    if let Some(Value::Array(types)) = params.get("type") {
        for entry in types {
            if let Some(value) = entry.as_str() {
                if value.eq_ignore_ascii_case(expected) {
                    return true;
                }
            }
        }
    }
    false
}

fn find_first_value(props: &[PropRow], field: &str) -> Option<String> {
    props
        .iter()
        .find(|prop| prop.field == field)
        .map(|prop| prop.value.clone())
}