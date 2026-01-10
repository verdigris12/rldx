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

use super::app::{AddFieldState, App, DetailsField, DetailsSection, MultiValueField, PaneField, PaneFocus, SearchFocus, SearchRow, STANDARD_PROPERTIES, TYPE_VALUES};

const MULTIVALUE_HELP: &str =
    "j/k: nav  Space: copy  Enter: default  e: edit  q/Esc: close";
const ALIAS_MODAL_HELP: &str =
    "j/k: nav  Space: copy  e: edit  a: add  x: delete  q/Esc: close";
const SEARCH_HELP_INPUT: &str =
    "Type to filter  Esc: focus results  Enter: open";
const ADD_ALIAS_HELP: &str = "Type alias  Enter: add  Esc: cancel";
const ADD_FIELD_HELP: &str = "j/k: nav  Enter: select  Esc: back/close";
const PHOTO_PATH_HELP: &str = "Enter path to image  Enter: set  Esc: cancel";
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
    draw_add_field_modal(frame, size, app);
    draw_photo_path_modal(frame, size, app);
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
    // Add 1-line gap at top to separate from F-button bar
    let top_gap = 1u16;
    if area.height <= top_gap {
        return;
    }
    let content_area = Rect {
        x: area.x,
        y: area.y + top_gap,
        width: area.width,
        height: area.height - top_gap,
    };

    let image_height = app.image_pane_height().min(content_area.height);
    let upper_height = image_height.min(content_area.height);

    let main_height = upper_height.min(content_area.height);
    let lower_start = content_area.y + upper_height;

    let top_rect = Rect {
        x: content_area.x,
        y: content_area.y,
        width: content_area.width,
        height: upper_height,
    };

    let lower_rect = Rect {
        x: content_area.x,
        y: lower_start,
        width: content_area.width,
        height: content_area.height.saturating_sub(upper_height),
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
        draw_details_pane(frame, lower_rect, app);
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
    
    if area.width < 2 || area.height < 2 {
        return;
    }

    // Render panel header (row 0) - fills entire width with accent background
    let title = app.current_contact
        .as_ref()
        .map(|c| c.display_fn.to_uppercase())
        .unwrap_or_else(|| "NO CONTACT SELECTED".to_string());
    let header_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    render_panel_header(frame, header_area, &title, '1', app);

    // Render panel borders (left, right, bottom) and get content area
    let content_area = render_panel_borders(frame, area, app, active);
    
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

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

    frame.render_widget(Paragraph::new(lines), content_area);

    if let Some((line_idx, column)) = cursor {
        let x = content_area.x.saturating_add(column as u16);
        let y = content_area.y.saturating_add(line_idx as u16);
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
    let focused = matches!(app.focused_pane, PaneFocus::Image);

    if area.width < 2 || area.height < 2 {
        return;
    }

    // Render panel header (row 0) - fills entire width with accent background
    let header_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    render_panel_header(frame, header_area, "IMAGE", '3', app);

    // Render panel borders (left, right, bottom) and get content area
    let content_area = render_panel_borders(frame, area, app, focused);

    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    frame.render_widget(Clear, content_area);

    let render_area = image_render_area(app, content_area);

    if let Some(state) = app.profile_image_state() {
        let widget = StatefulImage::new(None).resize(Resize::Fit);
        frame.render_stateful_widget(widget, render_area, state);
        return;
    }

    if let Some(error) = app.photo_error() {
        render_centered_words(frame, content_area, error);
        return;
    }

    if let Some(contact) = &app.current_contact {
        let message = if contact.has_photo {
            "PHOTO NOT EMBEDDED"
        } else {
            "NO IMAGE AVAILABLE"
        };
        render_centered_words(frame, content_area, message);
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

fn draw_details_pane(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let focused = matches!(app.focused_pane, PaneFocus::Details);

    if area.width < 2 || area.height < 2 {
        return;
    }

    // Render panel header (row 0) - fills entire width with accent background
    let header_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
    render_panel_header(frame, header_area, "DETAILS", '2', app);

    // Render panel borders (left, right, bottom) and get content area
    let content_area = render_panel_borders(frame, area, app, focused);

    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    // Build all lines from sections
    // Track which line indices are section separators (for ├─┤ rendering)
    let mut lines: Vec<Line> = Vec::new();
    let mut section_separator_indices: Vec<usize> = Vec::new();
    let mut field_line_indices: Vec<usize> = Vec::new(); // Maps flat field index to line index
    let mut flat_field_index = 0usize;

    for section in &app.details_sections {
        // Blank line before section separator
        lines.push(Line::from(""));
        
        // Section separator line: ─ SectionName ──────────────
        // (├ and ┤ will be rendered separately over the border)
        section_separator_indices.push(lines.len());
        let section_header = render_section_header(&section.name, content_area.width as usize);
        lines.push(Line::from(Span::styled(section_header, separator_style(app))));
        
        // Blank line after section separator
        lines.push(Line::from(""));
        
        // Collect property columns for this section
        // Each property (e.g., TYPE, NOTE) gets columns equal to max values across fields
        let prop_columns = collect_section_property_columns(section);
        
        // Calculate column widths for this section
        let label_width = section.fields.iter()
            .map(|f| f.label.len())
            .max()
            .unwrap_or(0)
            .max(5); // Minimum label width
        
        // Calculate max value width for alignment
        let value_width = section.fields.iter()
            .map(|f| f.value.len())
            .max()
            .unwrap_or(0)
            .max(10); // Minimum value width
        
        // Add column header row if there are property columns
        if !prop_columns.is_empty() {
            let header_line = build_section_column_header(
                app,
                label_width,
                value_width,
                &prop_columns,
            );
            lines.push(header_line);
        }
        
        // Add section fields
        for field in &section.fields {
            let highlight = focused && flat_field_index == app.details_field_index;
            field_line_indices.push(lines.len());
            let line = build_details_field_line(
                app,
                field,
                highlight,
                label_width,
                value_width,
                &prop_columns,
            );
            lines.push(line);
            flat_field_index += 1;
        }
    }

    if lines.is_empty() {
        lines.push(Line::from("No data"));
    }

    // Apply scroll offset and track visible section separators
    let scroll = app.details_scroll;
    let viewport_height = content_area.height as usize;
    let visible_lines: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(viewport_height)
        .cloned()
        .collect();

    frame.render_widget(Paragraph::new(visible_lines), content_area);

    // Render ├ and ┤ for visible section separators (use separator_style to match the ─ line)
    let sep_style = separator_style(app);
    for &line_idx in &section_separator_indices {
        // Check if this separator is visible
        if line_idx >= scroll && line_idx < scroll + viewport_height {
            let visible_row = line_idx - scroll;
            let y = content_area.y + visible_row as u16;
            
            // Render ├ at left border position (area.x, which is one column left of content)
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(LINE.vertical_right, sep_style))),
                Rect { x: area.x, y, width: 1, height: 1 }
            );
            // Render ┤ at right border position
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(LINE.vertical_left, sep_style))),
                Rect { x: area.x + area.width - 1, y, width: 1, height: 1 }
            );
        }
    }
}

