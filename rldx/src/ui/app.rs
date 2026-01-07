use std::collections::HashSet;
use std::convert::TryFrom;
use std::io::{stdout, Write};
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use serde_json::Value;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;
use tui_widgets::popup::PopupState;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;

use crate::config::{CommandExec, Config, UiColors};
use crate::db::{ContactItem, ContactListEntry, Database, PropRow};
use crate::indexer;
use crate::search;
use crate::vcard_io;
use crate::vdir;
use vcard4::{property::TextProperty, Vcard};

use image::{self, DynamicImage};

use super::draw;
use super::edit::{FieldRef, InlineEditor};
use super::panes::DetailTab;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneField {
    pub label: String,
    pub value: String,
    pub copy_value: String,
    pub source: Option<FieldRef>,
}

impl PaneField {
    fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self::with_source(label, value, None, None)
    }

    fn from_prop(
        label: impl Into<String>,
        value: impl Into<String>,
        copy_value: impl Into<String>,
        field: &str,
        seq: i64,
        component: Option<usize>,
    ) -> Self {
        Self::with_source(
            label,
            value,
            Some(copy_value.into()),
            Some(match component {
                Some(index) => FieldRef::with_component(field.to_string(), seq, index),
                None => FieldRef::new(field.to_string(), seq),
            }),
        )
    }

    fn with_source(
        label: impl Into<String>,
        value: impl Into<String>,
        copy_value: Option<String>,
        source: Option<FieldRef>,
    ) -> Self {
        let value = value.into();
        let copy_value = copy_value.unwrap_or_else(|| value.clone());
        Self {
            label: label.into(),
            value,
            copy_value,
            source,
        }
    }

    pub fn copy_text(&self) -> &str {
        &self.copy_value
    }

    pub fn source(&self) -> Option<FieldRef> {
        self.source.clone()
    }
}

const DEFAULT_FONT_SIZE: (u16, u16) = (8, 16);

fn create_image_picker() -> Picker {
    let mut picker = base_picker();
    picker.guess_protocol();
    picker
}

#[cfg(unix)]
fn base_picker() -> Picker {
    Picker::from_termios().unwrap_or_else(|_| Picker::new(DEFAULT_FONT_SIZE))
}

#[cfg(not(unix))]
fn base_picker() -> Picker {
    Picker::new(DEFAULT_FONT_SIZE)
}

const DEFAULT_ADDRESS_BOOK: &str = "default";

#[derive(Debug, Clone)]
pub struct SearchRow {
    pub text: String,
    pub depth: u16,
    pub contact_index: Option<usize>,
}

impl SearchRow {
    pub fn selectable(&self) -> bool {
        self.contact_index.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Search,
    Card,
    Detail(DetailTab),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiValueField {
    Email,
    Phone,
}

impl MultiValueField {
    pub fn from_field_name(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "EMAIL" => Some(Self::Email),
            "TEL" => Some(Self::Phone),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Email => "EMAIL ADDRESSES",
            Self::Phone => "PHONE NUMBERS",
        }
    }

    fn field_name(self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
            Self::Phone => "TEL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MultiValueItem {
    pub seq: i64,
    pub value: String,
    pub copy_value: String,
    pub type_label: String,
}

#[derive(Debug, Clone)]
pub struct ConfirmModal {
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct AliasModal {
    pub input: Input,
}

/// Help modal state with scroll support
#[derive(Debug, Clone)]
pub struct HelpModal {
    /// Current scroll offset (line index at top of viewport)
    pub scroll: usize,
    /// Total number of content lines
    pub total_lines: usize,
    /// Viewport height (set during rendering)
    pub viewport_height: usize,
}

impl HelpModal {
    pub fn new(total_lines: usize) -> Self {
        Self {
            scroll: 0,
            total_lines,
            viewport_height: 10, // Will be updated during render
        }
    }

    pub fn scroll_down(&mut self, lines: usize) {
        let max_scroll = self.total_lines.saturating_sub(self.viewport_height);
        self.scroll = (self.scroll + lines).min(max_scroll);
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll = 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = self.total_lines.saturating_sub(self.viewport_height);
    }

    pub fn can_scroll_up(&self) -> bool {
        self.scroll > 0
    }

    pub fn can_scroll_down(&self) -> bool {
        self.scroll + self.viewport_height < self.total_lines
    }
}

/// A section in the help modal (e.g., "Global", "Navigation")
pub struct HelpSection {
    pub title: &'static str,
    pub entries: Vec<HelpEntry>,
}

/// A single help entry (action name + key bindings)
pub struct HelpEntry {
    pub action: &'static str,
    pub keys: String,
}

#[derive(Debug, Clone)]
pub struct MultiValueModal {
    field: MultiValueField,
    items: Vec<MultiValueItem>,
    selected: usize,
}

impl MultiValueModal {
    fn new(field: MultiValueField, items: Vec<MultiValueItem>, selected: usize) -> Self {
        let selected = if items.is_empty() {
            0
        } else {
            selected.min(items.len().saturating_sub(1))
        };
        Self {
            field,
            items,
            selected,
        }
    }

    pub fn field(&self) -> MultiValueField {
        self.field
    }

    pub fn items(&self) -> &[MultiValueItem] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_item(&self) -> Option<&MultiValueItem> {
        self.items.get(self.selected)
    }

    fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
    }

    fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.items.len() - 1;
        } else {
            self.selected -= 1;
        }
    }
}

pub struct App<'a> {
    db: &'a mut Database,
    config: &'a Config,
    pub contacts: Vec<ContactListEntry>,
    pub selected: usize,
    pub search_input: Input,
    pub show_search: bool,
    pub search_focus: SearchFocus,
    pub current_contact: Option<ContactItem>,
    pub current_props: Vec<PropRow>,
    pub tab: DetailTab,
    pub editor: InlineEditor,
    pub status: Option<String>,
    pub aliases: Vec<String>,
    pub languages: Vec<String>,
    pub focused_pane: PaneFocus,
    pub card_fields: Vec<PaneField>,
    pub card_field_index: usize,
    pub tab_fields: [Vec<PaneField>; DetailTab::COUNT],
    pub tab_field_indices: [usize; DetailTab::COUNT],
    pub search_rows: Vec<SearchRow>,
    pub selected_row: Option<usize>,
    // Marked contacts by UUID
    pub marked: HashSet<String>,
    // When true, the search pane shows only marked contacts
    pub show_marked_only: bool,
    image_picker: Picker,
    image_state: Option<Box<dyn StatefulProtocol>>,
    pub photo_data: Option<PhotoData>,
    pub photo_error: Option<String>,
    multivalue_modal: Option<MultiValueModal>,
    // Popup state for modal dialog (tui-widgets popup)
    pub modal_popup: PopupState,
    // Merge confirmation modal
    pub confirm_modal: Option<ConfirmModal>,
    // Add-alias modal
    pub alias_modal: Option<AliasModal>,
    // Help modal (F1)
    pub help_modal: Option<HelpModal>,
}

