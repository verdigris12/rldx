use std::env;

use anyhow::Result;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::config::RgbColor;

use super::app::{App, PaneField, PaneFocus, PhotoData, SearchRow};
use super::panes::DetailTab;

pub fn render<B: Backend>(terminal: &mut Terminal<B>, app: &App) -> Result<()> {
    terminal.draw(|frame| draw_frame(frame, app))?;
    Ok(())
}

fn draw_frame(frame: &mut Frame<'_>, app: &App) {
    let size = frame.size();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(size);

    draw_header(frame, layout[0], app);
    draw_body(frame, layout[1], app);
    draw_footer(frame, layout[2], app);

    if app.editor.active {
        draw_editor_overlay(frame, size, app);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let header_style = header_text_style(app);
    let mut spans: Vec<Span> = Vec::new();

    if let Some(contact) = &app.current_contact {
        let display_path = app.contact_path_display(&contact.path);
        spans.push(Span::styled(
            format!("VDIR://{}", display_path),
            header_style,
        ));

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
                    selection_style(app)
                } else {
                    header_style
                };
                spans.push(Span::styled(lang.to_uppercase(), style));
            }
        }
    } else {
        spans.push(Span::styled("No contacts indexed", header_style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let image_height = app.image_pane_height().min(area.height);
    let upper_height = image_height.min(area.height);

    let main_height = upper_height.min(area.height);
    let lower_start = area.y + upper_height;

    let top_rect = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: upper_height,
    };

    let lower_rect = Rect {
        x: area.x,
        y: lower_start,
        width: area.width,
        height: area.height.saturating_sub(upper_height),
    };

    let image_width = app.image_pane_width();
    let upper = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(image_width)])
        .split(top_rect);

    let mut main_area = upper[0];
    let mut image_area = upper[1];
    let original_main_area = main_area;
    let original_image_area = image_area;

    main_area.height = main_height.min(main_area.height);
    image_area.height = main_height.min(image_area.height);

    draw_main_card(frame, main_area, app);
    if original_main_area.height > main_area.height {
        let clear_rect = Rect {
            x: original_main_area.x,
            y: original_main_area.y + main_area.height,
            width: original_main_area.width,
            height: original_main_area.height - main_area.height,
        };
        frame.render_widget(Clear, clear_rect);
    }

    draw_image(frame, image_area, app);
    if original_image_area.height > image_area.height {
        let clear_rect = Rect {
            x: original_image_area.x,
            y: original_image_area.y + image_area.height,
            width: original_image_area.width,
            height: original_image_area.height - image_area.height,
        };
        frame.render_widget(Clear, clear_rect);
    }

    if lower_rect.height > 0 {
        draw_tabs(frame, lower_rect, app);
    }
}

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let active = matches!(app.focused_pane, PaneFocus::Search);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app, active));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let header_line = Line::from(Span::styled(search_title(app), header_text_style(app)));
    render_header_with_double_line(frame, layout[0], header_line, app);
    draw_search_list(frame, layout[1], app);
}

fn draw_search_list(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem> = if app.search_rows.is_empty() {
        vec![ListItem::new(Line::from("No contacts"))]
    } else {
        app.search_rows
            .iter()
            .map(|row| build_search_item(row, app))
            .collect()
    };

    let mut state = ListState::default();
    if let Some(selected) = app.selected_row {
        state.select(Some(selected));
    }

    let list = List::new(items)
        .highlight_style(selection_style(app))
        .highlight_symbol(" ")
        .repeat_highlight_symbol(false);

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_main_card(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let active = matches!(app.focused_pane, PaneFocus::Card);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app, active));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let header_line = if let Some(contact) = &app.current_contact {
        Line::from(Span::styled(
            contact.display_fn.to_uppercase(),
            header_text_style(app),
        ))
    } else {
        Line::from(Span::styled(
            "NO CONTACT SELECTED".to_string(),
            header_text_style(app),
        ))
    };
    render_header_with_double_line(frame, layout[0], header_line, app);

    let mut lines: Vec<Line> = Vec::new();
    if app.current_contact.is_none() {
        lines.push(Line::from("Select a contact"));
    } else if app.card_fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        for (idx, field) in app.card_fields.iter().enumerate() {
            let highlight = active && idx == app.card_field_index;
            lines.push(field_line(app, field, highlight));
        }
    }

    frame.render_widget(Paragraph::new(lines), layout[1]);
}

