use anyhow::Result;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};
use ratatui::symbols::line::NORMAL as LINE;
use ratatui::{Frame, Terminal};
use ratatui_image::{Resize, StatefulImage};
// Use Popup from tui-widgets to render modals
use tui_widgets::popup::Popup;

use crate::config::{RgbColor, TopBarButton};

use super::app::{App, MultiValueField, PaneField, PaneFocus, SearchFocus, SearchRow};
use super::panes::DetailTab;

const MULTIVALUE_HELP: &str =
    "j/k: nav  Space: copy  Enter: default  e: edit  q/Esc: close";
const ALIAS_MODAL_HELP: &str =
    "j/k: nav  Space: copy  e: edit  a: add  x: delete  q/Esc: close";
const SEARCH_HELP_INPUT: &str =
    "Type to filter  Esc: focus results  Enter: open";
const ADD_ALIAS_HELP: &str = "Type alias  Enter: add  Esc: cancel";
const HELP_MODAL_FOOTER: &str = "j/k: scroll  Esc/q: close";

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
    draw_alias_modal(frame, size, app);
    draw_multivalue_modal(frame, size, app);
    draw_confirm_modal(frame, size, app);
    draw_help_modal(frame, size, app);
    draw_reindex_modal(frame, size, app);
    draw_share_modal(frame, size, app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    // Calculate button area width
    let buttons = app.top_bar_buttons();
    let total_buttons_width = calculate_buttons_width(buttons);

    // Split area: left for path/languages, right for buttons
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(total_buttons_width),
        ])
        .split(area);

    // Draw path/languages on left
    draw_header_left(frame, chunks[0], app);

    // Draw buttons on right
    draw_top_bar_buttons(frame, chunks[1], app);
}