impl<'a> App<'a> {
    pub fn new(db: &'a mut Database, config: &'a Config) -> Result<Self> {
        let contacts = db.list_contacts(None)?;
        let mut app = Self {
            db,
            config,
            contacts,
            selected: 0,
            search_input: Input::default(),
            show_search: true,
            search_focus: SearchFocus::Input,
            current_contact: None,
            current_props: Vec::new(),
            tab: DetailTab::Work,
            editor: InlineEditor::default(),
            status: None,
            aliases: Vec::new(),
            languages: Vec::new(),
            focused_pane: PaneFocus::Search,
            card_fields: Vec::new(),
            card_field_index: 0,
            tab_fields: Default::default(),
            tab_field_indices: [0; DetailTab::COUNT],
            search_rows: Vec::new(),
            selected_row: None,
            marked: HashSet::new(),
            show_marked_only: false,
            image_picker: create_image_picker(),
            image_state: None,
            photo_data: None,
            photo_error: None,
            multivalue_modal: None,
            modal_popup: PopupState::default(),
            confirm_modal: None,
            alias_modal: None,
            help_modal: None,
        };
        app.rebuild_search_rows();
        app.load_selection()?;
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        stdout.execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let result = self.event_loop(&mut terminal);

        disable_raw_mode()?;
        terminal.backend_mut().execute(LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        result
    }

    fn event_loop<B>(&mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: ratatui::backend::Backend,
    {
        loop {
            draw::render(terminal, self)?;

            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(key) => {
                        if self.handle_key(key)? {
                            break;
                        }
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Ctrl+C always quits (hardcoded for safety)
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Ok(true);
        }

        // F1 always opens help (hardcoded)
        if matches!(key.code, KeyCode::F(1)) {
            self.show_help();
            return Ok(false);
        }

        // If help modal is open, handle its keys first
        if self.help_modal.is_some() {
            self.handle_help_modal_key(key);
            return Ok(false);
        }

        // Route to context-specific handlers first
        if self.editor.active && self.handle_editor_key(key)? {
            return Ok(false);
        }

        if self.confirm_modal.is_some() {
            self.handle_confirm_modal_key(key)?;
            return Ok(false);
        }

        if self.alias_modal.is_some() {
            self.handle_alias_modal_key(key)?;
            return Ok(false);
        }

        if self.multivalue_modal.is_some() {
            self.handle_multivalue_modal_key(key)?;
            return Ok(false);
        }

        if self.show_search && self.handle_search_key(key)? {
            return Ok(false);
        }

        // Handle navigation context (card/detail panes when search is closed)
        self.handle_navigation_key(key)
    }

    /// Handle keys in navigation context (card/detail panes)
    fn handle_navigation_key(&mut self, key: KeyEvent) -> Result<bool> {
        let nav = &self.config.keys.navigation;
        let global = &self.config.keys.global;

        // Global: quit (only when not in search)
        if self.key_matches_any(&key, &global.quit) {
            return Ok(true);
        }

        // Global: open search
        if self.key_matches_any(&key, &global.search) {
            self.show_search = true;
            self.focused_pane = PaneFocus::Search;
            self.search_focus = SearchFocus::Input;
            return Ok(false);
        }

        // Global: help
        if self.key_matches_any(&key, &global.help) {
            self.show_help();
            return Ok(false);
        }

        // Navigation: confirm (open multivalue modal if applicable)
        if self.key_matches_any(&key, &nav.confirm)
            && self.open_multivalue_modal_for_current_field()
        {
            return Ok(false);
        }

        // Navigation: next/prev field
        if self.key_matches_any(&key, &nav.next) {
            self.advance_field(1);
            return Ok(false);
        }
        if self.key_matches_any(&key, &nav.prev) {
            self.advance_field(-1);
            return Ok(false);
        }

        // Navigation: tab next/prev (switch between panes)
        if self.key_matches_any(&key, &nav.tab_next) {
            self.advance_tab(1);
            return Ok(false);
        }
        if self.key_matches_any(&key, &nav.tab_prev) {
            self.advance_tab(-1);
            return Ok(false);
        }

        // Navigation: edit
        if self.key_matches_any(&key, &nav.edit) {
            self.begin_edit();
            return Ok(false);
        }

        // Navigation: copy
        if self.key_matches_any(&key, &nav.copy) {
            self.copy_focused_value()?;
            return Ok(false);
        }

        // Navigation: add alias (only when ALIAS field is focused)
        if self.key_matches_any(&key, &nav.add_alias) {
            if let Some(field) = self.focused_field() {
                if field.label.eq_ignore_ascii_case("ALIAS") {
                    self.alias_modal = Some(AliasModal { input: Input::default() });
                    self.set_status("Add alias");
                    return Ok(false);
                }
            }
        }

        // Navigation: photo fetch
        if self.key_matches_any(&key, &nav.photo_fetch) {
            self.set_status("Image fetch is not yet implemented");
            return Ok(false);
        }

        // Navigation: language cycle
        if self.key_matches_any(&key, &nav.lang_cycle) {
            self.set_status("Language toggle not yet implemented");
            return Ok(false);
        }

        // Digit shortcuts for pane focus (1-5)
        if let KeyCode::Char(c) = key.code {
            if self.focus_by_digit(c) {
                return Ok(false);
            }
        }

        Ok(false)
    }

    /// Advance to next/previous tab (pane)
    fn advance_tab(&mut self, delta: isize) {
        match self.focused_pane {
            PaneFocus::Search => {
                // From search, go to card
                self.focus_pane(PaneFocus::Card);
            }
            PaneFocus::Card => {
                if delta > 0 {
                    self.focus_pane(PaneFocus::Detail(DetailTab::Work));
                } else {
                    // Wrap to last tab
                    self.focus_pane(PaneFocus::Detail(DetailTab::Metadata));
                }
            }
            PaneFocus::Detail(tab) => {
                let next_tab = if delta > 0 {
                    tab.next()
                } else {
                    tab.prev()
                };
                match next_tab {
                    Some(t) => self.focus_pane(PaneFocus::Detail(t)),
                    None => self.focus_pane(PaneFocus::Card),
                }
            }
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Result<bool> {
        match self.search_focus {
            SearchFocus::Input => {
                let input_keys = &self.config.keys.search_input;

                // Cancel: move focus to results (do not close search)
                if self.key_matches_any(&key, &input_keys.cancel) {
                    self.search_focus = SearchFocus::Results;
                    return Ok(true);
                }

                // Confirm: open selected contact, collapse search
                if self.key_matches_any(&key, &input_keys.confirm) {
                    self.show_search = false;
                    self.focus_pane(PaneFocus::Card);
                    self.refresh_contacts()?;
                    return Ok(true);
                }

                // Next/prev: navigate results while typing
                if self.key_matches_any(&key, &input_keys.next) {
                    self.move_selection(1)?;
                    return Ok(true);
                }
                if self.key_matches_any(&key, &input_keys.prev) {
                    self.move_selection(-1)?;
                    return Ok(true);
                }

                // Pass other keys to the input widget
                if let Some(change) = self.search_input.handle_event(&Event::Key(key)) {
                    if change.value {
                        self.refresh_contacts()?;
                    }
                    return Ok(true);
                }
                Ok(false)
            }
            SearchFocus::Results => {
                let results_keys = &self.config.keys.search_results;
                let global_keys = &self.config.keys.global;

                // Global: help
                if self.key_matches_any(&key, &global_keys.help) {
                    self.show_help();
                    return Ok(true);
                }

                // Global: search key refocuses input
                if self.key_matches_any(&key, &global_keys.search) {
                    self.search_focus = SearchFocus::Input;
                    return Ok(true);
                }

                // Cancel: close search
                if self.key_matches_any(&key, &results_keys.cancel) {
                    self.show_search = false;
                    self.refresh_contacts()?;
                    return Ok(true);
                }

                // Confirm: open selected contact and collapse search
                if self.key_matches_any(&key, &results_keys.confirm) {
                    self.show_search = false;
                    self.focus_pane(PaneFocus::Card);
                    self.refresh_contacts()?;
                    return Ok(true);
                }

                // Next/prev selection
                if self.key_matches_any(&key, &results_keys.next) {
                    self.move_selection(1)?;
                    return Ok(true);
                }
                if self.key_matches_any(&key, &results_keys.prev) {
                    self.move_selection(-1)?;
                    return Ok(true);
                }

                // Page up/down
                if self.key_matches_any(&key, &results_keys.page_down) {
                    self.move_selection(5)?;
                    return Ok(true);
                }
                if self.key_matches_any(&key, &results_keys.page_up) {
                    self.move_selection(-5)?;
                    return Ok(true);
                }

                // Mark contact for merge
                if self.key_matches_any(&key, &results_keys.mark) {
                    self.toggle_mark_current();
                    if self.show_marked_only {
                        self.rebuild_marked_contacts()?;
                    } else {
                        self.rebuild_search_rows();
                    }
                    return Ok(true);
                }

                // Merge marked contacts
                if self.key_matches_any(&key, &results_keys.merge) {
                    let count = self.marked.len();
                    if count < 2 {
                        self.set_status("Mark at least 2 contacts to merge");
                        return Ok(true);
                    }
                    self.confirm_modal = Some(ConfirmModal {
                        title: "MERGE CONTACTS".to_string(),
                        message: format!(
                            "Merge {} marked contacts into a single card?",
                            count
                        ),
                    });
                    return Ok(true);
                }

                // Toggle marked-only view
                if self.key_matches_any(&key, &results_keys.toggle_marked) {
                    self.show_marked_only = !self.show_marked_only;
                    if self.show_marked_only {
                        self.rebuild_marked_contacts()?;
                    } else {
                        self.refresh_contacts()?;
                    }
                    return Ok(true);
                }

                Ok(false)
            }
        }
    }

    fn toggle_mark_current(&mut self) {
        if let Some(uuid) = self
            .contacts
            .get(self.selected)
            .map(|entry| entry.uuid.clone())
        {
            if !uuid.is_empty() {
                if !self.marked.insert(uuid.clone()) {
                    // already present, remove it
                    self.marked.remove(&uuid);
                    self.set_status("Unmarked");
                } else {
                    self.set_status("Marked");
                }
            }
        }
    }

    fn rebuild_marked_contacts(&mut self) -> Result<()> {
        // load all contacts and filter to marked
        let all = self.db.list_contacts(None)?;
        self.contacts = all
            .into_iter()
            .filter(|c| self.marked.contains(&c.uuid))
            .collect();
        self.sort_contacts();

        if self.selected >= self.contacts.len() {
            self.selected = self.contacts.len().saturating_sub(1);
        }

        self.rebuild_search_rows();
        if self.contacts.is_empty() {
            self.current_contact = None;
            self.current_props.clear();
            self.aliases.clear();
            self.languages.clear();
            self.rebuild_field_views();
        } else {
            self.load_selection()?;
        }
        Ok(())
    }

    fn merge_marked_contacts(&mut self) -> Result<()> {
        use std::path::PathBuf;

        if self.marked.len() < 2 {
            self.set_status("Mark at least 2 contacts to merge");
            return Ok(());
        }

        // Collect paths for marked contacts, preserving current sort order
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in &self.contacts {
            if self.marked.contains(&entry.uuid) {
                paths.push(entry.path.clone());
            }
        }
        if paths.len() < 2 {
            self.set_status("Mark at least 2 contacts to merge");
            return Ok(());
        }

        // Parse cards and merge inductively
        let default_region = self.config.phone_region.as_deref();
        let mut cards: Vec<Vcard> = Vec::new();
        for path in &paths {
            let parsed = vcard_io::parse_file(path, default_region)?;
            if let Some(card) = parsed.cards.into_iter().next() {
                cards.push(card);
            }
        }
        if cards.len() < 2 {
            self.set_status("Unable to load all marked contacts");
            return Ok(());
        }

        let mut merged = cards.remove(0);
        for other in cards.into_iter() {
            merged = merge_two_cards(merged, other);
        }

        // Ensure UID and REV
        let _uid = vcard_io::ensure_uuid_uid(&mut merged)?;
        vcard_io::touch_rev(&mut merged);

        // Determine target path in same directory as first contact
        let first_dir = paths
            .get(0)
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| self.config.vdir.clone());
        let mut used = vdir::existing_stems(&self.config.vdir)?;
        let uuid_str = vcard_io::card_uid(&merged).unwrap_or_default();
        let uuid = uuid::Uuid::parse_str(&uuid_str).unwrap_or_else(|_| uuid::Uuid::new_v4());
        let stem = vdir::select_filename(&uuid, &mut used, None);
        let target = first_dir.join(format!("{stem}.vcf"));

        // Write merged card
        vdir::write_atomic(&target, &vcard_io::card_to_bytes(&merged))?;

        // Remove old files
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }

        // Update DB: delete old, insert new
        self.db
            .delete_items_by_paths(paths.clone().into_iter())?;
        let state = vdir::compute_file_state(&target)?;
        let record = indexer::build_record(&target, &merged, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh UI
        self.marked.clear();
        self.show_marked_only = false;
        self.refresh_contacts()?;
        self.set_status("Merged contacts");
        Ok(())
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Result<bool> {
        if !self.editor.active {
            return Ok(false);
        }

        let editor_keys = &self.config.keys.editor;

        // Cancel editing
        if self.key_matches_any(&key, &editor_keys.cancel) {
            self.editor.cancel();
            self.set_status("Edit cancelled");
            return Ok(true);
        }

        // Confirm edit
        if self.key_matches_any(&key, &editor_keys.confirm) {
            if let Some(target) = self.editor.target().cloned() {
                let value = self.editor.value().to_string();
                self.editor.cancel();
                self.commit_field_edit(target, value)?;
                self.set_status("Field updated");
            } else {
                self.editor.cancel();
                self.set_status("Field not editable");
            }
            return Ok(true);
        }

        // Pass other keys to the editor widget
        self.editor.handle_key_event(key);
        Ok(true)
    }

    fn handle_multivalue_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.multivalue_modal.is_none() {
            return Ok(());
        }

        let editor_keys = &self.config.keys.editor;
        let modal_keys = &self.config.keys.modal;

        // When editing inline inside the modal, use editor keys
        if self.editor.active {
            if self.key_matches_any(&key, &editor_keys.cancel) {
                self.editor.cancel();
                self.set_status("Edit cancelled");
                return Ok(());
            }

            if self.key_matches_any(&key, &editor_keys.confirm) {
                if let Some(target) = self.editor.target().cloned() {
                    let value = self.editor.value().to_string();
                    self.editor.cancel();
                    self.commit_field_edit(target.clone(), value)?;
                    // Keep the modal open and rebuild it for the same field, keep selection
                    if let Some(field) = MultiValueField::from_field_name(&target.field) {
                        self.rebuild_multivalue_modal(field, Some(target.seq));
                    }
                    self.set_status("Field updated");
                    return Ok(());
                }
            }

            // Route other keys to the inline editor
            if self.editor.handle_key_event(key) {
                return Ok(());
            }
        }

        // Modal: cancel (close modal)
        if self.key_matches_any(&key, &modal_keys.cancel) {
            self.close_multivalue_modal();
            return Ok(());
        }

        // Modal: edit selected item
        if self.key_matches_any(&key, &modal_keys.edit) {
            if let Some((field, item)) = self.current_modal_selection() {
                let target = FieldRef::new(field.field_name(), item.seq);
                self.editor.start(&item.value, target);
                self.set_status(format!("Editing {}", field.field_name()));
            }
            return Ok(());
        }

        // Modal: next/prev selection
        if self.key_matches_any(&key, &modal_keys.next) {
            if let Some(modal) = self.multivalue_modal.as_mut() {
                modal.select_next();
            }
            return Ok(());
        }
        if self.key_matches_any(&key, &modal_keys.prev) {
            if let Some(modal) = self.multivalue_modal.as_mut() {
                modal.select_prev();
            }
            return Ok(());
        }

        // Modal: set default
        if self.key_matches_any(&key, &modal_keys.set_default) {
            if let Some((field, item)) = self.current_modal_selection() {
                if self.set_multivalue_default(field, item.seq)? {
                    self.rebuild_multivalue_modal(field, None);
                    self.set_status("Default updated");
                }
            }
            return Ok(());
        }

        // Modal: copy and close
        if self.key_matches_any(&key, &modal_keys.copy) {
            if let Some((_, item)) = self.current_modal_selection() {
                self.copy_value_to_clipboard(&item.copy_value)?;
                self.close_multivalue_modal();
            }
            return Ok(());
        }

        // Modal: confirm (also sets default, for Enter key)
        if self.key_matches_any(&key, &modal_keys.confirm) {
            if let Some((field, item)) = self.current_modal_selection() {
                if self.set_multivalue_default(field, item.seq)? {
                    self.rebuild_multivalue_modal(field, None);
                    self.set_status("Default updated");
                }
            }
            return Ok(());
        }

        Ok(())
    }

    fn handle_alias_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let modal_keys = &self.config.keys.modal;

        // Cancel: close modal
        if self.key_matches_any(&key, &modal_keys.cancel) {
            self.alias_modal = None;
            return Ok(());
        }

        // Confirm: add alias
        if self.key_matches_any(&key, &modal_keys.confirm) {
            let value = self.alias_modal.as_ref()
                .map(|m| m.input.value().trim().to_string())
                .unwrap_or_default();
            self.alias_modal = None;
            if value.is_empty() {
                return Ok(());
            }
            self.add_alias_to_current_contact(&value)?;
            self.set_status("Alias added");
            return Ok(());
        }

        // Route other keys to inline input
        if let Some(modal) = self.alias_modal.as_mut() {
            let _ = modal.input.handle_event(&Event::Key(key));
        }
        Ok(())
    }

    fn handle_confirm_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.confirm_modal.is_none() {
            return Ok(());
        }

        let modal_keys = &self.config.keys.modal;

        // Cancel: close modal without action
        if self.key_matches_any(&key, &modal_keys.cancel) {
            self.confirm_modal = None;
            return Ok(());
        }

        // Also accept 'n' as cancel (common convention)
        if matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'n')) {
            self.confirm_modal = None;
            return Ok(());
        }

        // Confirm: execute action (merge)
        if self.key_matches_any(&key, &modal_keys.confirm) {
            self.confirm_modal = None;
            self.merge_marked_contacts()?;
            return Ok(());
        }

        Ok(())
    }