/// Render a section header line: ─ Name ─────────────────
fn render_section_header(name: &str, width: usize) -> String {
    let prefix = "─ ";
    let suffix = " ";
    let name_part = format!("{}{}{}", prefix, name, suffix);
    let remaining = width.saturating_sub(name_part.len());
    let dashes = "─".repeat(remaining);
    format!("{}{}", name_part, dashes)
}

/// Property column info: name and number of columns needed
#[derive(Debug, Clone)]
struct PropColumn {
    name: String,
    count: usize,  // Number of columns (max values across all fields)
    width: usize,  // Column width (max value length + padding)
}

/// Collect property columns for a section
/// Returns list of (property_name, column_count, column_width)
fn collect_section_property_columns(section: &DetailsSection) -> Vec<PropColumn> {
    use std::collections::BTreeMap;
    
    // Track max value count and max value width for each property
    let mut prop_info: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // name -> (max_count, max_width)
    
    for field in &section.fields {
        for (prop_name, values) in &field.params {
            let entry = prop_info.entry(prop_name.clone()).or_insert((0, 0));
            // Update max count
            entry.0 = entry.0.max(values.len());
            // Update max width
            for v in values {
                entry.1 = entry.1.max(v.len());
            }
        }
    }
    
    // Convert to PropColumn list
    prop_info.into_iter()
        .map(|(name, (count, max_width))| PropColumn {
            name,
            count,
            width: max_width.max(4) + 1, // At least 4 chars + 1 space padding
        })
        .collect()
}

