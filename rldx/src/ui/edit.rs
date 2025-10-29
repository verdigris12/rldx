#[derive(Default)]
pub struct InlineEditor {
    pub active: bool,
    pub field: Option<String>,
    pub value: String,
}

impl InlineEditor {
    pub fn start(&mut self, field: &str, current: &str) {
        self.active = true;
        self.field = Some(field.to_string());
        self.value = current.to_string();
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.field = None;
        self.value.clear();
    }
}