    fn current_modal_selection(&self) -> Option<(MultiValueField, MultiValueItem)> {
        let modal = self.multivalue_modal.as_ref()?;
        let item = modal.selected_item()?.clone();
        Some((modal.field(), item))
    }

    fn close_multivalue_modal(&mut self) {
        self.multivalue_modal = None;
    }

    fn open_multivalue_modal_for_current_field(&mut self) -> bool {
        let Some(field) = self.focused_field() else {
            return false;
        };

        let Some(source) = field.source() else {
            return false;
        };

        let Some(kind) = MultiValueField::from_field_name(&source.field) else {
            return false;
        };

        let items = self.build_multivalue_items(kind);
        if items.len() < 2 {
            return false;
        }

        let selected = items
            .iter()
            .position(|item| item.seq == source.seq)
            .unwrap_or(0);
        self.multivalue_modal = Some(MultiValueModal::new(kind, items, selected));
        true
    }

    fn rebuild_multivalue_modal(&mut self, field: MultiValueField, selected_seq: Option<i64>) {
        let items = self.build_multivalue_items(field);
        if items.is_empty() {
            self.multivalue_modal = None;
            return;
        }

        let selected = selected_seq
            .and_then(|seq| items.iter().position(|item| item.seq == seq))
            .unwrap_or(0);
        self.multivalue_modal = Some(MultiValueModal::new(field, items, selected));
    }

