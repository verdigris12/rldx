use anyhow::Result;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};
use ratatui::{Frame, Terminal};
use ratatui_image::{Resize, StatefulImage};
// Use Popup from tui-widgets to render modals
use tui_widgets::popup::Popup;

use crate::config::RgbColor;

use super::app::{App, MultiValueField, PaneField, PaneFocus, SearchFocus, SearchRow};
use super::panes::DetailTab;

const MULTIVALUE_HELP: &str =
    "TAB/Down: next  Backspace/Up: prev  Space: copy & close  Enter: set default  E: edit  Q/Esc: close";
const SEARCH_HELP_INPUT: &str =
    "Type to filter  Esc: focus results  Enter: open";

pub fn render<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    terminal.draw(|frame| draw_frame(frame, app))?;
    Ok(())
}

fn draw_frame(frame: &mut Frame<'_>, app: &mut App) {
    let size = frame.area();
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
    draw_multivalue_modal(frame, size, app);
    draw_confirm_modal(frame, size, app);
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

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
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

fn draw_content(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
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
    let active = matches!(app.focused_pane, PaneFocus::Search)
        && matches!(app.search_focus, SearchFocus::Input);
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

    draw_search_header(frame, layout[0], app, active);
    draw_search_list(frame, layout[1], app);
}

fn draw_search_header(frame: &mut Frame<'_>, area: Rect, app: &App, active: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let label = "SEARCH: ";
    let label_style = header_text_style(app);
    let value_style = if active {
        selection_style(app)
    } else {
        Style::default()
    };
    let value = app.search_input.value();
    let line = Line::from(vec![
        Span::styled(label, label_style),
        Span::styled(value.to_string(), value_style),
    ]);

    let cursor_column = if active {
        let label_width = Span::raw(label).width();
        Some(label_width + app.search_input.visual_cursor())
    } else {
        None
    };

    if area.height == 1 {
        frame.render_widget(Paragraph::new(line.clone()), area);
        if let Some(column) = cursor_column {
            let x = area.x.saturating_add(column as u16);
        frame.set_cursor_position((x, area.y));
        }
        return;
    }

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    frame.render_widget(Paragraph::new(line), parts[0]);

    if let Some(column) = cursor_column {
        let x = parts[0].x.saturating_add(column as u16);
        frame.set_cursor_position((x, parts[0].y));
    }

    let separator = "═".repeat(parts[1].width as usize);
    let separator_line = Line::from(Span::styled(separator, separator_style(app)));
    frame.render_widget(Paragraph::new(separator_line), parts[1]);
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
    render_header_with_double_line(frame, layout[0], header_line, app, None);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor = None;
    if app.current_contact.is_none() {
        lines.push(Line::from("Select a contact"));
    } else if app.card_fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        for (idx, field) in app.card_fields.iter().enumerate() {
            let highlight = active && idx == app.card_field_index;
            let line_index = lines.len();
            let (line, cursor_info) = field_line(app, field, highlight);
            if cursor.is_none() {
                if let Some(column) = cursor_info {
                    cursor = Some((line_index, column));
                }
            }
            lines.push(line);
        }
    }

    frame.render_widget(Paragraph::new(lines), layout[1]);

    if let Some((line_idx, column)) = cursor {
        let x = layout[1].x.saturating_add(column as u16);
        let y = layout[1].y.saturating_add(line_idx as u16);
        frame.set_cursor_position((x, y));
    }
}