/// Build column header row for a section
/// Format: (padding for label+value) then property headers
fn build_section_column_header(
    app: &App,
    label_width: usize,
    value_width: usize,
    prop_columns: &[PropColumn],
) -> Line<'static> {
    let header_style = header_text_style(app);
    
    let mut spans: Vec<Span> = Vec::new();
    
    // Padding for label column (label_width + ": ")
    let label_col_width = label_width + 2;
    spans.push(Span::raw(" ".repeat(label_col_width)));
    
    // Padding for value column + gap
    spans.push(Span::raw(" ".repeat(value_width + 1)));
    
    // Property column headers
    for prop in prop_columns {
        // One header per column (repeated if count > 1)
        for _ in 0..prop.count {
            spans.push(Span::styled(format!("{:<width$}", prop.name, width = prop.width), header_style));
        }
    }
    
    Line::from(spans)
}

/// Build a field line for details pane with aligned columns
fn build_details_field_line(
    app: &App,
    field: &DetailsField,
    highlight: bool,
    label_width: usize,
    value_width: usize,
    prop_columns: &[PropColumn],
) -> Line<'static> {
    let (label_style, value_style) = line_styles(app, highlight);
    
    let mut spans: Vec<Span> = Vec::new();
    
    // Label column (right-padded)
    let label_text = format!("{:width$}: ", field.label, width = label_width);
    spans.push(Span::styled(label_text, label_style));
    
    // Value column (right-padded to value_width)
    let value_text = format!("{:<width$}", field.value, width = value_width);
    spans.push(Span::styled(value_text, value_style));
    
    // Gap before property columns
    if !prop_columns.is_empty() {
        spans.push(Span::raw(" "));
    }
    
    // Property columns - fill with values or empty space
    for prop in prop_columns {
        let values = field.params.get(&prop.name);
        for i in 0..prop.count {
            let cell = values
                .and_then(|v| v.get(i))
                .map(|s| s.as_str())
                .unwrap_or("");
            spans.push(Span::styled(format!("{:<width$}", cell, width = prop.width), value_style));
        }
    }
    
    Line::from(spans)
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let message: String = if app.alias_modal.is_some() {
        ADD_ALIAS_HELP.to_string()
    } else if app.photo_path_modal.is_some() {
        PHOTO_PATH_HELP.to_string()
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

fn draw_add_field_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let Some(modal) = &app.add_field_modal else { return; };

    let mut width = area.width.saturating_mul(2).saturating_div(3);
    let min_width = area.width.min(40);
    if width < min_width { width = min_width; }
    if width > area.width { width = area.width; }

    let _content_width = width.saturating_sub(4) as usize;

    // Build content based on current state
    let (title, lines, cursor_info) = match modal.state {
        AddFieldState::SelectProperty => {
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled("Select property type:", header_text_style(app))));
            lines.push(Line::from(""));

            for (idx, (display_name, _, _)) in STANDARD_PROPERTIES.iter().enumerate() {
                let prefix = if idx == modal.property_index { "► " } else { "  " };
                let style = if idx == modal.property_index {
                    selection_style(app)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(format!("{}{}", prefix, display_name), style)));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(ADD_FIELD_HELP));
            ("ADD FIELD", lines, None)
        }
        AddFieldState::SelectType => {
            let mut lines: Vec<Line> = Vec::new();
            let prop_name = modal.current_property().map(|(name, _, _)| name).unwrap_or("Field");
            lines.push(Line::from(Span::styled(format!("Select type for {}:", prop_name), header_text_style(app))));
            lines.push(Line::from(""));

            // Option for "no type"
            let no_type_selected = modal.type_index.is_none();
            let prefix = if no_type_selected { "► " } else { "  " };
            let style = if no_type_selected { selection_style(app) } else { Style::default() };
            lines.push(Line::from(Span::styled(format!("{}(none)", prefix), style)));

            for (idx, type_val) in TYPE_VALUES.iter().enumerate() {
                let is_selected = modal.type_index == Some(idx);
                let prefix = if is_selected { "► " } else { "  " };
                let style = if is_selected { selection_style(app) } else { Style::default() };
                lines.push(Line::from(Span::styled(format!("{}{}", prefix, type_val), style)));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(ADD_FIELD_HELP));
            ("SELECT TYPE", lines, None)
        }
        AddFieldState::EnterCustomProperty => {
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled("Enter custom property name:", header_text_style(app))));
            lines.push(Line::from(""));

            let label = "X-";
            let value = modal.custom_property_input.value();
            lines.push(Line::from(vec![
                Span::styled(label, header_text_style(app)),
                Span::raw(value.to_string()),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from("Enter: continue  Esc: back"));

            let cursor_col = label.len() + modal.custom_property_input.visual_cursor();
            ("CUSTOM PROPERTY", lines, Some((2, cursor_col))) // Line 2 is the input line
        }
        AddFieldState::EnterValue => {
            let mut lines: Vec<Line> = Vec::new();
            let prop_name = if let Some((name, vcard_field, _)) = modal.current_property() {
                if vcard_field == "X-" {
                    let custom = modal.custom_property_input.value().trim();
                    if custom.is_empty() {
                        "X-".to_string()
                    } else if custom.to_uppercase().starts_with("X-") {
                        custom.to_uppercase()
                    } else {
                        format!("X-{}", custom.to_uppercase())
                    }
                } else {
                    name.to_string()
                }
            } else {
                "Field".to_string()
            };

            let type_suffix = modal.current_type()
                .map(|t| format!(" ({})", t))
                .unwrap_or_default();

            lines.push(Line::from(Span::styled(format!("Enter value for {}{}:", prop_name, type_suffix), header_text_style(app))));
            lines.push(Line::from(""));

            let label = "VALUE: ";
            let value = modal.value_input.value();
            lines.push(Line::from(vec![
                Span::styled(label, header_text_style(app)),
                Span::raw(value.to_string()),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from("Enter: add field  Esc: back"));

            let cursor_col = label.len() + modal.value_input.visual_cursor();
            ("ENTER VALUE", lines, Some((2, cursor_col)))
        }
    };

    // Calculate height needed
    let _content_height = lines.len();
    let body_lines: Vec<Line> = lines.into_iter().collect();
    let body_text = ratatui::text::Text::from(body_lines.clone());

    let title_line = Line::from(Span::styled(title, header_text_style(app)));
    let popup = Popup::new(body_text)
        .title(title_line)
        .border_style(border_style(app, true));

    frame.render_stateful_widget_ref(popup, area, &mut app.modal_popup);

    // Set cursor if we have an input field active
    if let Some((line_idx, cursor_col)) = cursor_info {
        if let Some(popup_area) = app.modal_popup.area() {
            let inner = Block::default().borders(Borders::ALL).inner(*popup_area);
            let x = inner.x.saturating_add(cursor_col as u16);
            let y = inner.y.saturating_add(line_idx as u16);
            frame.set_cursor_position((x, y));
        }
    }
}

fn draw_photo_path_modal(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    if app.photo_path_modal.is_none() { return; }

    let mut width = area.width.saturating_mul(2).saturating_div(3);
    let min_width = area.width.min(50);
    if width < min_width { width = min_width; }
    if width > area.width { width = area.width; }

    let content_width = width.saturating_sub(2) as usize;

    let label = "PATH: ";
    let value = app
        .photo_path_modal
        .as_ref()
        .map(|m| m.input.value().to_string())
        .unwrap_or_default();
    let line = Line::from(vec![
        Span::styled(label, header_text_style(app)),
        Span::raw(value.clone()),
    ]);
    let lines = vec![
        Line::from(Span::styled("Enter path to image file (max 128x128)", header_text_style(app))),
        Line::from(""),
        line,
        Line::from(""),
        Line::from(PHOTO_PATH_HELP.to_string()),
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

    let title_line = Line::from(Span::styled("SET PHOTO", header_text_style(app)));
    let popup = Popup::new(body_text)
        .title(title_line)
        .border_style(border_style(app, true));

    frame.render_stateful_widget_ref(popup, area, &mut app.modal_popup);

    if let Some(popup_area) = app.modal_popup.area() {
        let inner = Block::default().borders(Borders::ALL).inner(*popup_area);
        if let Some(m) = app.photo_path_modal.as_ref() {
            // Cursor on the input line (line index 2)
            let x = inner.x.saturating_add(label.len() as u16 + m.input.visual_cursor() as u16);
            let y = inner.y.saturating_add(2); // Line index 2 is the input line
            frame.set_cursor_position((x, y));
        }
    }
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

/// Render a panel header: 1-line with accent background, title left-aligned, "| #" right-aligned
/// Uses half-block characters at edges: ▐ (right half) on left, ▌ (left half) on right
fn render_panel_header(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    panel_number: char,
    app: &App,
) {
    if area.width < 2 || area.height == 0 {
        return;
    }

    let colors = app.ui_colors();
    let header_style = Style::default()
        .fg(color(colors.selection_fg))
        .bg(color(colors.selection_bg));

    // Half-block characters for visual corners
    const RIGHT_HALF_BLOCK: &str = "▐";  // Right half block (U+2590) - creates left edge
    const LEFT_HALF_BLOCK: &str = "▌"; // Left half block (U+258C) - creates right edge

    // Build the line: "▐TITLE ... | #▌"
    let suffix = format!("| {}", panel_number);
    let title_len = title.len();
    let suffix_len = suffix.len();
    // Available space for content (excluding the two half-block chars)
    let inner_width = (area.width as usize).saturating_sub(2);

    // Calculate padding between title and suffix
    let padding_needed = inner_width.saturating_sub(title_len + suffix_len);
    let padding = " ".repeat(padding_needed);

    let line = Line::from(vec![
        Span::styled(LEFT_HALF_BLOCK, header_style),
        Span::styled(title.to_string(), header_style),
        Span::styled(padding, header_style),
        Span::styled(suffix, header_style),
        Span::styled(RIGHT_HALF_BLOCK, header_style),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

/// Render panel borders (left, right, bottom) and return the content area.
/// The header row is assumed to be already rendered at area.y.
/// This renders:
/// - Left border (│) from row 1 to row height-2
/// - Right border (│) from row 1 to row height-2  
/// - Bottom border (└───┘) at row height-1
fn render_panel_borders(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    focused: bool,
) -> Rect {
    if area.width < 2 || area.height < 2 {
        return Rect::default();
    }

    let border_style = border_style(app, focused);
    
    // Content area is inside the borders, starting from row 1 (after header)
    let content_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2), // -1 for header, -1 for bottom border
    };

    // Render left and right borders for content rows
    for row in 1..area.height.saturating_sub(1) {
        let y = area.y + row;
        // Left border
        let left_span = Span::styled(LINE.vertical, border_style);
        frame.render_widget(
            Paragraph::new(Line::from(left_span)),
            Rect { x: area.x, y, width: 1, height: 1 }
        );
        // Right border
        let right_span = Span::styled(LINE.vertical, border_style);
        frame.render_widget(
            Paragraph::new(Line::from(right_span)),
            Rect { x: area.x + area.width - 1, y, width: 1, height: 1 }
        );
    }

    // Render bottom border
    let bottom_y = area.y + area.height - 1;
    let inner_width = area.width.saturating_sub(2) as usize;
    let bottom_line = format!(
        "{}{}{}",
        LINE.bottom_left,
        LINE.horizontal.repeat(inner_width),
        LINE.bottom_right
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(bottom_line, border_style))),
        Rect { x: area.x, y: bottom_y, width: area.width, height: 1 }
    );

    content_area
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

fn color(rgb: RgbColor) -> Color {
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}
const CONFIRM_HELP: &str = "Y/Enter: confirm  N/Esc: cancel";