    fn build_multivalue_items(&self, field: MultiValueField) -> Vec<MultiValueItem> {
        let default_region = self.config.phone_region.as_deref();
        let field_name = field.field_name();
        let mut items: Vec<MultiValueItem> = self
            .current_props
            .iter()
            .filter(|prop| prop.field.eq_ignore_ascii_case(field_name))
            .map(|prop| {
                let type_label =
                    extract_type_labels(&prop.params).unwrap_or_else(|| "—".to_string());
                let (value, copy_value) = match field {
                    MultiValueField::Email => {
                        let trimmed = prop.value.trim().to_string();
                        (trimmed.clone(), trimmed)
                    }
                    MultiValueField::Phone => {
                        let display = vcard_io::phone_display_value(&prop.value, default_region);
                        (display.clone(), display)
                    }
                };
                MultiValueItem {
                    seq: prop.seq,
                    value,
                    copy_value,
                    type_label,
                }
            })
            .collect();

        items.sort_by_key(|item| item.seq);
        items
    }

    fn refresh_contacts(&mut self) -> Result<()> {
        let previous_uuid = self
            .contacts
            .get(self.selected)
            .map(|entry| entry.uuid.clone());

        let normalized = search::normalize_query(self.search_input.value());
        self.contacts = if let Some(filter) = normalized.as_ref() {
            self.db.list_contacts(Some(filter))?
        } else {
            self.db.list_contacts(None)?
        };

        self.sort_contacts();

        if let Some(uuid) = previous_uuid {
            if let Some(index) = self.contacts.iter().position(|entry| entry.uuid == uuid) {
                self.selected = index;
            }
        }

        if self.contacts.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.contacts.len() {
            self.selected = self.contacts.len() - 1;
        }

        self.rebuild_search_rows();

        if self.contacts.is_empty() {
            self.current_contact = None;
            self.current_props.clear();
            self.aliases.clear();
            self.languages.clear();
            self.rebuild_field_views();
        } else {
            self.load_selection()?;
        }
        Ok(())
    }

    fn rebuild_search_rows(&mut self) {
        self.search_rows.clear();

        if self.contacts.is_empty() {
            self.selected_row = None;
            return;
        }

        let mut last_chain: Vec<String> = Vec::new();

        for (index, contact) in self.contacts.iter().enumerate() {
            let chain = self.address_book_chain(&contact.path);

            let mut shared_prefix = 0;
            while shared_prefix < chain.len()
                && shared_prefix < last_chain.len()
                && chain[shared_prefix] == last_chain[shared_prefix]
            {
                shared_prefix += 1;
            }

            for level in shared_prefix..chain.len() {
                let name = &chain[level];
                let text = format!("{}{}", self.config.ui.icons.address_book, name);
                self.search_rows.push(SearchRow {
                    text,
                    depth: level as u16,
                    contact_index: None,
                });
            }

            let depth = chain.len() as u16;
            last_chain = chain.clone();

            let text = if self.marked.contains(&contact.uuid) {
                format!("★ {}", contact.display_fn.to_uppercase())
            } else {
                let icon = if contact_is_org(contact) {
                    &self.config.ui.icons.organization
                } else {
                    &self.config.ui.icons.contact
                };
                format!("{}{}", icon, contact.display_fn.to_uppercase())
            };
            self.search_rows.push(SearchRow {
                text,
                depth,
                contact_index: Some(index),
            });
        }

        self.update_selected_row();
    }

    fn address_book_chain(&self, path: &Path) -> Vec<String> {
        address_book_chain_from(&self.config.vdir, path)
    }

    fn update_selected_row(&mut self) {
        if self.contacts.is_empty() || self.search_rows.is_empty() {
            self.selected_row = None;
            return;
        }

        if self.selected >= self.contacts.len() {
            self.selected = self.contacts.len() - 1;
        }

        if let Some((idx, _)) = self
            .search_rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.contact_index == Some(self.selected))
        {
            self.selected_row = Some(idx);
            return;
        }