fn draw_multivalue_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.multivalue_modal().is_none() {
        return;
    }

    let mut width = area.width.saturating_mul(2).saturating_div(3);
    let min_width = area.width.min(30);
    if width < min_width {
        width = min_width;
    }
    if width > area.width {
        width = area.width;
    }

    // Popup content size is derived from computed width

    let header =
        Row::new(vec![Cell::from("VALUE"), Cell::from("TYPE")]).style(header_text_style(app));

    // Prepare table data and context before mutable borrow of popup state
    let (field_title, selected_index, items_len, rows) = {
        let m = app.multivalue_modal().unwrap();
        let selected_index = m.selected();
        let field = m.field();
        let items = m.items();

        let editing_for_selected = if app.editor.active {
            if let Some(target) = app.editor.target() {
                if let Some(f) = MultiValueField::from_field_name(&target.field) {
                    f == field && (target.seq as usize) == selected_index
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        let rows: Vec<Row> = items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let value_cell = if editing_for_selected && idx == selected_index {
                    Cell::from(app.editor.value().to_string())
                } else {
                    Cell::from(item.value.clone())
                };
                Row::new(vec![value_cell, Cell::from(item.type_label.clone())])
            })
            .collect();

        (field.title(), selected_index, items.len(), rows)
    };

    let widths = [Constraint::Percentage(70), Constraint::Percentage(30)];
    let table = Table::new(rows, widths)
        .header(header)
        .highlight_style(selection_style(app));

    // Build popup body placeholder sized to content area (width/height exclude borders)
    let content_width = width.saturating_sub(2) as usize;
    let content_height = items_len.saturating_add(1); // header + items
    let body_lines: Vec<Line> = (0..content_height)
        .map(|_| Line::from(" ".repeat(content_width)))
        .collect();
    let body_text = ratatui::text::Text::from(body_lines);

    let title_line = Line::from(Span::styled(field_title, header_text_style(app)));
    let popup = Popup::new(body_text)
        .title(title_line)
        .border_style(border_style(app, true));

    // Render popup using state so we can retrieve its area
    frame.render_stateful_widget_ref(popup, area, &mut app.modal_popup);

    if let Some(area) = app.modal_popup.area() {
        // Compute inner area (content area) based on borders
        let inner = Block::default().borders(Borders::ALL).inner(*area);
        if inner.width > 0 && inner.height > 0 {
            let mut state = TableState::default();
            state.select(Some(selected_index));
            frame.render_stateful_widget(table, inner, &mut state);
            // If editing inline, place cursor in the value cell of the selected row
            if app.editor.active {
                if let Some(target) = app.editor.target() {
                    if let Some(field) = MultiValueField::from_field_name(&target.field) {
                        if field_title.eq(field.title()) {
                            let cursor_x = inner.x.saturating_add(app.editor.visual_cursor() as u16);
                            let cursor_y = inner
                                .y
                                .saturating_add(1) // header row offset
                                .saturating_add(selected_index as u16);
                            frame.set_cursor_position((cursor_x, cursor_y));
                        }
                    }
                }
            }
        }
    }
}

fn draw_confirm_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let Some(modal) = app.confirm_modal.as_ref() else { return; };

    let mut width = area.width.saturating_mul(2).saturating_div(3);
    let min_width = area.width.min(30);
    if width < min_width { width = min_width; }
    if width > area.width { width = area.width; }

    let content_width = width.saturating_sub(2) as usize;
    let lines = vec![
        Line::from(modal.message.clone()),
        Line::from("".to_string()),
        Line::from(CONFIRM_HELP.to_string()),
    ];
    let body_text = ratatui::text::Text::from(
        lines
            .into_iter()
            .map(|line| {
                // Ensure we allocate at least content width to avoid tiny popup
                let l = line;
                if l.width() < content_width { /* leave as is; Popup sizes itself */ }
                l
            })
            .collect::<Vec<Line>>(),
    );

    let title_line = Line::from(Span::styled(modal.title.clone(), header_text_style(app)));
    let popup = Popup::new(body_text)
        .title(title_line)
        .border_style(border_style(app, true));

    frame.render_stateful_widget_ref(popup, area, &mut app.modal_popup);
}

fn draw_image(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
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

    let render_area = image_render_area(app, inner);

    if let Some(state) = app.profile_image_state() {
        let widget = StatefulImage::new(None).resize(Resize::Fit);
        frame.render_stateful_widget(widget, render_area, state);
        return;
    }

    if let Some(error) = app.photo_error() {
        render_centered_words(frame, inner, error);
        return;
    }

    if let Some(contact) = &app.current_contact {
        let message = if contact.has_photo {
            "PHOTO NOT EMBEDDED"
        } else {
            "NO IMAGE AVAILABLE"
        };
        render_centered_words(frame, inner, message);
    }
}

fn image_render_area(app: &App, area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return area;
    }

    let Some(photo) = app.photo_data.as_ref() else {
        return area;
    };

    let (font_w, font_h) = app.image_font_size();
    if font_w == 0 || font_h == 0 {
        return area;
    }

    let desired_width = div_ceil_u32(photo.image().width(), u32::from(font_w));
    let desired_height = div_ceil_u32(photo.image().height(), u32::from(font_h));

    if desired_width == 0 || desired_height == 0 {
        return area;
    }

    let desired_width = desired_width.min(u32::from(u16::MAX)) as u16;
    let desired_height = desired_height.min(u32::from(u16::MAX)) as u16;

    let target_width = desired_width.min(area.width);
    let target_height = desired_height.min(area.height);

    let wratio = target_width as f64 / desired_width as f64;
    let hratio = target_height as f64 / desired_height as f64;
    let mut ratio = wratio.min(hratio);
    if !ratio.is_finite() || ratio <= 0.0 {
        ratio = 1.0;
    }

    let width = (desired_width as f64 * ratio)
        .round()
        .clamp(1.0, area.width as f64) as u16;
    let height = (desired_height as f64 * ratio)
        .round()
        .clamp(1.0, area.height as f64) as u16;

    let offset_x = area.width.saturating_sub(width) / 2;
    let offset_y = area.height.saturating_sub(height) / 2;

    Rect {
        x: area.x.saturating_add(offset_x),
        y: area.y.saturating_add(offset_y),
        width: width.max(1),
        height: height.max(1),
    }
}

