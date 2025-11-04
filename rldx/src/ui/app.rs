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

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;

use crate::config::{CommandExec, Config, UiColors};
use crate::db::{ContactItem, ContactListEntry, Database, PropRow};
use crate::indexer;
use crate::search;
use crate::vcard_io;
use crate::vdir;

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

pub struct App<'a> {
    db: &'a mut Database,
    config: &'a Config,
    pub contacts: Vec<ContactListEntry>,
    pub selected: usize,
    pub search_input: Input,
    pub show_search: bool,
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
    image_picker: Picker,
    image_state: Option<Box<dyn StatefulProtocol>>,
    pub photo_data: Option<PhotoData>,
    pub photo_error: Option<String>,
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
            image_picker: create_image_picker(),
            image_state: None,
            photo_data: None,
            photo_error: None,
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
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Ok(true);
        }

        if self.editor.active {
            if self.handle_editor_key(key)? {
                return Ok(false);
            }
        }

        if self.show_search && self.handle_search_key(key)? {
            return Ok(false);
        }

        if self.key_matches(&key, &self.config.keys.quit)
            && !matches!(self.focused_pane, PaneFocus::Search)
        {
            return Ok(true);
        }

        if self.show_search && self.key_matches(&key, &self.config.keys.confirm) {
            self.focus_pane(PaneFocus::Card);
            self.refresh_contacts()?;
            return Ok(false);
        }

        if self.key_matches(&key, &self.config.keys.toggle_search) {
            self.show_search = true;
            self.focused_pane = PaneFocus::Search;
            return Ok(false);
        }

        if self.key_matches(&key, &self.config.keys.tab_next) {
            self.advance_field(1);
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                if self.show_search {
                    self.focus_pane(PaneFocus::Card);
                    self.refresh_contacts()?;
                } else if self.editor.active {
                    self.editor.cancel();
                } else {
                    self.focus_pane(PaneFocus::Card);
                }
            }
            KeyCode::BackTab => {
                self.advance_field(-1);
            }
            KeyCode::Backspace => {
                if !self.show_search {
                    self.advance_field(-1);
                }
            }
            KeyCode::Char(' ') => {
                self.copy_focused_value()?;
            }
            KeyCode::Char(c) => {
                if self.focus_by_digit(c) {
                    // handled by digit shortcuts
                } else if self.key_matches(&key, &self.config.keys.next) {
                    self.move_selection(1)?;
                } else if self.key_matches(&key, &self.config.keys.prev) {
                    self.move_selection(-1)?;
                } else if self.key_matches(&key, &self.config.keys.edit) {
                    self.begin_edit();
                } else if self.key_matches(&key, &self.config.keys.photo_fetch) {
                    self.set_status("Image fetch is not yet implemented");
                } else if self.key_matches(&key, &self.config.keys.lang_next) {
                    self.set_status("Language toggle not yet implemented");
                }
            }
            KeyCode::Down => self.move_selection(1)?,
            KeyCode::Up => self.move_selection(-1)?,
            KeyCode::PageDown => self.move_selection(5)?,
            KeyCode::PageUp => self.move_selection(-5)?,
            _ => {}
        }
        Ok(false)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Result<bool> {
        if matches!(key.code, KeyCode::Esc) {
            self.show_search = false;
            self.refresh_contacts()?;
            return Ok(true);
        }

        if let Some(change) = self.search_input.handle_event(&Event::Key(key)) {
            if change.value {
                self.refresh_contacts()?;
            }
            return Ok(true);
        }

        Ok(false)
    }

    fn handle_editor_key(&mut self, key: KeyEvent) -> Result<bool> {
        if !self.editor.active {
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                self.editor.cancel();
                self.set_status("Edit cancelled");
            }
            KeyCode::Enter => {
                if let Some(target) = self.editor.target().cloned() {
                    let value = self.editor.value().to_string();
                    self.editor.cancel();
                    self.commit_field_edit(target, value)?;
                    self.set_status("Field updated");
                } else {
                    self.editor.cancel();
                    self.set_status("Field not editable");
                }
            }
            _ => {
                self.editor.handle_key_event(key);
            }
        }

        Ok(true)
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

            let icon = if contact_is_org(contact) {
                &self.config.ui.icons.organization
            } else {
                &self.config.ui.icons.contact
            };
            let text = format!("{}{}", icon, contact.display_fn.to_uppercase());
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

        let value = field.copy_text().trim();
        if value.is_empty() {
            self.set_status("Nothing to copy");
            return Ok(());
        }

        if let Some(command) = self.config.commands.copy.clone() {
            match self.run_copy_command(&command, value) {
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

    fn key_matches(&self, event: &KeyEvent, binding: &str) -> bool {
        let trimmed = binding.trim();
        if trimmed.is_empty() {
            return false;
        }

        let disallowed = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
        if event.modifiers.intersects(disallowed) {
            return false;
        }

        match trimmed.to_ascii_lowercase().as_str() {
            "enter" => matches!(event.code, KeyCode::Enter),
            "tab" => matches!(event.code, KeyCode::Tab),
            "backtab" | "shift+tab" => matches!(event.code, KeyCode::BackTab),
            "backspace" => matches!(event.code, KeyCode::Backspace),
            "esc" | "escape" => matches!(event.code, KeyCode::Esc),
            "space" => matches!(event.code, KeyCode::Char(' ')),
            _ => {
                let mut chars = trimmed.chars();
                if let (Some(first), None) = (chars.next(), chars.next()) {
                    matches!(event.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&first))
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
                if !handled_name {
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
                if !handled_name {
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