        if let Some((idx, contact_index)) = self
            .search_rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| row.contact_index.map(|ci| (idx, ci)))
            .next()
        {
            self.selected = contact_index;
            self.selected_row = Some(idx);
        } else {
            self.selected_row = None;
        }
    }

    fn sort_contacts(&mut self) {
        self.contacts.sort_by_cached_key(|entry| {
            let book = address_book_chain_from(&self.config.vdir, &entry.path)
                .join("/")
                .to_ascii_lowercase();
            let name = entry.display_fn.to_ascii_lowercase();
            (book, name)
        });
    }

    fn move_selection(&mut self, delta: isize) -> Result<()> {
        if self.contacts.is_empty() {
            return Ok(());
        }
        let len = self.contacts.len() as isize;
        let mut index = self.selected as isize + delta;
        if index < 0 {
            index = 0;
        } else if index >= len {
            index = len - 1;
        }
        self.selected = index as usize;
        self.load_selection()?;
        Ok(())
    }

    fn load_selection(&mut self) -> Result<()> {
        if self.contacts.is_empty() {
            self.current_contact = None;
            self.current_props.clear();
            self.aliases.clear();
            self.languages.clear();
            self.photo_error = None;
            self.set_photo(None);
            self.rebuild_field_views();
            return Ok(());
        }
        self.update_selected_row();
        let contact = &self.contacts[self.selected];
        self.current_contact = self.db.get_contact(&contact.uuid)?;
        self.current_props = self.db.get_props(&contact.uuid)?;
        self.aliases = collect_aliases(&self.current_props, &contact.display_fn);
        self.languages = collect_languages(&self.current_props);
        match decode_embedded_photo(&self.current_props) {
            Ok(photo) => {
                self.photo_error = None;
                self.set_photo(photo);
            }
            Err(err) => {
                self.photo_error = Some(err.to_string());
                self.set_photo(None);
            }
        }
        self.rebuild_field_views();
        Ok(())
    }

    fn set_photo(&mut self, photo: Option<PhotoData>) {
        match photo {
            Some(photo) => {
                let protocol = self.image_picker.new_resize_protocol(photo.image().clone());
                self.image_state = Some(protocol);
                self.photo_data = Some(photo);
            }
            None => {
                self.image_state = None;
                self.photo_data = None;
            }
        }
    }

    fn rebuild_field_views(&mut self) {
        let default_region = self.config.phone_region.as_deref();

        if self.current_contact.is_some() {
            self.card_fields = build_card_fields(
                &self.current_props,
                &self.aliases,
                &self.config.fields_first_pane,
                default_region,
            );
        } else {
            self.card_fields.clear();
        }

        if self.card_fields.is_empty() {
            self.card_field_index = 0;
        } else if self.card_field_index >= self.card_fields.len() {
            self.card_field_index = 0;
        }

        for tab in DetailTab::ALL {
            let idx = tab.index();
            if self.current_contact.is_some() {
                self.tab_fields[idx] = build_tab_fields(tab, &self.current_props, default_region);
            } else {
                self.tab_fields[idx].clear();
            }

            if self.tab_fields[idx].is_empty() {
                self.tab_field_indices[idx] = 0;
            } else if self.tab_field_indices[idx] >= self.tab_fields[idx].len() {
                self.tab_field_indices[idx] = 0;
            }
        }
    }

    fn focus_pane(&mut self, pane: PaneFocus) {
        self.focused_pane = pane;
        if let PaneFocus::Detail(tab) = pane {
            self.tab = tab;
        }
        if !matches!(pane, PaneFocus::Search) {
            self.show_search = false;
        }
        self.ensure_focus_field(pane);
    }

    fn ensure_focus_field(&mut self, pane: PaneFocus) {
        match pane {
            PaneFocus::Card => {
                if self.card_fields.is_empty() {
                    self.card_field_index = 0;
                } else if self.card_field_index >= self.card_fields.len() {
                    self.card_field_index = 0;
                }
            }
            PaneFocus::Detail(tab) => {
                let idx = tab.index();
                if self.tab_fields[idx].is_empty() {
                    self.tab_field_indices[idx] = 0;
                } else if self.tab_field_indices[idx] >= self.tab_fields[idx].len() {
                    self.tab_field_indices[idx] = 0;
                }
            }
            PaneFocus::Search => {}
        }
    }

    fn advance_field(&mut self, delta: isize) {
        match self.focused_pane {
            PaneFocus::Card => {
                if self.card_fields.is_empty() {
                    return;
                }
                let len = self.card_fields.len() as isize;
                let current = self.card_field_index as isize;
                let next = (current + delta).rem_euclid(len);
                self.card_field_index = next as usize;
            }
            PaneFocus::Detail(tab) => {
                let idx = tab.index();
                let fields = &self.tab_fields[idx];
                if fields.is_empty() {
                    return;
                }
                let len = fields.len() as isize;
                let current = self.tab_field_indices[idx] as isize;
                let next = (current + delta).rem_euclid(len);
                self.tab_field_indices[idx] = next as usize;
            }
            PaneFocus::Search => {}
        }
    }

    fn focused_field(&self) -> Option<PaneField> {
        match self.focused_pane {
            PaneFocus::Card => self.card_fields.get(self.card_field_index).cloned(),
            PaneFocus::Detail(tab) => {
                let idx = tab.index();
                self.tab_fields[idx]
                    .get(self.tab_field_indices[idx])
                    .cloned()
            }
            PaneFocus::Search => None,
        }
    }

    fn copy_focused_value(&mut self) -> Result<()> {
        let Some(field) = self.focused_field() else {
            self.set_status("Nothing to copy");
            return Ok(());
        };

        self.copy_value_to_clipboard(field.copy_text())
    }

    fn copy_value_to_clipboard(&mut self, value: &str) -> Result<()> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.set_status("Nothing to copy");
            return Ok(());
        }

        if let Some(command) = self.config.commands.copy.clone() {
            match self.run_copy_command(&command, trimmed) {
                Ok(_) => self.set_status("Field copied!"),
                Err(err) => self.set_status(format!("Copy failed: {}", err)),
            }
        } else {
            self.set_status("Copy command not configured");
        }

        Ok(())
    }

    fn run_copy_command(&self, command: &CommandExec, value: &str) -> Result<()> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn `{}`", command.program))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(value.as_bytes())?;
        }

        let status = child.wait()?;
        if !status.success() {
            bail!("`{}` exited with {}", command.program, status);
        }

        Ok(())
    }

    fn set_status<S: Into<String>>(&mut self, message: S) {
        self.status = Some(message.into());
    }

    pub fn ui_colors(&self) -> &UiColors {
        &self.config.ui.colors
    }

    pub fn image_pane_width(&self) -> u16 {
        self.config.ui.pane.image.width
    }

    pub fn image_pane_height(&self) -> u16 {
        self.config.ui.pane.image.height
    }

    pub fn profile_image_state(&mut self) -> Option<&mut Box<dyn StatefulProtocol>> {
        self.image_state.as_mut()
    }

    pub fn image_font_size(&self) -> (u16, u16) {
        self.image_picker.font_size
    }

    pub fn photo_error(&self) -> Option<&str> {
        self.photo_error.as_deref()
    }

    pub fn multivalue_modal(&self) -> Option<&MultiValueModal> {
        self.multivalue_modal.as_ref()
    }

    pub fn contact_path_display(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.config.vdir).unwrap_or(path);

        let mut text = relative.to_string_lossy().to_string();
        if text.is_empty() {
            text = path.to_string_lossy().to_string();
        }

        if std::path::MAIN_SEPARATOR != '/' {
            text = text.replace(std::path::MAIN_SEPARATOR, "/");
        }

        text.to_uppercase()
    }

    fn begin_edit(&mut self) {
        if let Some(field) = self.focused_field() {
            if let Some(source) = field.source() {
                self.editor.start(field.copy_text(), source);
                self.set_status(format!("Editing {}", field.label));
            } else {
                self.set_status("Field not editable");
            }
        } else {
            self.set_status("Nothing to edit");
        }
    }

    fn add_alias_to_current_contact(&mut self, alias: &str) -> Result<()> {
        let Some(contact) = &self.current_contact else { return Ok(()); };
        let trimmed = alias.trim();
        if trimmed.is_empty() { return Ok(()); }

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref())?;
        let mut cards = parsed.cards;
        if cards.is_empty() { return Ok(()); }

        {
            let card = cards.get_mut(0).unwrap();
            let exists = card
                .nickname
                .iter()
                .any(|p| p.value.eq_ignore_ascii_case(trimmed));
            if !exists {
                card.nickname.push(TextProperty {
                    group: None,
                    value: trimmed.to_string(),
                    parameters: None,
                });
            }
        }

        vcard_io::write_cards(&contact.path, &cards)?;

        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh and keep focus stable
        let previous_index = self.card_field_index;
        self.refresh_contacts()?;
        if !self.card_fields.is_empty() {
            let max_index = self.card_fields.len().saturating_sub(1);
            self.card_field_index = previous_index.min(max_index);
        }

        Ok(())
    }

    fn commit_field_edit(&mut self, target: FieldRef, new_value: String) -> Result<()> {
        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref())?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(());
        }

        let updated = {
            let card = cards.get_mut(0).unwrap();
            vcard_io::update_card_field(
                card,
                &target.field,
                target.seq,
                target.component,
                &new_value,
                self.config.phone_region.as_deref(),
            )?
        };

        if !updated {
            self.set_status("Field not editable");
            return Ok(());
        }

        vcard_io::write_cards(&contact.path, &cards)?;

        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        let previous_index = self.card_field_index;
        self.refresh_contacts()?;
        if !self.card_fields.is_empty() {
            let max_index = self.card_fields.len().saturating_sub(1);
            self.card_field_index = previous_index.min(max_index);
        }

        Ok(())
    }

    fn set_multivalue_default(&mut self, field: MultiValueField, seq: i64) -> Result<bool> {
        if seq < 0 {
            self.set_status("Unable to set default");
            return Ok(false);
        }

        let idx = match usize::try_from(seq) {
            Ok(value) => value,
            Err(_) => {
                self.set_status("Unable to set default");
                return Ok(false);
            }
        };

        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(false);
        };

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref())?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(false);
        }

        let updated = {
            let card = cards.get_mut(0).unwrap();
            match field {
                MultiValueField::Email => vcard_io::promote_email_entry(card, idx),
                MultiValueField::Phone => vcard_io::promote_tel_entry(card, idx),
            }
        };

        if !updated {
            self.set_status("Unable to set default");
            return Ok(false);
        }

        vcard_io::write_cards(&contact.path, &cards)?;

        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        let previous_index = self.card_field_index;
        self.refresh_contacts()?;
        if !self.card_fields.is_empty() {
            let max_index = self.card_fields.len().saturating_sub(1);
            self.card_field_index = previous_index.min(max_index);
        }

        Ok(true)
    }

    /// Check if the key event matches any of the bindings in the list
    fn key_matches_any(&self, event: &KeyEvent, bindings: &[String]) -> bool {
        bindings.iter().any(|b| self.key_matches_single(event, b))
    }

    /// Check if the key event matches a single binding string
    fn key_matches_single(&self, event: &KeyEvent, binding: &str) -> bool {
        let trimmed = binding.trim();
        if trimmed.is_empty() {
            return false;
        }

        // Disallow Ctrl/Alt/Super modifiers (we don't support them)
        let disallowed = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
        if event.modifiers.intersects(disallowed) {
            return false;
        }

        match trimmed.to_ascii_lowercase().as_str() {
            // Special keys
            "enter" => matches!(event.code, KeyCode::Enter),
            "tab" => matches!(event.code, KeyCode::Tab),
            "backtab" | "shift+tab" => matches!(event.code, KeyCode::BackTab),
            "backspace" => matches!(event.code, KeyCode::Backspace),
            "esc" | "escape" => matches!(event.code, KeyCode::Esc),
            "space" => matches!(event.code, KeyCode::Char(' ')),
            // Arrow keys
            "up" => matches!(event.code, KeyCode::Up),
            "down" => matches!(event.code, KeyCode::Down),
            "left" => matches!(event.code, KeyCode::Left),
            "right" => matches!(event.code, KeyCode::Right),
            // Page navigation
            "pageup" | "page_up" => matches!(event.code, KeyCode::PageUp),
            "pagedown" | "page_down" => matches!(event.code, KeyCode::PageDown),
            "home" => matches!(event.code, KeyCode::Home),
            "end" => matches!(event.code, KeyCode::End),
            // Function keys
            "f1" => matches!(event.code, KeyCode::F(1)),
            "f2" => matches!(event.code, KeyCode::F(2)),
            "f3" => matches!(event.code, KeyCode::F(3)),
            "f4" => matches!(event.code, KeyCode::F(4)),
            "f5" => matches!(event.code, KeyCode::F(5)),
            "f6" => matches!(event.code, KeyCode::F(6)),
            "f7" => matches!(event.code, KeyCode::F(7)),
            "f8" => matches!(event.code, KeyCode::F(8)),
            "f9" => matches!(event.code, KeyCode::F(9)),
            "f10" => matches!(event.code, KeyCode::F(10)),
            "f11" => matches!(event.code, KeyCode::F(11)),
            "f12" => matches!(event.code, KeyCode::F(12)),
            // Single character - case-sensitive (m != M, since M requires Shift)
            _ => {
                let mut chars = trimmed.chars();
                if let (Some(first), None) = (chars.next(), chars.next()) {
                    matches!(event.code, KeyCode::Char(c) if c == first)
                } else {
                    false
                }
            }
        }
    }

    fn focus_by_digit(&mut self, digit: char) -> bool {
        match digit {
            '1' => {
                self.focus_pane(PaneFocus::Card);
                true
            }
            c => {
                if let Some(tab) = DetailTab::from_digit(c) {
                    self.focus_pane(PaneFocus::Detail(tab));
                    true
                } else {
                    false
                }
            }
        }
    }

    // =========================================================================
    // Help Modal
    // =========================================================================

    /// Generate help content from current keybindings configuration
    pub fn help_entries(&self) -> Vec<HelpSection> {
        let keys = &self.config.keys;

        vec![
            HelpSection {
                title: "Global",
                entries: vec![
                    HelpEntry {
                        action: "Quit",
                        keys: keys.global.quit.join(", "),
                    },
                    HelpEntry {
                        action: "Search",
                        keys: keys.global.search.join(", "),
                    },
                    HelpEntry {
                        action: "Help",
                        keys: keys.global.help.join(", "),
                    },
                ],
            },
            HelpSection {
                title: "Search Input",
                entries: vec![
                    HelpEntry {
                        action: "Cancel",
                        keys: keys.search_input.cancel.join(", "),
                    },
                    HelpEntry {
                        action: "Confirm",
                        keys: keys.search_input.confirm.join(", "),
                    },
                    HelpEntry {
                        action: "Next",
                        keys: keys.search_input.next.join(", "),
                    },
                    HelpEntry {
                        action: "Previous",
                        keys: keys.search_input.prev.join(", "),
                    },
                ],
            },
            HelpSection {
                title: "Search Results",
                entries: vec![
                    HelpEntry {
                        action: "Cancel",
                        keys: keys.search_results.cancel.join(", "),
                    },
                    HelpEntry {
                        action: "Confirm",
                        keys: keys.search_results.confirm.join(", "),
                    },
                    HelpEntry {
                        action: "Next",
                        keys: keys.search_results.next.join(", "),
                    },
                    HelpEntry {
                        action: "Previous",
                        keys: keys.search_results.prev.join(", "),
                    },
                    HelpEntry {
                        action: "Page Down",
                        keys: keys.search_results.page_down.join(", "),
                    },
                    HelpEntry {
                        action: "Page Up",
                        keys: keys.search_results.page_up.join(", "),
                    },
                    HelpEntry {
                        action: "Mark",
                        keys: keys.search_results.mark.join(", "),
                    },
                    HelpEntry {
                        action: "Merge",
                        keys: keys.search_results.merge.join(", "),
                    },
                    HelpEntry {
                        action: "Toggle Marked",
                        keys: keys.search_results.toggle_marked.join(", "),
                    },
                ],
            },
            HelpSection {
                title: "Navigation",
                entries: vec![
                    HelpEntry {
                        action: "Next",
                        keys: keys.navigation.next.join(", "),
                    },
                    HelpEntry {
                        action: "Previous",
                        keys: keys.navigation.prev.join(", "),
                    },
                    HelpEntry {
                        action: "Tab Next",
                        keys: keys.navigation.tab_next.join(", "),
                    },
                    HelpEntry {
                        action: "Tab Previous",
                        keys: keys.navigation.tab_prev.join(", "),
                    },
                    HelpEntry {
                        action: "Edit",
                        keys: keys.navigation.edit.join(", "),
                    },
                    HelpEntry {
                        action: "Copy",
                        keys: keys.navigation.copy.join(", "),
                    },
                    HelpEntry {
                        action: "Confirm",
                        keys: keys.navigation.confirm.join(", "),
                    },
                    HelpEntry {
                        action: "Add Alias",
                        keys: keys.navigation.add_alias.join(", "),
                    },
                    HelpEntry {
                        action: "Fetch Photo",
                        keys: keys.navigation.photo_fetch.join(", "),
                    },
                    HelpEntry {
                        action: "Cycle Language",
                        keys: keys.navigation.lang_cycle.join(", "),
                    },
                ],
            },
            HelpSection {
                title: "Modal",
                entries: vec![
                    HelpEntry {
                        action: "Cancel",
                        keys: keys.modal.cancel.join(", "),
                    },
                    HelpEntry {
                        action: "Confirm",
                        keys: keys.modal.confirm.join(", "),
                    },
                    HelpEntry {
                        action: "Next",
                        keys: keys.modal.next.join(", "),
                    },
                    HelpEntry {
                        action: "Previous",
                        keys: keys.modal.prev.join(", "),
                    },
                    HelpEntry {
                        action: "Edit",
                        keys: keys.modal.edit.join(", "),
                    },
                    HelpEntry {
                        action: "Copy",
                        keys: keys.modal.copy.join(", "),
                    },
                    HelpEntry {
                        action: "Set Default",
                        keys: keys.modal.set_default.join(", "),
                    },
                ],
            },
            HelpSection {
                title: "Editor",
                entries: vec![
                    HelpEntry {
                        action: "Cancel",
                        keys: keys.editor.cancel.join(", "),
                    },
                    HelpEntry {
                        action: "Confirm",
                        keys: keys.editor.confirm.join(", "),
                    },
                ],
            },
        ]
    }

    /// Calculate total number of lines in help content
    fn help_total_lines(&self) -> usize {
        let sections = self.help_entries();
        let mut total = 0;
        for section in &sections {
            total += 1; // Section header
            total += section.entries.len(); // Entries
            total += 1; // Blank line after section
        }
        total
    }

    /// Open the help modal
    pub fn show_help(&mut self) {
        let total_lines = self.help_total_lines();
        self.help_modal = Some(HelpModal::new(total_lines));
    }

    /// Handle keys when help modal is open
    fn handle_help_modal_key(&mut self, key: KeyEvent) {
        // Close on Escape or q
        if matches!(key.code, KeyCode::Esc) || matches!(key.code, KeyCode::Char('q')) {
            self.help_modal = None;
            return;
        }

        let Some(modal) = self.help_modal.as_mut() else {
            return;
        };

        match key.code {
            // Scroll down
            KeyCode::Char('j') | KeyCode::Down => {
                modal.scroll_down(1);
            }
            // Scroll up
            KeyCode::Char('k') | KeyCode::Up => {
                modal.scroll_up(1);
            }
            // Page down
            KeyCode::PageDown => {
                let page = modal.viewport_height.saturating_sub(1).max(1);
                modal.scroll_down(page);
            }
            // Page up
            KeyCode::PageUp => {
                let page = modal.viewport_height.saturating_sub(1).max(1);
                modal.scroll_up(page);
            }
            // Scroll to top
            KeyCode::Char('g') | KeyCode::Home => {
                modal.scroll_to_top();
            }
            // Scroll to bottom
            KeyCode::Char('G') | KeyCode::End => {
                modal.scroll_to_bottom();
            }
            _ => {}
        }
    }
}

