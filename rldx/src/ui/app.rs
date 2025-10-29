use std::io::stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
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
            KeyCode::Char('q') if !self.show_search => return Ok(true),
            KeyCode::Esc => {
                if self.show_search {
                    self.show_search = false;
                    self.refresh_contacts()?;
                } else if self.editor.active {
                    self.editor.cancel();
                }
            }
            KeyCode::Char('/') => {
                self.show_search = true;
            }
            KeyCode::Enter => {
                if self.show_search {
                    self.show_search = false;
                    self.refresh_contacts()?;
                }
            }
            KeyCode::Tab => {
                self.tab = self.tab.next();
            }
            KeyCode::Char(c) => {
                let lower = c.to_ascii_lowercase();
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
        let filter = if self.show_search {
            search::normalize_query(&self.query)
        } else {
            None
        };
        self.contacts = if let Some(filter) = filter {
            self.db.list_contacts(Some(&filter))?
        } else {
            self.db.list_contacts(None)?
        };
        if self.contacts.is_empty() {
            self.selected = 0;
            self.current_contact = None;
            self.current_props.clear();
            self.aliases.clear();
            self.languages.clear();
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
            return Ok(());
        }
        let contact = &self.contacts[self.selected];
        self.current_contact = self.db.get_contact(&contact.uuid)?;
        self.current_props = self.db.get_props(&contact.uuid)?;
        self.aliases = collect_aliases(&self.current_props, &contact.display_fn);
        self.languages = collect_languages(&self.current_props);
        Ok(())
    }
}

fn collect_aliases(props: &[PropRow], display_fn: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for prop in props.iter().filter(|p| p.field == "NICKNAME" || p.field == "FN") {
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
