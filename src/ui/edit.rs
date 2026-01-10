use crossterm::event::{Event, KeyEvent};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRef {
    pub field: String,
    pub seq: i64,
    pub component: Option<usize>,
}

impl FieldRef {
    pub fn new(field: impl Into<String>, seq: i64) -> Self {
        Self {
            field: field.into(),
            seq,
            component: None,
        }
    }

    pub fn with_component(field: impl Into<String>, seq: i64, component: usize) -> Self {
        Self {
            field: field.into(),
            seq,
            component: Some(component),
        }
    }
}

#[derive(Default)]
pub struct InlineEditor {
    pub active: bool,
    target: Option<FieldRef>,
    input: Input,
}

impl InlineEditor {
    pub fn start(&mut self, current: &str, target: FieldRef) {
        self.active = true;
        self.target = Some(target);
        self.input = Input::new(current.to_string());
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.target = None;
        self.input.reset();
    }

    pub fn target(&self) -> Option<&FieldRef> {
        self.target.as_ref()
    }

    pub fn value(&self) -> &str {
        self.input.value()
    }

    pub fn visual_cursor(&self) -> usize {
        self.input.visual_cursor()
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        self.input.handle_event(&Event::Key(key)).is_some()
    }
}