fn contact_is_org(entry: &ContactListEntry) -> bool {
    if let Some(kind) = entry.kind.as_deref() {
        if kind.eq_ignore_ascii_case("org") || kind.eq_ignore_ascii_case("organization") {
            return true;
        }
    }
    entry.primary_org.is_some()
}

fn merge_two_cards(mut a: Vcard, b: Vcard) -> Vcard {
    // FN: keep a.FN; add b.FN[0] to NICKNAME if not present
    let a_fn = a
        .formatted_name
        .first()
        .map(|p| p.value.clone())
        .unwrap_or_else(|| "".to_string());
    if let Some(b_fn) = b.formatted_name.first() {
        let b_name = b_fn.value.trim();
        if !b_name.is_empty()
            && !eq_ignore_ascii_case_any(b_name, std::iter::once(a_fn.as_str()).chain(
                a.nickname.iter().map(|p| p.value.as_str())
            ))
        {
            a.nickname.push(TextProperty {
                group: None,
                value: b_name.to_string(),
                parameters: None,
            });
        }
    }

    // N*: prefer a.name; ignore b.name

    // NICKNAME: merge uniques from b
    for nick in b.nickname.iter() {
        let val = nick.value.trim();
        if !val.is_empty()
            && !eq_ignore_ascii_case_any(val, a.nickname.iter().map(|p| p.value.as_str()))
        {
            a.nickname.push(nick.clone());
        }
    }

    // Append remaining list properties
    a.photo.extend(b.photo.clone());
    if b.bday.is_some() && a.bday.is_none() {
        a.bday = b.bday.clone();
    }
    if b.anniversary.is_some() && a.anniversary.is_none() {
        a.anniversary = b.anniversary.clone();
    }
    if b.gender.is_some() && a.gender.is_none() {
        a.gender = b.gender.clone();
    }
    a.url.extend(b.url.clone());
    a.address.extend(b.address.clone());
    a.tel.extend(b.tel.clone());
    a.email.extend(b.email.clone());
    a.impp.extend(b.impp.clone());
    a.lang.extend(b.lang.clone());
    a.title.extend(b.title.clone());
    a.role.extend(b.role.clone());
    a.logo.extend(b.logo.clone());
    a.org.extend(b.org.clone());
    a.member.extend(b.member.clone());
    a.related.extend(b.related.clone());
    a.timezone.extend(b.timezone.clone());
    a.geo.extend(b.geo.clone());
    a.categories.extend(b.categories.clone());
    a.note.extend(b.note.clone());
    if a.prod_id.is_none() {
        a.prod_id = b.prod_id.clone();
    }
    a.sound.extend(b.sound.clone());
    a.key.extend(b.key.clone());
    a.fburl.extend(b.fburl.clone());
    a.cal_adr_uri.extend(b.cal_adr_uri.clone());
    a.cal_uri.extend(b.cal_uri.clone());
    a.extensions.extend(b.extensions.clone());

    a
}

