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
    label: Option<String>,
    target: Option<FieldRef>,
    pub value: String,
}

impl InlineEditor {
    pub fn start(&mut self, label: &str, current: &str, target: FieldRef) {
        self.active = true;
        self.label = Some(label.to_string());
        self.target = Some(target);
        self.value = current.to_string();
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.label = None;
        self.target = None;
        self.value.clear();
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn target(&self) -> Option<&FieldRef> {
        self.target.as_ref()
    }
}