fn draw_image(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app, false))
        .title("Image");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    frame.render_widget(Clear, inner);
    frame.render_widget(KittyClear, inner);

    if let Some(photo) = app.embedded_photo() {
        if supports_kitty_graphics() {
            frame.render_widget(KittyImage::new(photo), inner);
        } else {
            render_centered_words(frame, inner, "KITTY GRAPHICS NOT AVAILABLE");
        }
        return;
    }

    let message = if let Some(error) = app.photo_error() {
        error.to_string()
    } else if let Some(contact) = &app.current_contact {
        if contact.has_photo {
            "PHOTO NOT EMBEDDED".to_string()
        } else {
            "NO IMAGE AVAILABLE".to_string()
        }
    } else {
        String::new()
    };

    if !message.is_empty() {
        render_centered_words(frame, inner, &message);
    }
}

fn draw_tabs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let focused = matches!(app.focused_pane, PaneFocus::Detail(current) if current == app.tab);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app, focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    render_header_with_double_line(frame, layout[0], build_tab_header(app), app);

    let tab_index = app.tab.index();
    let fields = &app.tab_fields[tab_index];

    let mut lines: Vec<Line> = Vec::new();
    if fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        for (idx, field) in fields.iter().enumerate() {
            let highlight = focused && idx == app.tab_field_indices[tab_index];
            lines.push(field_line(app, field, highlight));
        }
    }

    frame.render_widget(Paragraph::new(lines), layout[1]);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let message = app.status.as_deref().unwrap_or("READY");
    let colors = app.ui_colors();
    let style = Style::default()
        .fg(color(colors.status_fg))
        .bg(color(colors.status_bg));

    let background = Block::default().style(Style::default().bg(color(colors.status_bg)));
    frame.render_widget(background, area);

    frame.render_widget(Paragraph::new(message.to_string()).style(style), area);
}

fn build_tab_header(app: &App) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (idx, tab) in DetailTab::ALL.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled(" | ".to_string(), header_text_style(app)));
        }
        let text = format!("{}: {}", tab.digit(), tab.title().to_uppercase());
        let style = if *tab == app.tab {
            selection_style(app)
        } else {
            header_text_style(app)
        };
        spans.push(Span::styled(text, style));
    }
    Line::from(spans)
}

fn field_line(app: &App, field: &PaneField, highlight: bool) -> Line<'static> {
    let editing = app.editor.active
        && field
            .source()
            .zip(app.editor.target())
            .map(|(lhs, rhs)| lhs == *rhs)
            .unwrap_or(false);
    let (label_style, value_style) = line_styles(app, highlight || editing);
    let label = format!("{}: ", field.label);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::styled(field.value.clone(), value_style),
    ])
}

fn line_styles(app: &App, highlight: bool) -> (Style, Style) {
    if highlight {
        let style = selection_style(app);
        (style, style)
    } else {
        (header_text_style(app), Style::default())
    }
}

fn build_search_item(row: &SearchRow, app: &App) -> ListItem<'static> {
    let indent = "  ".repeat(row.depth as usize);
    let mut text = String::with_capacity(indent.len() + row.text.len());
    text.push_str(&indent);
    text.push_str(&row.text);

    let mut item = ListItem::new(Line::from(text));
    if !row.selectable() {
        item = item.style(header_text_style(app));
    }
    item
}

fn search_title(app: &App) -> String {
    let trimmed = app.query.trim();
    if trimmed.is_empty() {
        "SEARCH".to_string()
    } else {
        format!("SEARCH: {}", trimmed.to_uppercase())
    }
}

fn selection_style(app: &App) -> Style {
    let colors = app.ui_colors();
    Style::default()
        .fg(color(colors.selection_fg))
        .bg(color(colors.selection_bg))
}

fn border_style(app: &App, _active: bool) -> Style {
    let colors = app.ui_colors();
    Style::default().fg(color(colors.border))
}

fn header_text_style(app: &App) -> Style {
    let colors = app.ui_colors();
    Style::default().fg(color(colors.separator))
}

