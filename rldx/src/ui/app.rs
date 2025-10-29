use std::io::stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::Value;

use crate::config::Config;
use crate::db::{ContactItem, ContactListEntry, Database, PropRow};
use crate::search;

use super::draw;
use super::edit::InlineEditor;
use super::panes::DetailTab;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneField {
    pub label: String,
    pub value: String,
}

impl PaneField {
    fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    Search,
    Card,
    Detail(DetailTab),
}

pub struct App<'a> {
    db: &'a Database,
    config: &'a Config,
    pub contacts: Vec<ContactListEntry>,
    pub selected: usize,
    pub query: String,
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
}

impl<'a> App<'a> {
    pub fn new(db: &'a Database, config: &'a Config) -> Result<Self> {
        let contacts = db.list_contacts(None)?;
        let mut app = Self {
            db,
            config,
            contacts,
            selected: 0,
            query: String::new(),
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
        };
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
        if self.show_search && self.handle_search_key(key)? {
            return Ok(false);
        }

        match key.code {
            KeyCode::Char('q') if !matches!(self.focused_pane, PaneFocus::Search) => {
                return Ok(true)
            }
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
            KeyCode::Char('/') => {
                self.show_search = true;
                self.focused_pane = PaneFocus::Search;
            }
            KeyCode::Enter => {
                if self.show_search {
                    self.focus_pane(PaneFocus::Card);
                    self.refresh_contacts()?;
                }
            }
            KeyCode::Tab => {
                self.advance_field(1);
            }
            KeyCode::BackTab => {
                self.advance_field(-1);
            }
            KeyCode::Backspace => {
                if !self.show_search {
                    self.advance_field(-1);
                }
            }
            KeyCode::Char(c) => {
                let lower = c.to_ascii_lowercase();
                if self.focus_by_digit(c) {
                    // handled
                } else {
                    match lower {
                        'j' => self.move_selection(1)?,
                        'k' => self.move_selection(-1)?,
                        'e' => {
                            self.status = Some("Editing is not yet implemented".to_string());
                        }
                        'i' => {
                            self.status = Some("Image fetch is not yet implemented".to_string());
                        }
                        'l' => {
                            self.status = Some("Language toggle not yet implemented".to_string());
                        }
                        _ => {}
                    }
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
        match key.code {
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.query.push(c);
                    self.refresh_contacts()?;
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refresh_contacts()?;
            }
            KeyCode::Esc => {
                self.show_search = false;
                self.refresh_contacts()?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn refresh_contacts(&mut self) -> Result<()> {
        let normalized = search::normalize_query(&self.query);
        self.contacts = if let Some(filter) = normalized.as_ref() {
            self.db.list_contacts(Some(filter))?
        } else {
            self.db.list_contacts(None)?
        };
        if self.contacts.is_empty() {
            self.selected = 0;
            self.current_contact = None;
            self.current_props.clear();
            self.aliases.clear();
            self.languages.clear();
            self.rebuild_field_views();
        } else {
            if self.selected >= self.contacts.len() {
                self.selected = self.contacts.len() - 1;
            }
            self.load_selection()?;
        }
        Ok(())
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
            self.rebuild_field_views();
            return Ok(());
        }
        let contact = &self.contacts[self.selected];
        self.current_contact = self.db.get_contact(&contact.uuid)?;
        self.current_props = self.db.get_props(&contact.uuid)?;
        self.aliases = collect_aliases(&self.current_props, &contact.display_fn);
        self.languages = collect_languages(&self.current_props);
        self.rebuild_field_views();
        Ok(())
    }

    fn rebuild_field_views(&mut self) {
        if self.current_contact.is_some() {
            self.card_fields = build_card_fields(&self.current_props, &self.aliases);
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
                self.tab_fields[idx] = build_tab_fields(tab, &self.current_props);
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

fn build_card_fields(props: &[PropRow], aliases: &[String]) -> Vec<PaneField> {
    let mut fields = Vec::new();

    if let Some(name_parts) = extract_name_parts(props) {
        let NameParts {
            family,
            given,
            additional,
        } = name_parts;
        fields.push(PaneField::new("FNAME", fallback_placeholder(given)));
        fields.push(PaneField::new("MNAME", fallback_placeholder(additional)));
        fields.push(PaneField::new("LNAME", fallback_placeholder(family)));
    } else {
        fields.push(PaneField::new("FNAME", "—"));
        fields.push(PaneField::new("MNAME", "—"));
        fields.push(PaneField::new("LNAME", "—"));
    }

    if !aliases.is_empty() {
        fields.push(PaneField::new("ALIAS", aliases.join("/")));
    }

    for prop in props.iter().filter(|p| p.field == "TEL") {
        let label = format_label_with_type("PHONE", &prop.params);
        let value = format_with_index(&prop.value, prop.seq);
        fields.push(PaneField::new(label, value));
    }

    for prop in props.iter().filter(|p| p.field == "EMAIL") {
        let label = format_label_with_type("EMAIL", &prop.params);
        let value = format_with_index(&prop.value, prop.seq);
        fields.push(PaneField::new(label, value));
    }

    fields
}

fn build_tab_fields(tab: DetailTab, props: &[PropRow]) -> Vec<PaneField> {
    match tab {
        DetailTab::Work => build_work_fields(props),
        DetailTab::Personal => build_personal_fields(props),
        DetailTab::Accounts => build_account_fields(props),
        DetailTab::Metadata => build_metadata_fields(props),
    }
}

fn build_work_fields(props: &[PropRow]) -> Vec<PaneField> {
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
            fields.push(PaneField::new(
                label,
                format_with_index(&prop.value, prop.seq),
            ));
        }
    }

    for prop in props.iter().filter(|p| p.field == "TEL") {
        if prop_has_type(&prop.params, "work") {
            let label = format_label_with_type("PHONE", &prop.params);
            fields.push(PaneField::new(
                label,
                format_with_index(&prop.value, prop.seq),
            ));
        }
    }

    fields
}

fn build_personal_fields(props: &[PropRow]) -> Vec<PaneField> {
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
            fields.push(PaneField::new(
                label,
                format_with_index(&prop.value, prop.seq),
            ));
        }
    }

    for prop in props.iter().filter(|p| p.field == "TEL") {
        if prop_has_type(&prop.params, "home") {
            let label = format_label_with_type("PHONE", &prop.params);
            fields.push(PaneField::new(
                label,
                format_with_index(&prop.value, prop.seq),
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

fn fallback_placeholder(value: String) -> String {
    if value.trim().is_empty() {
        "—".to_string()
    } else {
        value
    }
}

struct NameParts {
    family: String,
    given: String,
    additional: String,
}

fn extract_name_parts(props: &[PropRow]) -> Option<NameParts> {
    let prop = props.iter().find(|p| p.field == "N")?;
    let mut parts = prop.value.split(';');
    let family = parts.next().unwrap_or("").to_string();
    let given = parts.next().unwrap_or("").to_string();
    let additional = parts.next().unwrap_or("").to_string();
    Some(NameParts {
        family,
        given,
        additional,
    })
}
