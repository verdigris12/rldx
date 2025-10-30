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
    pub value: String,
}

impl InlineEditor {
    pub fn start(&mut self, current: &str, target: FieldRef) {
        self.active = true;
        self.target = Some(target);
        self.value = current.to_string();
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.target = None;
        self.value.clear();
    }

    pub fn target(&self) -> Option<&FieldRef> {
        self.target.as_ref()
    }
}