fn draw_header_left(frame: &mut Frame<'_>, area: Rect, app: &App) {
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

fn calculate_buttons_width(buttons: &[TopBarButton]) -> u16 {
    if buttons.is_empty() {
        return 0;
    }

    // Find longest title
    let max_title_len = buttons
        .iter()
        .map(|b| b.action.title().len())
        .max()
        .unwrap_or(0);

    // Format: " F1: TITLE " = 1 (space) + key.len() + 2 (": ") + title + 1 (space)
    // Keys are F1-F12, so 2-3 chars. Use 3 for consistency.
    let button_width = (1 + 3 + 2 + max_title_len + 1) as u16;

    // Total: button_width * count + separators (1 space between each button)
    let num_buttons = buttons.len() as u16;
    let separators = num_buttons.saturating_sub(1);
    button_width * num_buttons + separators
}

fn draw_top_bar_buttons(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let buttons = app.top_bar_buttons();
    if buttons.is_empty() || area.width == 0 {
        return;
    }

    let max_title_len = buttons
        .iter()
        .map(|b| b.action.title().len())
        .max()
        .unwrap_or(0);

    // Each button: " F1: TITLE "
    let button_content_width = (1 + 3 + 2 + max_title_len + 1) as u16;

    let colors = app.ui_colors();
    let button_style = Style::default()
        .fg(color(colors.selection_fg))
        .bg(color(colors.selection_bg))
        .add_modifier(Modifier::BOLD);

    let mut x = area.x;
    for (idx, button) in buttons.iter().enumerate() {
        if x + button_content_width > area.x + area.width {
            break;
        }

        // Format: " F1: TITLE " with title padded to max_title_len, centered
        let text = format!(
            " {}: {:^width$} ",
            button.key,
            button.action.title(),
            width = max_title_len
        );

        let button_area = Rect {
            x,
            y: area.y,
            width: button_content_width,
            height: 1,
        };

        frame.render_widget(
            Paragraph::new(text).style(button_style).alignment(Alignment::Center),
            button_area,
        );

        x += button_content_width;

        // Add separator space (with default/background style) between buttons
        if idx < buttons.len() - 1 && x < area.x + area.width {
            let sep_area = Rect {
                x,
                y: area.y,
                width: 1,
                height: 1,
            };
            frame.render_widget(Paragraph::new(" "), sep_area);
            x += 1;
        }
    }
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

    draw_search_header(frame, layout[0], app, active, area.width);
    draw_search_list(frame, layout[1], app);
}

fn draw_search_header(frame: &mut Frame<'_>, area: Rect, app: &App, active: bool, outer_width: u16) {
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

    // Build separator with connector characters: ├───┤
    let inner_width = outer_width.saturating_sub(2) as usize;
    let separator = format!(
        "{}{}{}",
        LINE.vertical_right,
        LINE.horizontal.to_string().repeat(inner_width),
        LINE.vertical_left
    );
    let separator_line = Line::from(Span::styled(separator, separator_style(app)));
    
    // Render at the separator row, shifted left by 1 to start at the border
    let separator_area = Rect {
        x: parts[1].x.saturating_sub(1),
        y: parts[1].y,
        width: outer_width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(separator_line), separator_area);
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
    render_header_with_separator(frame, layout[0], header_line, app, None, area.width);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor = None;
    if app.current_contact.is_none() {
        lines.push(Line::from("Select a contact"));
    } else if app.card_fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        // Calculate max label width for alignment (label + colon)
        let label_width = app
            .card_fields
            .iter()
            .map(|f| f.label.len() + 1) // +1 for colon
            .max()
            .unwrap_or(0);

        for (idx, field) in app.card_fields.iter().enumerate() {
            let highlight = active && idx == app.card_field_index;
            let line_index = lines.len();
            let (line, cursor_info) = field_line(app, field, highlight, label_width);
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

    // Prepare table data and context before mutable borrow of popup state
    let (field_kind, field_title, selected_index, items_len, rows) = {
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
                if field.has_type_label() {
                    Row::new(vec![value_cell, Cell::from(item.type_label.clone())])
                } else {
                    Row::new(vec![value_cell])
                }
            })
            .collect();

        (field, field.title(), selected_index, items.len(), rows)
    };

    // Build header and table based on field type
    let table = if field_kind.has_type_label() {
        let header = Row::new(vec![Cell::from("VALUE"), Cell::from("TYPE")])
            .style(header_text_style(app));
        let widths = vec![Constraint::Percentage(70), Constraint::Percentage(30)];
        Table::new(rows, widths)
            .header(header)
            .highlight_style(selection_style(app))
    } else {
        // No header for simple lists (e.g., Alias)
        let widths = vec![Constraint::Percentage(100)];
        Table::new(rows, widths)
            .highlight_style(selection_style(app))
    };

    // Build popup body placeholder sized to content area (width/height exclude borders)
    let content_width = width.saturating_sub(2) as usize;
    // For fields with type labels, add 1 for header row; otherwise just item count
    let content_height = if items_len == 0 {
        2
    } else if field_kind.has_type_label() {
        items_len.saturating_add(1) // header + items
    } else {
        items_len // just items, no header
    };
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
            if items_len == 0 {
                // Show "No aliases" message for empty alias list
                let msg = if field_kind == MultiValueField::Alias {
                    "No aliases. Press 'a' to add."
                } else {
                    "No items"
                };
                let para = Paragraph::new(msg).style(header_text_style(app));
                frame.render_widget(para, inner);
            } else {
                let mut state = TableState::default();
                state.select(Some(selected_index));
                frame.render_stateful_widget(table, inner, &mut state);
                // If editing inline, place cursor in the value cell of the selected row
                if app.editor.active {
                    if let Some(target) = app.editor.target() {
                        if let Some(field) = MultiValueField::from_field_name(&target.field) {
                            if field_title.eq(field.title()) {
                                let cursor_x = inner.x.saturating_add(app.editor.visual_cursor() as u16);
                                // Add header row offset only for fields with type labels
                                let header_offset = if field.has_type_label() { 1 } else { 0 };
                                let cursor_y = inner
                                    .y
                                    .saturating_add(header_offset)
                                    .saturating_add(selected_index as u16);
                                frame.set_cursor_position((cursor_x, cursor_y));
                            }
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

fn draw_help_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.help_modal.is_none() {
        return;
    }

    // Calculate modal size: 2/3 width, 80% height
    let width = area.width.saturating_mul(2).saturating_div(3).max(40).min(area.width);
    let height = area.height.saturating_mul(4).saturating_div(5).max(10).min(area.height);

    // Center the modal
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    // Get styles before any mutable borrows
    let header_style = header_text_style(app);
    let border_s = border_style(app, true);

    // Build the help content
    let sections = app.help_entries();
    let mut lines: Vec<Line> = Vec::new();

    // Calculate column widths for alignment
    let content_width = width.saturating_sub(4) as usize; // Account for borders and padding
    let action_width = 20usize;

    for (section_idx, section) in sections.iter().enumerate() {
        // Section header
        let header_text = format!(" {} ", section.title);
        let padding_total = content_width.saturating_sub(header_text.len());
        let left_pad = padding_total / 2;
        let right_pad = padding_total - left_pad;
        let header_line = format!(
            "{}{}{}",
            LINE.horizontal.to_string().repeat(left_pad),
            header_text,
            LINE.horizontal.to_string().repeat(right_pad)
        );
        lines.push(Line::from(Span::styled(header_line, header_style)));

        // Section entries
        for entry in &section.entries {
            let action = format!("{:<width$}", entry.action, width = action_width);
            let keys = &entry.keys;
            lines.push(Line::from(vec![
                Span::styled(action, Style::default()),
                Span::styled(keys.clone(), header_style),
            ]));
        }

        // Blank line between sections (except after the last one)
        if section_idx < sections.len() - 1 {
            lines.push(Line::from(""));
        }
    }

    let total_lines = lines.len();
    // Viewport height is the inner height minus space for footer
    let inner_height = height.saturating_sub(3) as usize; // borders (2) + footer line (1)

    // Now we can safely mutate the modal
    let modal = app.help_modal.as_mut().unwrap();
    modal.total_lines = total_lines;
    modal.viewport_height = inner_height;

    // Clamp scroll to valid range
    let max_scroll = modal.total_lines.saturating_sub(modal.viewport_height);
    if modal.scroll > max_scroll {
        modal.scroll = max_scroll;
    }

    let scroll = modal.scroll;
    let viewport_height = modal.viewport_height;
    let can_scroll_up = modal.can_scroll_up();
    let can_scroll_down = modal.can_scroll_down();

    // Apply scroll offset
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(viewport_height)
        .collect();

    // Build scroll indicators
    let scroll_indicator = match (can_scroll_up, can_scroll_down) {
        (true, true) => "▲▼",
        (true, false) => "▲ ",
        (false, true) => " ▼",
        (false, false) => "  ",
    };

    // Build the title with scroll indicator
    let title = Line::from(vec![
        Span::styled(" HELP ", header_style),
        Span::styled(scroll_indicator, header_style),
    ]);

    // Build footer
    let footer = Line::from(Span::styled(
        format!(" {} ", HELP_MODAL_FOOTER),
        header_style,
    ));

    // Create the block with title and footer
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_s)
        .title(title)
        .title_bottom(footer)
        .title_alignment(Alignment::Center);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Render the content as a paragraph
    let paragraph = Paragraph::new(visible_lines);
    frame.render_widget(paragraph, inner);
}

fn draw_reindex_modal(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(modal) = &app.reindex_modal else {
        return;
    };

    let width = 30u16.min(area.width);
    let height = 3u16.min(area.height);

    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app, true));

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let text = Paragraph::new(modal.message.clone())
        .alignment(Alignment::Center)
        .style(header_text_style(app));
    frame.render_widget(text, inner);
}

fn draw_share_modal(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(modal) = &app.share_modal else {
        return;
    };

    let qr_width = modal
        .qr_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let qr_height = modal.qr_lines.len() as u16;

    // Add padding for borders and title/footer
    let width = (qr_width + 4).min(area.width);
    let height = (qr_height + 4).min(area.height); // +2 for borders, +2 for title/footer

    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, modal_area);

    let title = Line::from(Span::styled(" SHARE ", header_text_style(app)));
    let footer = Line::from(Span::styled(" Esc: close ", header_text_style(app)));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style(app, true))
        .title(title)
        .title_bottom(footer)
        .title_alignment(Alignment::Center);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Render QR code lines - invert the rendering so dark modules are selection color
    // The QR was rendered with Light for dark and Dark for light, so we need to swap
    // by using selection_bg as background and default as foreground
    let colors = app.ui_colors();
    let qr_style = Style::default()
        .fg(color(colors.selection_bg))
        .bg(Color::Reset);

    let qr_text: Vec<Line> = modal
        .qr_lines
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), qr_style)))
        .collect();

    let paragraph = Paragraph::new(qr_text).alignment(Alignment::Center);
    frame.render_widget(paragraph, inner);
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

    render_header_with_separator(frame, layout[0], build_tab_header(app), app, None, area.width);

    let tab_index = app.tab.index();
    let fields = &app.tab_fields[tab_index];

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor = None;
    if fields.is_empty() {
        lines.push(Line::from("No data"));
    } else {
        // Calculate max label width for alignment (label + colon)
        let label_width = fields
            .iter()
            .map(|f| f.label.len() + 1) // +1 for colon
            .max()
            .unwrap_or(0);

        for (idx, field) in fields.iter().enumerate() {
            let highlight = focused && idx == app.tab_field_indices[tab_index];
            let line_index = lines.len();
            let (line, cursor_info) = field_line(app, field, highlight, label_width);
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
    let message: String = if app.alias_modal.is_some() {
        ADD_ALIAS_HELP.to_string()
    } else if let Some(modal) = app.multivalue_modal() {
        if modal.field() == MultiValueField::Alias {
            ALIAS_MODAL_HELP.to_string()
        } else {
            MULTIVALUE_HELP.to_string()
        }
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

fn draw_alias_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.alias_modal.is_none() { return; }

    let mut width = area.width.saturating_mul(2).saturating_div(3);
    let min_width = area.width.min(30);
    if width < min_width { width = min_width; }
    if width > area.width { width = area.width; }

    let content_width = width.saturating_sub(2) as usize;

    let label = "ALIAS: ";
    let value = app
        .alias_modal
        .as_ref()
        .map(|m| m.input.value().to_string())
        .unwrap_or_default();
    let line = Line::from(vec![
        Span::styled(label, header_text_style(app)),
        Span::raw(value.clone()),
    ]);
    let lines = vec![
        line,
        Line::from("".to_string()),
        Line::from(ADD_ALIAS_HELP.to_string()),
    ];

    let body_text = ratatui::text::Text::from(
        lines
            .into_iter()
            .map(|l| {
                let ln = l;
                if ln.width() < content_width { /* let popup size itself */ }
                ln
            })
            .collect::<Vec<Line>>()
    );

    let title_line = Line::from(Span::styled("ADD ALIAS", header_text_style(app)));
    let popup = Popup::new(body_text)
        .title(title_line)
        .border_style(border_style(app, true));

    frame.render_stateful_widget_ref(popup, area, &mut app.modal_popup);

    if let Some(area) = app.modal_popup.area() {
        let inner = Block::default().borders(Borders::ALL).inner(*area);
        if let Some(m) = app.alias_modal.as_ref() {
            let x = inner.x.saturating_add(label.len() as u16 + m.input.visual_cursor() as u16);
            let y = inner.y;
            frame.set_cursor_position((x, y));
        }
    }
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

fn field_line(
    app: &App,
    field: &PaneField,
    highlight: bool,
    label_width: usize,
) -> (Line<'static>, Option<usize>) {
    let editing = app.editor.active
        && field
            .source()
            .zip(app.editor.target())
            .map(|(lhs, rhs)| lhs == *rhs)
            .unwrap_or(false);
    let (label_style, value_style) = line_styles(app, highlight || editing);
    // Pad the label (including colon) to consistent width, then add space before value
    let label = format!("{:width$} ", format!("{}:", field.label), width = label_width);
    let mut spans = vec![Span::styled(label.clone(), label_style)];
    let mut cursor = None;

    if editing {
        let value = app.editor.value().to_string();
        let visual_label_width = Span::raw(&label).width();
        let cursor_column = visual_label_width + app.editor.visual_cursor();
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

/// Render a header line with a separator below it.
/// `area` is the inner content area for the header.
/// `outer_width` is the full pane width (including borders) for drawing connected separators.
fn render_header_with_separator(
    frame: &mut Frame<'_>,
    area: Rect,
    content: Line<'static>,
    app: &App,
    style: Option<Style>,
    outer_width: u16,
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

    // Build separator with connector characters: ├───┤
    // The separator spans the full outer width to connect with side borders
    let inner_width = outer_width.saturating_sub(2) as usize; // exclude border chars
    let separator = format!(
        "{}{}{}",
        LINE.vertical_right,
        LINE.horizontal.to_string().repeat(inner_width),
        LINE.vertical_left
    );
    let separator_line = Line::from(Span::styled(separator, separator_style(app)));
    
    // Render at the separator row, but shifted left by 1 to start at the border
    let separator_area = Rect {
        x: layout[1].x.saturating_sub(1),
        y: layout[1].y,
        width: outer_width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(separator_line), separator_area);
}

fn color(rgb: RgbColor) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}
const CONFIRM_HELP: &str = "Y/Enter: confirm  N/Esc: cancel";