fn div_ceil_u32(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        return 0;
    }
    value / divisor + u32::from(value % divisor != 0)
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

    render_header_with_double_line(frame, layout[0], build_tab_header(app), app, None);

    let tab_index = app.tab.index();
    let fields = &app.tab_fields[tab_index];

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor = None;
    if fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        for (idx, field) in fields.iter().enumerate() {
            let highlight = focused && idx == app.tab_field_indices[tab_index];
            let line_index = lines.len();
            let (line, cursor_info) = field_line(app, field, highlight);
            if cursor.is_none() {
                if let Some(column) = cursor_info {
                    cursor = Some((line_index, column));
                }
            }
            lines.push(line);
        }
    }

    frame.render_widget(Paragraph::new(lines), layout[1]);

    if let Some((line_idx, column)) = cursor {
        let x = layout[1].x.saturating_add(column as u16);
        let y = layout[1].y.saturating_add(line_idx as u16);
        frame.set_cursor_position((x, y));
    }
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let message: String = if app.multivalue_modal().is_some() {
        MULTIVALUE_HELP.to_string()
    } else if app.confirm_modal.is_some() {
        CONFIRM_HELP.to_string()
    } else if app.show_search {
        match app.search_focus {
            SearchFocus::Input => SEARCH_HELP_INPUT.to_string(),
            SearchFocus::Results => {
                if app.show_marked_only {
                    "Space: unmark  M: show search results  /: focus search  Enter: open & close search  Esc: close".to_string()
                } else {
                    "Space: mark  M: show marked only  /: focus search  Enter: open & close search  Esc: close".to_string()
                }
            }
        }
    } else {
        app.status.clone().unwrap_or_else(|| "READY".to_string())
    };
    let colors = app.ui_colors();
    let style = Style::default()
        .fg(color(colors.status_fg))
        .bg(color(colors.status_bg));

    let background = Block::default().style(Style::default().bg(color(colors.status_bg)));
    frame.render_widget(background, area);

    frame.render_widget(Paragraph::new(message).style(style), area);
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

fn field_line(app: &App, field: &PaneField, highlight: bool) -> (Line<'static>, Option<usize>) {
    let editing = app.editor.active
        && field
            .source()
            .zip(app.editor.target())
            .map(|(lhs, rhs)| lhs == *rhs)
            .unwrap_or(false);
    let (label_style, value_style) = line_styles(app, highlight || editing);
    let label = format!("{}: ", field.label);
    let mut spans = vec![Span::styled(label.clone(), label_style)];
    let mut cursor = None;

    if editing {
        let value = app.editor.value().to_string();
        let label_width = Span::raw(label).width();
        let cursor_column = label_width + app.editor.visual_cursor();
        cursor = Some(cursor_column);
        spans.push(Span::styled(value, value_style));
    } else {
        spans.push(Span::styled(field.value.clone(), value_style));
    }

    (Line::from(spans), cursor)
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

fn render_header_with_double_line(
    frame: &mut Frame<'_>,
    area: Rect,
    content: Line<'static>,
    app: &App,
    style: Option<Style>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if area.height == 1 {
        let paragraph = if let Some(style) = style {
            Paragraph::new(content).style(style)
        } else {
            Paragraph::new(content)
        };
        frame.render_widget(paragraph, area);
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    let paragraph = if let Some(style) = style {
        Paragraph::new(content).style(style)
    } else {
        Paragraph::new(content)
    };

    frame.render_widget(paragraph, layout[0]);

    let separator = "═".repeat(layout[1].width as usize);
    let separator_line = Line::from(Span::styled(separator, separator_style(app)));
    frame.render_widget(Paragraph::new(separator_line), layout[1]);
}

fn color(rgb: RgbColor) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}
const CONFIRM_HELP: &str = "Y/Enter: confirm  N/Esc: cancel";