fn separator_style(app: &App) -> Style {
    let colors = app.ui_colors();
    Style::default().fg(color(colors.separator))
}

fn render_centered_words(frame: &mut Frame<'_>, area: Rect, text: &str) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = text
        .split_whitespace()
        .map(|word| Line::from(word.to_string()))
        .collect();

    if lines.is_empty() {
        return;
    }

    if lines.len() as u16 > area.height {
        lines.truncate(area.height as usize);
    }

    let height = lines.len() as u16;
    let start_y = area.y + (area.height.saturating_sub(height)) / 2;
    let target = Rect {
        x: area.x,
        y: start_y,
        width: area.width,
        height,
    };

    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), target);
}

struct KittyImage<'a> {
    photo: &'a PhotoData,
}

impl<'a> KittyImage<'a> {
    fn new(photo: &'a PhotoData) -> Self {
        Self { photo }
    }
}

impl<'a> Widget for KittyImage<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let (cols, rows) = fit_image_to_area(self.photo, area.width, area.height);
        let origin_x = area.x + (area.width.saturating_sub(cols)) / 2;
        let origin_y = area.y + (area.height.saturating_sub(rows)) / 2;
        let sequence = kitty_sequence(self.photo, cols, rows);
        buf.get_mut(origin_x, origin_y).set_symbol(&sequence);
    }
}

fn kitty_sequence(photo: &PhotoData, width: u16, height: u16) -> String {
    format!(
        "\x1b_Ga=T,f=100,q=2,s={width},v={height};{}\x1b\\",
        photo.png_base64()
    )
}

fn supports_kitty_graphics() -> bool {
    if env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }
    if let Ok(term) = env::var("TERM") {
        if term.to_ascii_lowercase().contains("kitty") {
            return true;
        }
    }
    false
}

fn fit_image_to_area(photo: &PhotoData, area_width: u16, area_height: u16) -> (u16, u16) {
    let img_w = photo.width().max(1);
    let img_h = photo.height().max(1);
    let max_w = area_width.max(1);
    let max_h = area_height.max(1);

    let mut cells_w = max_w;
    let mut cells_h = ((cells_w as u32 * img_h + img_w - 1) / img_w).clamp(1, max_h as u32) as u16;

    if cells_h > max_h {
        cells_h = max_h;
        cells_w = ((cells_h as u32 * img_w + img_h - 1) / img_h).clamp(1, max_w as u32) as u16;
        if cells_w == 0 {
            cells_w = 1;
        }
    }

    cells_w = cells_w.max(1);
    cells_h = cells_h.max(1);
    (cells_w, cells_h)
}

struct KittyClear;

impl Widget for KittyClear {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        buf.get_mut(area.x, area.y).set_symbol("\x1b_Ga=d\x1b\\");
    }
}

fn draw_editor_overlay(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let width = area.width.min(60);
    let height = area.height.min(7);
    let popup = centered_rect(width, height, area);

    frame.render_widget(Clear, popup);

    let block = Block::default().borders(Borders::ALL).title("EDIT FIELD");
    frame.render_widget(block.clone(), popup);

    let inner = block.inner(popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let label = app.editor.label().unwrap_or("FIELD");
    let mut value = app.editor.value.clone();
    value.push('_');

    let lines = vec![
        Line::from(label.to_string()),
        Line::from(value),
        Line::from("Enter=Save   Esc=Cancel"),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let clamped_width = width.min(area.width);
    let clamped_height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(clamped_width)) / 2;
    let y = area.y + (area.height.saturating_sub(clamped_height)) / 2;
    Rect {
        x,
        y,
        width: clamped_width,
        height: clamped_height,
    }
}

fn render_header_with_double_line(
    frame: &mut Frame<'_>,
    area: Rect,
    content: Line<'static>,
    app: &App,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if area.height == 1 {
        frame.render_widget(Paragraph::new(content), area);
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    frame.render_widget(Paragraph::new(content), layout[0]);

    let separator = "═".repeat(layout[1].width as usize);
    let separator_line = Line::from(Span::styled(separator, separator_style(app)));
    frame.render_widget(Paragraph::new(separator_line), layout[1]);
}

fn color(rgb: RgbColor) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}