fn eq_ignore_ascii_case_any<'a, I>(needle: &str, hay: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    for item in hay {
        if needle.eq_ignore_ascii_case(item) {
            return true;
        }
    }
    false
}

fn address_book_chain_from(vdir: &Path, path: &Path) -> Vec<String> {
    let relative = path.strip_prefix(vdir).unwrap_or(path);
    let mut components: Vec<String> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(os_str) => Some(os_str.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();

    if !components.is_empty() {
        components.pop();
    }

    if components.is_empty() {
        vec![DEFAULT_ADDRESS_BOOK.to_string()]
    } else {
        components
    }
}

fn collect_aliases(props: &[PropRow], display_fn: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for prop in props
        .iter()
        .filter(|p| p.field == "NICKNAME" || p.field == "FN")
    {
        if prop.value != display_fn && !prop.value.is_empty() {
            aliases.push(prop.value.clone());
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn collect_languages(props: &[PropRow]) -> Vec<String> {
    let mut langs = Vec::new();
    for prop in props.iter().filter(|p| p.field == "FN") {
        if let Some(language) = prop.params.get("language").and_then(Value::as_str) {
            if !language.is_empty() {
                langs.push(language.to_string());
            }
        }
    }
    langs.sort();
    langs.dedup();
    langs
}

#[derive(Debug, Clone)]
pub struct PhotoData {
    image: DynamicImage,
}

impl PhotoData {
    pub fn image(&self) -> &DynamicImage {
        &self.image
    }
}

fn decode_embedded_photo(props: &[PropRow]) -> Result<Option<PhotoData>> {
    for prop in props.iter().filter(|p| p.field == "PHOTO") {
        match decode_photo_prop(prop) {
            Ok(Some(photo)) => return Ok(Some(photo)),
            Ok(None) => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

fn decode_photo_prop(prop: &PropRow) -> Result<Option<PhotoData>> {
    if prop
        .params
        .get("value")
        .and_then(Value::as_str)
        .map(|value| value.eq_ignore_ascii_case("uri"))
        .unwrap_or(false)
    {
        return Ok(None);
    }

    if prop.value.trim().is_empty() {
        return Ok(None);
    }

    if prop
        .value
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("http://")
        || prop
            .value
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("https://")
    {
        return Ok(None);
    }

    let data = if let Some(data_uri) = prop.value.trim().strip_prefix("data:") {
        parse_data_uri(data_uri)?
    } else {
        decode_base64_blob(&prop.value)?
    };

    if data.is_empty() {
        return Ok(None);
    }

    let image = image::load_from_memory(&data)
        .with_context(|| "unable to parse embedded photo data as image")?;

    Ok(Some(PhotoData { image }))
}

fn parse_data_uri(input: &str) -> Result<Vec<u8>> {
    let mut parts = input.splitn(2, ',');
    let meta = parts
        .next()
        .ok_or_else(|| anyhow!("invalid data URI in PHOTO value"))?;
    let data = parts
        .next()
        .ok_or_else(|| anyhow!("data URI is missing payload"))?;

    let mut is_base64 = false;

    for segment in meta.split(';') {
        if segment.is_empty() {
            continue;
        }
        if segment.eq_ignore_ascii_case("base64") {
            is_base64 = true;
        }
    }

    if !is_base64 {
        bail!("embedded data URI is not base64 encoded");
    }

    let decoded = BASE64_STANDARD
        .decode(data.trim())
        .with_context(|| "failed to decode base64 data URI contents")?;
    Ok(decoded)
}

fn decode_base64_blob(value: &str) -> Result<Vec<u8>> {
    let filtered: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    if filtered.is_empty() {
        return Ok(Vec::new());
    }
    let decoded = BASE64_STANDARD
        .decode(filtered)
        .with_context(|| "failed to decode embedded PHOTO data as base64")?;
    Ok(decoded)
}

const DEFAULT_CARD_FIELDS: &[&str] = &["fname", "mname", "lname", "alias", "phone", "email"];

fn build_card_fields(
    props: &[PropRow],
    aliases: &[String],
    order: &[String],
    default_region: Option<&str>,
) -> Vec<PaneField> {
    let fields = build_card_fields_inner(
        props,
        aliases,
        order.iter().map(|s| s.as_str()),
        default_region,
    );
    if fields.is_empty() {
        build_card_fields_inner(
            props,
            aliases,
            DEFAULT_CARD_FIELDS.iter().copied(),
            default_region,
        )
    } else {
        fields
    }
}

fn build_card_fields_inner<I, S>(
    props: &[PropRow],
    aliases: &[String],
    order: I,
    default_region: Option<&str>,
) -> Vec<PaneField>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut fields = Vec::new();
    let is_org = props_is_org(props);

    let alias_value = if aliases.is_empty() {
        "—".to_string()
    } else {
        aliases.join("/")
    };

    let first_phone = props.iter().find(|p| p.field == "TEL");
    let total_phone_count = props.iter().filter(|p| p.field == "TEL").count();
    let first_email = props.iter().find(|p| p.field == "EMAIL");
    let total_email_count = props.iter().filter(|p| p.field == "EMAIL").count();

    let mut handled_name = false;

    for item in order {
        let key = item.as_ref().trim().to_ascii_lowercase();
        match key.as_str() {
            "fname" => {
                if let Some(prop) = props.iter().find(|p| p.field == "FN") {
                    let value = prop.value.trim().to_string();
                    fields.push(PaneField::from_prop(
                        "FNAME",
                        value.clone(),
                        value,
                        "FN",
                        prop.seq,
                        None,
                    ));
                }
                if !handled_name && !is_org {
                    handled_name = true;
                    if let Some(prop) = props.iter().find(|p| p.field == "N") {
                        for (idx, component) in name_components(&prop.value).into_iter().enumerate()
                        {
                            let display_component = if component.is_empty() {
                                "—".to_string()
                            } else {
                                component.clone()
                            };
                            let copy_component = component.clone();
                            let label = match idx {
                                0 => "NAME_FAMILY",
                                1 => "NAME_GIVEN",
                                2 => "NAME_ADDITIONAL",
                                3 => "NAME_PREFIX",
                                4 => "NAME_SUFFIX",
                                _ => "NAME",
                            };
                            fields.push(PaneField::from_prop(
                                label,
                                display_component,
                                copy_component,
                                "N",
                                prop.seq,
                                Some(idx),
                            ));
                        }
                    }
                }
            }
            "mname" | "lname" => {
                if !handled_name && !is_org {
                    handled_name = true;
                    if let Some(prop) = props.iter().find(|p| p.field == "N") {
                        for (idx, component) in name_components(&prop.value).into_iter().enumerate()
                        {
                            let display_component = if component.is_empty() {
                                "—".to_string()
                            } else {
                                component.clone()
                            };
                            let copy_component = component.clone();
                            let label = match idx {
                                0 => "NAME_FAMILY",
                                1 => "NAME_GIVEN",
                                2 => "NAME_ADDITIONAL",
                                3 => "NAME_PREFIX",
                                4 => "NAME_SUFFIX",
                                _ => "NAME",
                            };
                            fields.push(PaneField::from_prop(
                                label,
                                display_component,
                                copy_component,
                                "N",
                                prop.seq,
                                Some(idx),
                            ));
                        }
                    }
                }
            }
            "alias" => fields.push(PaneField::new("ALIAS", alias_value.clone())),
            "phone" => {
                if let Some(prop) = first_phone {
                    let label = "PHONE".to_string();
                    let base_value = vcard_io::phone_display_value(&prop.value, default_region);
                    if !base_value.is_empty() {
                        let display_value = if total_phone_count > 1 {
                            format!("{} [{}]", base_value, total_phone_count)
                        } else {
                            base_value.clone()
                        };
                        fields.push(PaneField::from_prop(
                            label,
                            display_value,
                            base_value,
                            "TEL",
                            prop.seq,
                            None,
                        ));
                    }
                }
            }
            "email" => {
                if let Some(prop) = first_email {
                    let label = "EMAIL".to_string();
                    let copy_text = prop.value.trim().to_string();
                    let display_value = if total_email_count > 1 && !copy_text.is_empty() {
                        format!("{} [{}]", copy_text, total_email_count)
                    } else {
                        copy_text.clone()
                    };
                    fields.push(PaneField::from_prop(
                        label,
                        display_value,
                        copy_text,
                        "EMAIL",
                        prop.seq,
                        None,
                    ));
                }
            }
            _ => {}
        }
    }

    fields
}

fn props_is_org(props: &[PropRow]) -> bool {
    // KIND:org or presence of ORG field implies organization
    if props.iter().any(|p| p.field.eq_ignore_ascii_case("ORG")) {
        return true;
    }
    props.iter().any(|p| p.field.eq_ignore_ascii_case("KIND")
        && (p.value.eq_ignore_ascii_case("org") || p.value.eq_ignore_ascii_case("organization")))
}

fn build_tab_fields(
    tab: DetailTab,
    props: &[PropRow],
    default_region: Option<&str>,
) -> Vec<PaneField> {
    match tab {
        DetailTab::Work => build_work_fields(props, default_region),
        DetailTab::Personal => build_personal_fields(props, default_region),
        DetailTab::Accounts => build_account_fields(props),
        DetailTab::Metadata => build_metadata_fields(props),
    }
}

fn build_work_fields(props: &[PropRow], default_region: Option<&str>) -> Vec<PaneField> {
    let mut fields = Vec::new();

    for prop in props.iter().filter(|p| p.field == "ORG") {
        fields.push(PaneField::new("ORG", prop.value.clone()));
    }
    for prop in props.iter().filter(|p| p.field == "TITLE") {
        fields.push(PaneField::new("TITLE", prop.value.clone()));
    }
    for prop in props.iter().filter(|p| p.field == "ROLE") {
        fields.push(PaneField::new("ROLE", prop.value.clone()));
    }

    for prop in props.iter().filter(|p| p.field == "ADR") {
        if prop_has_type(&prop.params, "work") {
            let label = format_label_with_type("ADDRESS", &prop.params);
            fields.push(PaneField::new(label, format_address_value(prop)));
        }
    }

    for prop in props.iter().filter(|p| p.field == "EMAIL") {
        if prop_has_type(&prop.params, "work") {
            let label = format_label_with_type("EMAIL", &prop.params);
            let base = prop.value.trim().to_string();
            let display = format_with_index(&base, prop.seq);
            fields.push(PaneField::from_prop(
                label, display, base, "EMAIL", prop.seq, None,
            ));
        }
    }

    for prop in props.iter().filter(|p| p.field == "TEL") {
        if prop_has_type(&prop.params, "work") {
            let label = format_label_with_type("PHONE", &prop.params);
            let base = vcard_io::phone_display_value(&prop.value, default_region);
            let display = format_with_index(&base, prop.seq);
            fields.push(PaneField::from_prop(
                label, display, base, "TEL", prop.seq, None,
            ));
        }
    }

    fields
}

fn build_personal_fields(props: &[PropRow], default_region: Option<&str>) -> Vec<PaneField> {
    let mut fields = Vec::new();

    for prop in props.iter().filter(|p| p.field == "BDAY") {
        fields.push(PaneField::new("BDAY", prop.value.clone()));
    }
    for prop in props.iter().filter(|p| p.field == "ANNIVERSARY") {
        fields.push(PaneField::new("ANNIVERSARY", prop.value.clone()));
    }

    for prop in props.iter().filter(|p| p.field == "ADR") {
        if prop_has_type(&prop.params, "home") {
            let label = format_label_with_type("ADDRESS", &prop.params);
            fields.push(PaneField::new(label, format_address_value(prop)));
        }
    }

    for prop in props.iter().filter(|p| p.field == "EMAIL") {
        if prop_has_type(&prop.params, "home") {
            let label = format_label_with_type("EMAIL", &prop.params);
            let base = prop.value.trim().to_string();
            let display = format_with_index(&base, prop.seq);
            fields.push(PaneField::from_prop(
                label, display, base, "EMAIL", prop.seq, None,
            ));
        }
    }

    for prop in props.iter().filter(|p| p.field == "TEL") {
        if prop_has_type(&prop.params, "home") {
            let label = format_label_with_type("PHONE", &prop.params);
            let base = vcard_io::phone_display_value(&prop.value, default_region);
            let display = format_with_index(&base, prop.seq);
            fields.push(PaneField::from_prop(
                label, display, base, "TEL", prop.seq, None,
            ));
        }
    }

    fields
}

fn build_account_fields(props: &[PropRow]) -> Vec<PaneField> {
    let mut fields = Vec::new();

    for prop in props.iter().filter(|p| p.field == "IMPP") {
        let label = format_label_with_type("IMPP", &prop.params);
        fields.push(PaneField::new(label, prop.value.clone()));
    }
    for prop in props.iter().filter(|p| p.field == "URL") {
        let label = format_label_with_type("URL", &prop.params);
        fields.push(PaneField::new(label, prop.value.clone()));
    }
    for prop in props.iter().filter(|p| p.field == "RELATED") {
        let label = format_label_with_type("RELATED", &prop.params);
        fields.push(PaneField::new(label, prop.value.clone()));
    }

    // Include recognizable social handles (X-*)
    for prop in props.iter().filter(|p| {
        p.field.starts_with("X-") && (p.field.contains("SOCIAL") || p.field.contains("IM"))
    }) {
        let label = prop.field.to_uppercase();
        fields.push(PaneField::new(label, prop.value.clone()));
    }

    fields
}

fn build_metadata_fields(props: &[PropRow]) -> Vec<PaneField> {
    let mut fields = Vec::new();
    for prop in props {
        let value = if params_is_empty(&prop.params) {
            prop.value.clone()
        } else {
            format!("{} ({})", prop.value, format_params(&prop.params))
        };
        fields.push(PaneField::new(prop.field.clone(), value));
    }
    fields
}

fn params_is_empty(params: &Value) -> bool {
    matches!(params, Value::Object(map) if map.is_empty())
}

fn format_params(params: &Value) -> String {
    serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string())
}

fn format_label_with_type(base: &str, params: &Value) -> String {
    let upper = base.to_uppercase();
    if let Some(types) = extract_type_labels(params) {
        format!("{} ({})", upper, types)
    } else {
        upper
    }
}

fn extract_type_labels(params: &Value) -> Option<String> {
    let entry = params.get("type")?;
    match entry {
        Value::String(s) if !s.is_empty() => Some(s.to_uppercase()),
        Value::Array(items) => {
            let mut labels = Vec::new();
            for item in items {
                if let Some(text) = item.as_str() {
                    labels.push(text.to_uppercase());
                }
            }
            if labels.is_empty() {
                None
            } else {
                Some(labels.join("/"))
            }
        }
        _ => None,
    }
}

fn prop_has_type(params: &Value, expected: &str) -> bool {
    let expected_lower = expected.to_ascii_lowercase();
    match params.get("type") {
        Some(Value::String(s)) => s.eq_ignore_ascii_case(&expected_lower),
        Some(Value::Array(items)) => items.iter().any(|item| {
            item.as_str()
                .map(|text| text.eq_ignore_ascii_case(&expected_lower))
                .unwrap_or(false)
        }),
        _ => false,
    }
}

fn format_with_index(value: &str, seq: i64) -> String {
    format!("{} [{}]", value, seq + 1)
}

fn format_address_value(prop: &PropRow) -> String {
    if let Some(label) = prop.params.get("label").and_then(Value::as_str) {
        label.to_string()
    } else {
        let parts: Vec<_> = prop
            .value
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            prop.value.clone()
        } else {
            parts.join("; ")
        }
    }
}

fn name_components(value: &str) -> Vec<String> {
    let mut components: Vec<String> = value
        .split(';')
        .map(|part| part.trim().to_string())
        .collect();
    let max_parts = 5;
    components.resize(max_parts, String::new());
    if components.is_empty() {
        components.push("".to_string());
    }
    components
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFocus {
    Input,
    Results,
}
