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

use crate::config::{CommandExec, Config, DetailsSectionsConfig, TopBarAction, UiColors};
use crate::crypto::CryptoProvider;
use crate::db::{ContactItem, ContactListEntry, Database, PropRow};
use crate::indexer;
use crate::search;
use crate::vcard_io;
use crate::vdir;
use vcard4::property::TextProperty;

use image::{self, DynamicImage};

use super::draw;
use super::edit::{FieldRef, InlineEditor};

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
    Details,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiValueField {
    Email,
    Phone,
    Alias,
}

impl MultiValueField {
    pub fn from_field_name(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "EMAIL" => Some(Self::Email),
            "TEL" => Some(Self::Phone),
            "NICKNAME" => Some(Self::Alias),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Email => "EMAIL ADDRESSES",
            Self::Phone => "PHONE NUMBERS",
            Self::Alias => "ALIASES",
        }
    }

    fn field_name(self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
            Self::Phone => "TEL",
            Self::Alias => "NICKNAME",
        }
    }

    /// Whether this field type supports a "type" label (e.g., "work", "home")
    pub fn has_type_label(self) -> bool {
        match self {
            Self::Email | Self::Phone => true,
            Self::Alias => false,
        }
    }

    /// Whether this field type supports "set default" operation
    pub fn has_default(self) -> bool {
        match self {
            Self::Email | Self::Phone => true,
            Self::Alias => false,
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
    pub action: ConfirmAction,
}

/// Action to perform when confirm modal is accepted
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Merge marked contacts
    MergeContacts,
    /// Delete the current contact
    DeleteContact,
    /// Delete a specific field (field name, seq)
    DeleteField { field: String, seq: i64 },
    /// Delete the contact photo
    DeletePhoto,
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

/// Reindex modal (blocking during reindex operation)
#[derive(Debug, Clone)]
pub struct ReindexModal {
    pub message: String,
}

/// Share modal with QR code
#[derive(Debug, Clone)]
pub struct ShareModal {
    /// QR code rendered as lines of Unicode characters
    pub qr_lines: Vec<String>,
}

/// Photo path input modal
#[derive(Debug, Clone)]
pub struct PhotoPathModal {
    pub input: Input,
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

/// A field in the details pane with property columns
#[derive(Debug, Clone)]
pub struct DetailsField {
    pub label: String,
    pub value: String,
    pub copy_value: String,
    /// Property parameters as name -> list of values (e.g., "TYPE" -> ["CELL", "WORK"])
    /// Excludes PREF which is handled separately in multivalue fields
    pub params: std::collections::HashMap<String, Vec<String>>,
    pub source: Option<FieldRef>,
}

impl DetailsField {
    /// Convert to PaneField for compatibility with existing code
    pub fn to_pane_field(&self) -> PaneField {
        PaneField {
            label: self.label.clone(),
            value: self.value.clone(),
            copy_value: self.copy_value.clone(),
            source: self.source.clone(),
        }
    }
}

/// A section in the details pane with its fields
#[derive(Debug, Clone)]
pub struct DetailsSection {
    pub name: String,
    pub fields: Vec<DetailsField>,
}

pub struct App<'a> {
    db: &'a mut Database,
    config: &'a Config,
    provider: &'a dyn CryptoProvider,
    pub contacts: Vec<ContactListEntry>,
    pub selected: usize,
    pub search_input: Input,
    pub show_search: bool,
    pub search_focus: SearchFocus,
    pub current_contact: Option<ContactItem>,
    pub current_props: Vec<PropRow>,
    pub editor: InlineEditor,
    pub status: Option<String>,
    pub aliases: Vec<String>,
    pub languages: Vec<String>,
    pub focused_pane: PaneFocus,
    // Card pane (panel 1)
    pub card_fields: Vec<PaneField>,
    pub card_field_index: usize,
    // Details pane (panel 2) - sections with fields
    pub details_sections: Vec<DetailsSection>,
    pub details_field_index: usize,
    pub details_scroll: usize,
    // Search results
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
    // Reindex modal (blocking)
    pub reindex_modal: Option<ReindexModal>,
    // Share modal with QR code
    pub share_modal: Option<ShareModal>,
    // Add field modal (multi-step wizard)
    pub add_field_modal: Option<AddFieldModal>,
    // Photo path input modal
    pub photo_path_modal: Option<PhotoPathModal>,
    // Flag to trigger reindex from event loop
    pub pending_reindex: bool,
}

/// Add field modal - multi-step wizard for adding new fields
#[derive(Debug, Clone)]
pub struct AddFieldModal {
    pub state: AddFieldState,
    pub property_index: usize,
    pub type_index: Option<usize>,
    pub value_input: Input,
    pub custom_property_input: Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddFieldState {
    SelectProperty,
    SelectType,
    EnterValue,
    EnterCustomProperty,
}

/// Standard vCard properties that can be added
pub const STANDARD_PROPERTIES: &[(&str, &str, bool)] = &[
    // (display name, vcard field, supports_type)
    ("Phone", "TEL", true),
    ("Email", "EMAIL", true),
    ("Address", "ADR", true),
    ("URL", "URL", true),
    ("Note", "NOTE", false),
    ("Birthday", "BDAY", false),
    ("Organization", "ORG", false),
    ("Title", "TITLE", false),
    ("Role", "ROLE", false),
    ("Nickname", "NICKNAME", false),
    ("IMPP", "IMPP", true),
    ("Custom (X-...)", "X-", true),
];

/// Standard TYPE values for fields that support them
pub const TYPE_VALUES: &[&str] = &[
    "work", "home", "cell", "voice", "fax", "pager", "text", "video",
];

impl AddFieldModal {
    pub fn new() -> Self {
        Self {
            state: AddFieldState::SelectProperty,
            property_index: 0,
            type_index: None,
            value_input: Input::default(),
            custom_property_input: Input::default(),
        }
    }

    pub fn current_property(&self) -> Option<(&str, &str, bool)> {
        STANDARD_PROPERTIES.get(self.property_index).copied()
    }

    pub fn current_type(&self) -> Option<&str> {
        self.type_index.and_then(|i| TYPE_VALUES.get(i).copied())
    }

    pub fn select_next_property(&mut self) {
        self.property_index = (self.property_index + 1) % STANDARD_PROPERTIES.len();
    }

    pub fn select_prev_property(&mut self) {
        if self.property_index == 0 {
            self.property_index = STANDARD_PROPERTIES.len() - 1;
        } else {
            self.property_index -= 1;
        }
    }

    pub fn select_next_type(&mut self) {
        match self.type_index {
            None => self.type_index = Some(0),
            Some(i) => self.type_index = Some((i + 1) % TYPE_VALUES.len()),
        }
    }

    pub fn select_prev_type(&mut self) {
        match self.type_index {
            None => self.type_index = Some(TYPE_VALUES.len() - 1),
            Some(0) => self.type_index = Some(TYPE_VALUES.len() - 1),
            Some(i) => self.type_index = Some(i - 1),
        }
    }
}

impl Default for AddFieldModal {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> App<'a> {
    pub fn new(db: &'a mut Database, config: &'a Config, provider: &'a dyn CryptoProvider) -> Result<Self> {
        let contacts = db.list_contacts(None)?;
        let mut app = Self {
            db,
            config,
            provider,
            contacts,
            selected: 0,
            search_input: Input::default(),
            show_search: true,
            search_focus: SearchFocus::Input,
            current_contact: None,
            current_props: Vec::new(),
            editor: InlineEditor::default(),
            status: None,
            aliases: Vec::new(),
            languages: Vec::new(),
            focused_pane: PaneFocus::Search,
            card_fields: Vec::new(),
            card_field_index: 0,
            details_sections: Vec::new(),
            details_field_index: 0,
            details_scroll: 0,
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
            reindex_modal: None,
            share_modal: None,
            add_field_modal: None,
            photo_path_modal: None,
            pending_reindex: false,
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

            // Handle pending reindex (shows blocking modal)
            if self.pending_reindex {
                self.pending_reindex = false;
                self.reindex_modal = Some(ReindexModal {
                    message: "REINDEXING...".to_string(),
                });
                draw::render(terminal, self)?; // Show the modal
                self.perform_reindex()?;
                self.reindex_modal = None;
                self.refresh_contacts()?;
                self.set_status("Reindex complete");
                continue;
            }

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

        // If help modal is open, handle its keys first
        if self.help_modal.is_some() {
            self.handle_help_modal_key(key);
            return Ok(false);
        }

        // If share modal is open, handle its keys
        if self.share_modal.is_some() {
            self.handle_share_modal_key(key);
            return Ok(false);
        }

        // Check top bar buttons (work in any context except modals/editor)
        if self.confirm_modal.is_none()
            && self.alias_modal.is_none()
            && self.multivalue_modal.is_none()
            && !self.editor.active
        {
            if let Some(action) = self.top_bar_action_for_key(&key) {
                return self.handle_top_bar_action(action);
            }
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

        if self.add_field_modal.is_some() {
            self.handle_add_field_modal_key(key)?;
            return Ok(false);
        }

        if self.photo_path_modal.is_some() {
            self.handle_photo_path_modal_key(key)?;
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

        // Navigation: add field
        if self.key_matches_any(&key, &nav.add_field) {
            // For ALIAS in Card pane, use existing add alias modal
            if let Some(field) = self.focused_field() {
                let label_upper = field.label.to_uppercase();
                if label_upper == "ALIAS" || label_upper.starts_with("ALIAS") {
                    self.modal_popup = PopupState::default();
                    self.alias_modal = Some(AliasModal { input: Input::default() });
                    self.set_status("Add alias");
                    return Ok(false);
                }
            }
            // For all other fields (or no field focused), open the generic add field modal
            if self.current_contact.is_some() {
                self.modal_popup = PopupState::default();
                self.add_field_modal = Some(AddFieldModal::new());
                self.set_status("Add field");
            } else {
                self.set_status("No contact selected");
            }
            return Ok(false);
        }

        // Navigation: delete field
        if self.key_matches_any(&key, &nav.delete_field) {
            if let Some(field) = self.focused_field() {
                if let Some(source) = field.source() {
                    // Show confirmation modal
                    self.modal_popup = PopupState::default();
                    self.confirm_modal = Some(ConfirmModal {
                        title: "DELETE FIELD".to_string(),
                        message: format!("Delete {} \"{}\"?", field.label, truncate_value(&field.value, 30)),
                        action: ConfirmAction::DeleteField {
                            field: source.field,
                            seq: source.seq,
                        },
                    });
                } else {
                    self.set_status("Field not deletable");
                }
            } else {
                self.set_status("No field selected");
            }
            return Ok(false);
        }

        // Navigation: photo fetch (only when Image pane is focused)
        if self.key_matches_any(&key, &nav.photo_fetch) {
            if matches!(self.focused_pane, PaneFocus::Image) {
                if self.current_contact.is_some() {
                    self.modal_popup = PopupState::default();
                    self.photo_path_modal = Some(PhotoPathModal { input: Input::default() });
                    self.set_status("Enter path to image");
                } else {
                    self.set_status("No contact selected");
                }
            } else {
                self.set_status("Focus Image pane (press 3) to add photo");
            }
            return Ok(false);
        }

        // Navigation: delete photo (only when Image pane is focused, use delete_field key)
        if matches!(self.focused_pane, PaneFocus::Image) && self.key_matches_any(&key, &nav.delete_field) {
            if self.current_contact.is_some() && self.photo_data.is_some() {
                self.modal_popup = PopupState::default();
                self.confirm_modal = Some(ConfirmModal {
                    title: "DELETE PHOTO".to_string(),
                    message: "Delete the contact photo?".to_string(),
                    action: ConfirmAction::DeletePhoto,
                });
            } else if self.photo_data.is_none() {
                self.set_status("No photo to delete");
            } else {
                self.set_status("No contact selected");
            }
            return Ok(false);
        }

        // Navigation: language cycle
        if self.key_matches_any(&key, &nav.lang_cycle) {
            self.set_status("Language toggle not yet implemented");
            return Ok(false);
        }

        // Digit shortcuts for pane focus (1-3) - only when search is closed
        if !self.show_search {
            if let KeyCode::Char(c) = key.code {
                if self.focus_by_digit(c) {
                    return Ok(false);
                }
            }
        }

        Ok(false)
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
                    self.modal_popup = PopupState::default();
                    self.confirm_modal = Some(ConfirmModal {
                        title: "MERGE CONTACTS".to_string(),
                        message: format!(
                            "Merge {} marked contacts into a single card?",
                            count
                        ),
                        action: ConfirmAction::MergeContacts,
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

        // Determine target directory (same as first contact)
        let target_dir = paths
            .first()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| self.config.vdir.clone());

        // Merge using the standalone function (handles encryption properly)
        let result = vcard_io::merge_vcard_files(
            &paths,
            &target_dir,
            self.provider,
            self.config.phone_region.as_deref(),
        )?;

        // Remove old files
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }

        // Update DB: delete old, insert new
        self.db
            .delete_items_by_paths(paths.clone().into_iter())?;
        let state = vdir::compute_file_state(&result.path)?;
        let record = indexer::build_record(&result.path, &result.card, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh UI
        self.marked.clear();
        self.show_marked_only = false;
        self.refresh_contacts()?;
        self.set_status("Merged contacts");
        Ok(())
    }

    /// Delete the current contact file
    fn delete_current_contact(&mut self) -> Result<()> {
        let Some(contact) = self.current_contact.take() else {
            self.set_status("No contact selected");
            return Ok(());
        };

        // Remove from database first
        self.db.delete_items_by_paths(std::iter::once(contact.path.clone()))?;

        // Delete the file
        if contact.path.exists() {
            std::fs::remove_file(&contact.path)
                .with_context(|| format!("failed to delete {}", contact.path.display()))?;
        }

        // Refresh contacts list
        self.refresh_contacts()?;
        self.set_status("Contact deleted");
        Ok(())
    }

    /// Delete a specific field from the current contact
    fn delete_field(&mut self, field: &str, seq: i64) -> Result<()> {
        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        let seq_usize = match usize::try_from(seq) {
            Ok(s) => s,
            Err(_) => {
                self.set_status("Invalid field index");
                return Ok(());
            }
        };

        // Parse the vCard file
        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(());
        }

        // Delete the field
        let deleted = {
            let card = cards.get_mut(0).unwrap();
            vcard_io::delete_card_field(card, field, seq_usize)
        };

        if !deleted {
            self.set_status("Failed to delete field");
            return Ok(());
        }

        // Write back
        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

        // Update database
        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh UI
        self.refresh_contacts()?;
        self.set_status("Field deleted");
        Ok(())
    }

    /// Delete the photo from the current contact
    fn delete_contact_photo(&mut self) -> Result<()> {
        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        // Parse the vCard file
        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(());
        }

        // Delete the photo
        {
            let card = cards.get_mut(0).unwrap();
            vcard_io::delete_photo(card);
        }

        // Write back
        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

        // Update database
        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh UI
        self.refresh_contacts()?;
        self.set_status("Photo deleted");
        Ok(())
    }

    /// Handle keys for photo path modal
    fn handle_photo_path_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let modal_keys = &self.config.keys.modal;

        // Cancel: close modal
        if self.key_matches_any(&key, &modal_keys.cancel) {
            self.photo_path_modal = None;
            return Ok(());
        }

        // Confirm: load and set the photo
        if self.key_matches_any(&key, &modal_keys.confirm) {
            let path = self.photo_path_modal
                .as_ref()
                .map(|m| m.input.value().trim().to_string())
                .unwrap_or_default();
            self.photo_path_modal = None;

            if path.is_empty() {
                self.set_status("No path entered");
                return Ok(());
            }

            // Expand ~ to home directory
            let expanded_path = if path.starts_with("~/") {
                if let Some(home) = home::home_dir() {
                    home.join(&path[2..])
                } else {
                    std::path::PathBuf::from(&path)
                }
            } else {
                std::path::PathBuf::from(&path)
            };

            self.set_contact_photo_from_path(&expanded_path)?;
            return Ok(());
        }

        // Route other keys to input
        if let Some(modal) = self.photo_path_modal.as_mut() {
            let _ = modal.input.handle_event(&Event::Key(key));
        }
        Ok(())
    }

    /// Load image from path, resize to max 128x128, and set as contact photo
    fn set_contact_photo_from_path(&mut self, path: &Path) -> Result<()> {
        use base64::Engine;
        use image::GenericImageView;
        use image::imageops::FilterType;

        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        // Check if file exists
        if !path.exists() {
            self.set_status(format!("File not found: {}", path.display()));
            return Ok(());
        }

        // Load the image
        let img = match image::open(path) {
            Ok(img) => img,
            Err(e) => {
                self.set_status(format!("Failed to load image: {}", e));
                return Ok(());
            }
        };

        // Resize to max 128x128 preserving aspect ratio
        let (width, height) = img.dimensions();
        let max_dim = 128u32;
        let (new_width, new_height) = if width > max_dim || height > max_dim {
            let ratio = f64::min(max_dim as f64 / width as f64, max_dim as f64 / height as f64);
            let new_w = (width as f64 * ratio).round() as u32;
            let new_h = (height as f64 * ratio).round() as u32;
            (new_w.max(1), new_h.max(1))
        } else {
            (width, height)
        };

        let resized = img.resize_exact(new_width, new_height, FilterType::Lanczos3);

        // Encode as JPEG
        let mut jpeg_data = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut jpeg_data);
            resized.write_to(&mut cursor, image::ImageFormat::Jpeg)
                .with_context(|| "failed to encode image as JPEG")?;
        }

        // Create data URI
        let base64_data = BASE64_STANDARD.encode(&jpeg_data);
        let data_uri = format!("data:image/jpeg;base64,{}", base64_data);

        // Parse and update the vCard
        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(());
        }

        {
            let card = cards.get_mut(0).unwrap();
            vcard_io::set_photo(card, &data_uri);
        }

        // Write back
        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

        // Update database
        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh UI
        self.refresh_contacts()?;
        self.set_status("Photo updated");
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

        // Modal: set default (only for fields that support it)
        if self.key_matches_any(&key, &modal_keys.set_default) {
            if let Some((field, item)) = self.current_modal_selection() {
                if field.has_default() {
                    if self.set_multivalue_default(field, item.seq)? {
                        self.rebuild_multivalue_modal(field, None);
                        self.set_status("Default updated");
                    }
                }
            }
            return Ok(());
        }

        // Modal: delete (only for Alias currently)
        if self.key_matches_any(&key, &modal_keys.delete) {
            if let Some((field, item)) = self.current_modal_selection() {
                if field == MultiValueField::Alias {
                    if self.delete_alias_entry(item.seq)? {
                        self.rebuild_multivalue_modal(field, None);
                        self.set_status("Alias deleted");
                    }
                }
            }
            return Ok(());
        }

        // Modal: add (only for Alias currently)
        if self.key_matches_any(&key, &modal_keys.add) {
            if let Some((field, _)) = self.current_modal_selection() {
                if field == MultiValueField::Alias {
                    // Close multivalue modal and open add-alias modal
                    self.multivalue_modal = None;
                    self.modal_popup = PopupState::default();
                    self.alias_modal = Some(AliasModal { input: Input::default() });
                    self.set_status("Add alias");
                }
            } else if let Some(modal) = &self.multivalue_modal {
                // No items selected, but modal is open for Alias
                if modal.field() == MultiValueField::Alias {
                    self.multivalue_modal = None;
                    self.modal_popup = PopupState::default();
                    self.alias_modal = Some(AliasModal { input: Input::default() });
                    self.set_status("Add alias");
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

        // Modal: confirm (sets default for EMAIL/PHONE, closes for ALIAS)
        if self.key_matches_any(&key, &modal_keys.confirm) {
            if let Some((field, item)) = self.current_modal_selection() {
                if field.has_default() {
                    if self.set_multivalue_default(field, item.seq)? {
                        self.rebuild_multivalue_modal(field, None);
                        self.set_status("Default updated");
                    }
                } else {
                    // For fields without default (Alias), just close the modal
                    self.close_multivalue_modal();
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

    fn handle_add_field_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let modal_keys = &self.config.keys.modal;

        // Cancel: close modal or go back
        if self.key_matches_any(&key, &modal_keys.cancel) {
            if let Some(modal) = &self.add_field_modal {
                match modal.state {
                    AddFieldState::SelectProperty => {
                        // Close modal entirely
                        self.add_field_modal = None;
                    }
                    AddFieldState::SelectType | AddFieldState::EnterCustomProperty => {
                        // Go back to property selection
                        if let Some(m) = self.add_field_modal.as_mut() {
                            m.state = AddFieldState::SelectProperty;
                        }
                    }
                    AddFieldState::EnterValue => {
                        // Go back to type selection or property selection
                        if let Some(m) = self.add_field_modal.as_mut() {
                            if let Some((_, _, supports_type)) = m.current_property() {
                                if supports_type {
                                    m.state = AddFieldState::SelectType;
                                } else {
                                    m.state = AddFieldState::SelectProperty;
                                }
                            } else {
                                m.state = AddFieldState::SelectProperty;
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        // Get current state
        let state = self.add_field_modal.as_ref().map(|m| m.state);
        let Some(state) = state else { return Ok(()); };

        match state {
            AddFieldState::SelectProperty => {
                // Next/prev property
                if self.key_matches_any(&key, &modal_keys.next) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        modal.select_next_property();
                    }
                    return Ok(());
                }
                if self.key_matches_any(&key, &modal_keys.prev) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        modal.select_prev_property();
                    }
                    return Ok(());
                }

                // Confirm: move to next step
                if self.key_matches_any(&key, &modal_keys.confirm) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        if let Some((_, vcard_field, supports_type)) = modal.current_property() {
                            if vcard_field == "X-" {
                                // Custom property: need to enter property name
                                modal.state = AddFieldState::EnterCustomProperty;
                            } else if supports_type {
                                // Has type parameter: go to type selection
                                modal.state = AddFieldState::SelectType;
                            } else {
                                // No type: go straight to value entry
                                modal.state = AddFieldState::EnterValue;
                            }
                        }
                    }
                    return Ok(());
                }
            }
            AddFieldState::SelectType => {
                // Next/prev type
                if self.key_matches_any(&key, &modal_keys.next) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        modal.select_next_type();
                    }
                    return Ok(());
                }
                if self.key_matches_any(&key, &modal_keys.prev) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        modal.select_prev_type();
                    }
                    return Ok(());
                }

                // Confirm: move to value entry
                if self.key_matches_any(&key, &modal_keys.confirm) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        modal.state = AddFieldState::EnterValue;
                    }
                    return Ok(());
                }
            }
            AddFieldState::EnterCustomProperty => {
                // Confirm: move to type selection (or value if no type support)
                if self.key_matches_any(&key, &modal_keys.confirm) {
                    if let Some(modal) = self.add_field_modal.as_mut() {
                        let custom_name = modal.custom_property_input.value().trim().to_string();
                        if custom_name.is_empty() {
                            self.set_status("Property name required");
                            return Ok(());
                        }
                        // Custom X- fields support types
                        modal.state = AddFieldState::SelectType;
                    }
                    return Ok(());
                }

                // Route other keys to input
                if let Some(modal) = self.add_field_modal.as_mut() {
                    let _ = modal.custom_property_input.handle_event(&Event::Key(key));
                }
            }
            AddFieldState::EnterValue => {
                // Confirm: add the field
                if self.key_matches_any(&key, &modal_keys.confirm) {
                    self.commit_add_field()?;
                    return Ok(());
                }

                // Route other keys to input
                if let Some(modal) = self.add_field_modal.as_mut() {
                    let _ = modal.value_input.handle_event(&Event::Key(key));
                }
            }
        }

        Ok(())
    }

    /// Commit the new field from add_field_modal to the current contact
    fn commit_add_field(&mut self) -> Result<()> {
        let Some(modal) = self.add_field_modal.take() else {
            return Ok(());
        };

        let value = modal.value_input.value().trim().to_string();
        if value.is_empty() {
            self.set_status("Value required");
            return Ok(());
        }

        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        // Determine field name
        let field_name = if let Some((_, vcard_field, _)) = modal.current_property() {
            if vcard_field == "X-" {
                // Custom property
                let custom = modal.custom_property_input.value().trim().to_uppercase();
                if custom.starts_with("X-") {
                    custom
                } else {
                    format!("X-{}", custom)
                }
            } else {
                vcard_field.to_string()
            }
        } else {
            self.set_status("No property selected");
            return Ok(());
        };

        // Get type parameter if selected
        let type_param = modal.current_type().map(|s| s.to_string());

        // Parse and update the vCard
        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(());
        }

        {
            let card = cards.get_mut(0).unwrap();
            let added = vcard_io::add_card_field(card, &field_name, &value, type_param.as_deref());
            if !added {
                self.set_status("Failed to add field");
                return Ok(());
            }
        }

        // Write back
        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

        // Update database
        let card_clone = cards[0].clone();
        let state = vdir::compute_file_state(&contact.path)?;
        let record = indexer::build_record(&contact.path, &card_clone, &state, None)?;
        self.db.upsert(&record.item, &record.props)?;

        // Refresh UI
        self.refresh_contacts()?;
        self.set_status(format!("{} added", field_name));

        Ok(())
    }

    fn handle_confirm_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(modal) = self.confirm_modal.take() else {
            return Ok(());
        };

        let modal_keys = &self.config.keys.modal;

        // Cancel: close modal without action
        if self.key_matches_any(&key, &modal_keys.cancel) {
            return Ok(());
        }

        // Also accept 'n' as cancel (common convention)
        if matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'n')) {
            return Ok(());
        }

        // Confirm: execute action based on modal type
        if self.key_matches_any(&key, &modal_keys.confirm) {
            match modal.action {
                ConfirmAction::MergeContacts => {
                    self.merge_marked_contacts()?;
                }
                ConfirmAction::DeleteContact => {
                    self.delete_current_contact()?;
                }
                ConfirmAction::DeleteField { field, seq } => {
                    self.delete_field(&field, seq)?;
                }
                ConfirmAction::DeletePhoto => {
                    self.delete_contact_photo()?;
                }
            }
            return Ok(());
        }

        // Put the modal back if key wasn't handled
        self.confirm_modal = Some(modal);
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

        // Determine the field kind - either from source or by label for ALIAS
        let kind = if let Some(source) = field.source() {
            MultiValueField::from_field_name(&source.field)
        } else if field.label.eq_ignore_ascii_case("ALIAS") {
            // ALIAS field without source (no existing aliases) - still allow opening
            Some(MultiValueField::Alias)
        } else {
            None
        };

        let Some(kind) = kind else {
            return false;
        };

        let items = self.build_multivalue_items(kind);

        // For EMAIL/PHONE, require at least 2 items to open modal
        // For ALIAS, allow opening even with 0 items (to add new aliases)
        if kind != MultiValueField::Alias && items.len() < 2 {
            return false;
        }

        let selected = field
            .source()
            .and_then(|source| items.iter().position(|item| item.seq == source.seq))
            .unwrap_or(0);
        self.modal_popup = PopupState::default();
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
        self.modal_popup = PopupState::default();
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
                let type_label = if field.has_type_label() {
                    extract_type_labels(&prop.params).unwrap_or_else(|| "—".to_string())
                } else {
                    String::new()
                };
                let (value, copy_value) = match field {
                    MultiValueField::Email => {
                        let trimmed = prop.value.trim().to_string();
                        (trimmed.clone(), trimmed)
                    }
                    MultiValueField::Phone => {
                        let display = vcard_io::phone_display_value(&prop.value, default_region);
                        (display.clone(), display)
                    }
                    MultiValueField::Alias => {
                        let trimmed = prop.value.trim().to_string();
                        (trimmed.clone(), trimmed)
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

        // Build details sections from config
        if self.current_contact.is_some() {
            self.details_sections = build_details_sections(
                &self.current_props,
                &self.config.details_sections,
                default_region,
            );
        } else {
            self.details_sections.clear();
        }

        let total_details = self.details_total_fields();
        if total_details == 0 {
            self.details_field_index = 0;
        } else if self.details_field_index >= total_details {
            self.details_field_index = 0;
        }
    }

    fn focus_pane(&mut self, pane: PaneFocus) {
        self.focused_pane = pane;
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
            PaneFocus::Details => {
                let total = self.details_total_fields();
                if total == 0 {
                    self.details_field_index = 0;
                } else if self.details_field_index >= total {
                    self.details_field_index = 0;
                }
            }
            PaneFocus::Image | PaneFocus::Search => {}
        }
    }

    /// Get total number of fields across all details sections
    fn details_total_fields(&self) -> usize {
        self.details_sections.iter().map(|s| s.fields.len()).sum()
    }

    /// Get a field from details by flat index
    fn details_field_at(&self, index: usize) -> Option<&DetailsField> {
        let mut offset = 0;
        for section in &self.details_sections {
            if index < offset + section.fields.len() {
                return section.fields.get(index - offset);
            }
            offset += section.fields.len();
        }
        None
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
            PaneFocus::Details => {
                let total = self.details_total_fields();
                if total == 0 {
                    return;
                }
                let len = total as isize;
                let current = self.details_field_index as isize;
                let next = (current + delta).rem_euclid(len);
                self.details_field_index = next as usize;
            }
            PaneFocus::Image | PaneFocus::Search => {}
        }
    }

    fn focused_field(&self) -> Option<PaneField> {
        match self.focused_pane {
            PaneFocus::Card => self.card_fields.get(self.card_field_index).cloned(),
            PaneFocus::Details => {
                self.details_field_at(self.details_field_index)
                    .map(|df| df.to_pane_field())
            }
            PaneFocus::Image | PaneFocus::Search => None,
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

    pub fn top_bar_buttons(&self) -> &[crate::config::TopBarButton] {
        &self.config.top_bar.buttons
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

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
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

        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

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

    fn delete_alias_entry(&mut self, seq: i64) -> Result<bool> {
        if seq < 0 {
            self.set_status("Unable to delete alias");
            return Ok(false);
        }

        let idx = match usize::try_from(seq) {
            Ok(value) => value,
            Err(_) => {
                self.set_status("Unable to delete alias");
                return Ok(false);
            }
        };

        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(false);
        };

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
        let mut cards = parsed.cards;
        if cards.is_empty() {
            self.set_status("Contact has no cards");
            return Ok(false);
        }

        let deleted = {
            let card = cards.get_mut(0).unwrap();
            vcard_io::delete_nickname_entry(card, idx)
        };

        if !deleted {
            self.set_status("Unable to delete alias");
            return Ok(false);
        }

        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

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

        Ok(true)
    }

    fn commit_field_edit(&mut self, target: FieldRef, new_value: String) -> Result<()> {
        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
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

        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

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

        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
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
                MultiValueField::Alias => {
                    // Aliases don't have a "default" concept
                    return Ok(false);
                }
            }
        };

        if !updated {
            self.set_status("Unable to set default");
            return Ok(false);
        }

        vcard_io::write_cards(&contact.path, &cards, self.provider)?;

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
            '2' => {
                self.focus_pane(PaneFocus::Details);
                true
            }
            '3' => {
                self.focus_pane(PaneFocus::Image);
                true
            }
            _ => false,
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
                        action: "Next Field",
                        keys: keys.navigation.next.join(", "),
                    },
                    HelpEntry {
                        action: "Previous Field",
                        keys: keys.navigation.prev.join(", "),
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
                        action: "Add Field",
                        keys: keys.navigation.add_field.join(", "),
                    },
                    HelpEntry {
                        action: "Delete Field",
                        keys: keys.navigation.delete_field.join(", "),
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
                    HelpEntry {
                        action: "Delete",
                        keys: keys.modal.delete.join(", "),
                    },
                    HelpEntry {
                        action: "Add",
                        keys: keys.modal.add.join(", "),
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
        self.modal_popup = PopupState::default();
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

    // =========================================================================
    // Top Bar Actions
    // =========================================================================

    /// Check if a key event matches a top bar button and return its action
    fn top_bar_action_for_key(&self, key: &KeyEvent) -> Option<TopBarAction> {
        for button in &self.config.top_bar.buttons {
            if self.key_matches_function_key(key, &button.key) {
                return Some(button.action);
            }
        }
        None
    }

    fn key_matches_function_key(&self, event: &KeyEvent, key_name: &str) -> bool {
        let code = match key_name.to_ascii_uppercase().as_str() {
            "F1" => KeyCode::F(1),
            "F2" => KeyCode::F(2),
            "F3" => KeyCode::F(3),
            "F4" => KeyCode::F(4),
            "F5" => KeyCode::F(5),
            "F6" => KeyCode::F(6),
            "F7" => KeyCode::F(7),
            "F8" => KeyCode::F(8),
            "F9" => KeyCode::F(9),
            "F10" => KeyCode::F(10),
            "F11" => KeyCode::F(11),
            "F12" => KeyCode::F(12),
            _ => return false,
        };
        event.code == code
    }

    fn handle_top_bar_action(&mut self, action: TopBarAction) -> Result<bool> {
        match action {
            TopBarAction::Help => {
                self.show_help();
                Ok(false)
            }
            TopBarAction::Sync => {
                // Check if any remotes are configured
                if self.config.remotes.is_empty() {
                    self.set_status("No remotes configured (use CLI: rldx sync)");
                } else {
                    let remote_names: Vec<&str> = self.config.remotes.iter()
                        .map(|r| r.name.as_str())
                        .collect();
                    self.set_status(format!("Sync from CLI: rldx sync ({})", remote_names.join(", ")));
                }
                Ok(false)
            }
            TopBarAction::Refresh => {
                // Set flag to trigger reindex in event loop (allows showing blocking modal)
                self.pending_reindex = true;
                Ok(false)
            }
            TopBarAction::Share => {
                self.show_share_modal()?;
                Ok(false)
            }
            TopBarAction::Delete => {
                // Delete current contact with confirmation
                if let Some(contact) = &self.current_contact {
                    self.modal_popup = PopupState::default();
                    self.confirm_modal = Some(ConfirmModal {
                        title: "DELETE CONTACT".to_string(),
                        message: format!("Delete {}?", contact.display_fn),
                        action: ConfirmAction::DeleteContact,
                    });
                } else {
                    self.set_status("No contact selected");
                }
                Ok(false)
            }
        }
    }

    /// Perform the actual reindex operation
    fn perform_reindex(&mut self) -> Result<()> {
        let files = vdir::list_vcf_files(&self.config.vdir)?;
        let paths_set: HashSet<_> = files.iter().cloned().collect();

        // Force full reindex
        self.db.reset_schema()?;

        for path in files {
            let state = vdir::compute_file_state(&path)?;
            let parsed = vcard_io::parse_file(&path, self.config.phone_region.as_deref(), self.provider)?;
            let cards = parsed.cards;

            if cards.is_empty() {
                continue;
            }

            let final_state = if parsed.changed {
                vdir::compute_file_state(&path)?
            } else {
                state
            };

            let card = cards.into_iter().next().unwrap();
            let record = indexer::build_record(&path, &card, &final_state, None)?;
            self.db.upsert(&record.item, &record.props)?;
        }

        self.db.remove_missing(&paths_set)?;
        Ok(())
    }

    // =========================================================================
    // Share Modal (QR Code)
    // =========================================================================

    fn show_share_modal(&mut self) -> Result<()> {
        use qrcode::{QrCode, render::unicode};

        let Some(contact) = &self.current_contact else {
            self.set_status("No contact selected");
            return Ok(());
        };

        // Parse the vCard file to get full card data
        let parsed = vcard_io::parse_file(&contact.path, self.config.phone_region.as_deref(), self.provider)?;
        let Some(mut card) = parsed.cards.into_iter().next() else {
            self.set_status("Unable to load contact");
            return Ok(());
        };

        // Remove PHOTO data to keep QR code manageable
        card.photo.clear();
        card.logo.clear();

        // Serialize to vCard string
        let vcard_string = card.to_string();

        // Check if data is too large for QR code (max ~2953 bytes for alphanumeric)
        if vcard_string.len() > 2500 {
            self.set_status("Contact data too large for QR code");
            return Ok(());
        }

        // Generate QR code
        let code = match QrCode::new(vcard_string.as_bytes()) {
            Ok(c) => c,
            Err(e) => {
                self.set_status(format!("QR generation failed: {}", e));
                return Ok(());
            }
        };

        // Render to Unicode using half-blocks for compact display
        // Dark modules rendered as Dark (will be colored), light as Light (background)
        let qr_string = code
            .render::<unicode::Dense1x2>()
            .dark_color(unicode::Dense1x2::Dark)
            .light_color(unicode::Dense1x2::Light)
            .build();

        let qr_lines: Vec<String> = qr_string.lines().map(|s| s.to_string()).collect();

        self.modal_popup = PopupState::default();
        self.share_modal = Some(ShareModal { qr_lines });

        Ok(())
    }

    fn handle_share_modal_key(&mut self, key: KeyEvent) {
        // Close on Escape or q
        if matches!(key.code, KeyCode::Esc) || matches!(key.code, KeyCode::Char('q')) {
            self.share_modal = None;
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

    let first_nickname = props.iter().find(|p| p.field == "NICKNAME");
    let total_nickname_count = props.iter().filter(|p| p.field == "NICKNAME").count();

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
            "alias" => {
                // Show alias field even if empty (allows adding new aliases)
                if let Some(prop) = first_nickname {
                    let display_value = if total_nickname_count > 1 {
                        format!("{} [{}]", alias_value, total_nickname_count)
                    } else {
                        alias_value.clone()
                    };
                    fields.push(PaneField::from_prop(
                        "ALIAS",
                        display_value,
                        alias_value.clone(),
                        "NICKNAME",
                        prop.seq,
                        None,
                    ));
                } else {
                    // No aliases exist yet - show placeholder without source
                    fields.push(PaneField::new("ALIAS", alias_value.clone()));
                }
            }
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

/// Build details sections from config, matching props to configured sections
fn build_details_sections(
    props: &[PropRow],
    config: &DetailsSectionsConfig,
    default_region: Option<&str>,
) -> Vec<DetailsSection> {
    use std::collections::HashSet;
    
    let mut sections = Vec::new();
    let mut used_props: HashSet<(String, i64)> = HashSet::new(); // (field, seq) pairs that have been assigned
    
    // Fields to exclude from Extras (card pane fields)
    let card_fields: HashSet<&str> = ["FN", "N", "NICKNAME", "PHOTO", "LOGO", "REV", "UID", "PRODID", "VERSION"]
        .iter()
        .copied()
        .collect();
    
    // Build each configured section
    for section_config in &config.sections {
        let mut fields = Vec::new();
        
        for prop in props {
            let field_upper = prop.field.to_uppercase();
            
            // Check if this field matches any of the section's configured fields
            let matches = section_config.fields.iter().any(|f| {
                let f_upper = f.to_uppercase();
                if f_upper.ends_with('*') {
                    // Wildcard match (e.g., "X-*" matches "X-TELEGRAM")
                    field_upper.starts_with(&f_upper[..f_upper.len()-1])
                } else {
                    field_upper == f_upper
                }
            });
            
            if matches {
                let prop_key = (prop.field.clone(), prop.seq);
                if !used_props.contains(&prop_key) {
                    used_props.insert(prop_key);
                    fields.push(build_details_field(prop, default_region));
                }
            }
        }
        
        // Only add section if it has fields
        if !fields.is_empty() {
            sections.push(DetailsSection {
                name: section_config.name.clone(),
                fields,
            });
        }
    }
    
    // Build "Extras" section for any remaining props not covered by configured sections
    let mut extras_fields = Vec::new();
    for prop in props {
        let field_upper = prop.field.to_uppercase();
        let prop_key = (prop.field.clone(), prop.seq);
        
        // Skip if already used or if it's a card-pane field
        if used_props.contains(&prop_key) || card_fields.contains(field_upper.as_str()) {
            continue;
        }
        
        extras_fields.push(build_details_field(prop, default_region));
    }
    
    if !extras_fields.is_empty() {
        sections.push(DetailsSection {
            name: "Extras".to_string(),
            fields: extras_fields,
        });
    }
    
    sections
}

/// Build a single field for the details pane
fn build_details_field(prop: &PropRow, default_region: Option<&str>) -> DetailsField {
    let field_upper = prop.field.to_uppercase();
    
    // Extract all parameters (except PREF which is handled in multivalue fields)
    let params = extract_field_params(&prop.params);
    
    // Format value based on field type
    let (label, value, copy_value) = match field_upper.as_str() {
        "TEL" => {
            let base = vcard_io::phone_display_value(&prop.value, default_region);
            ("TEL".to_string(), base.clone(), base)
        }
        "EMAIL" => {
            let base = prop.value.trim().to_string();
            ("EMAIL".to_string(), base.clone(), base)
        }
        "ADR" => {
            let formatted = format_address_value(prop);
            ("ADDRESS".to_string(), formatted.clone(), formatted)
        }
        "NOTE" => {
            let value = prop.value.clone();
            ("".to_string(), value.clone(), value)
        }
        _ => {
            // For X-* fields, use a cleaner label
            let label = if field_upper.starts_with("X-") {
                field_upper[2..].to_string()
            } else {
                field_upper.clone()
            };
            let value = prop.value.clone();
            (label, value.clone(), value)
        }
    };
    
    // Build full display label
    let display_label = if label.is_empty() {
        prop.field.to_uppercase()
    } else {
        label
    };
    
    DetailsField {
        label: display_label,
        value,
        copy_value,
        params,
        source: Some(FieldRef::new(prop.field.clone(), prop.seq)),
    }
}

/// Extract all field parameters as a map of name -> values
/// Excludes PREF (handled in multivalue fields) and VALUE (internal vCard param)
fn extract_field_params(params: &Value) -> std::collections::HashMap<String, Vec<String>> {
    use std::collections::HashMap;
    
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    
    let Some(obj) = params.as_object() else {
        return result;
    };
    
    for (key, value) in obj {
        let key_upper = key.to_uppercase();
        
        // Skip PREF (handled in multivalue) and VALUE (internal vCard param)
        if key_upper == "PREF" || key_upper == "VALUE" {
            continue;
        }
        
        let values = match value {
            Value::String(s) if !s.is_empty() => vec![s.to_uppercase()],
            Value::Array(items) => {
                items.iter()
                    .filter_map(|item| item.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_uppercase())
                    .collect()
            }
            _ => continue,
        };
        
        if !values.is_empty() {
            result.insert(key_upper, values);
        }
    }
    
    result
}

/// Extract TYPE parameter as a list of strings
fn extract_type_list(params: &Value) -> Vec<String> {
    let Some(entry) = params.get("type") else {
        return Vec::new();
    };
    
    match entry {
        Value::String(s) if !s.is_empty() => vec![s.to_uppercase()],
        Value::Array(items) => {
            items.iter()
                .filter_map(|item| item.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_uppercase())
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Extract TYPE parameter as a combined string (e.g., "WORK/CELL")
fn extract_type_labels(params: &Value) -> Option<String> {
    let types = extract_type_list(params);
    if types.is_empty() {
        None
    } else {
        Some(types.join("/"))
    }
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

/// Truncate a string to max_len characters, adding "..." if truncated
fn truncate_value(value: &str, max_len: usize) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..max_len.saturating_sub(3)])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFocus {
    Input,
    Results,
}
