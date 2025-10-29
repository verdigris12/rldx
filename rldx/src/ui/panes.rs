#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Work,
    Personal,
    Accounts,
    Metadata,
}

impl DetailTab {
    pub fn next(self) -> Self {
        match self {
            DetailTab::Work => DetailTab::Personal,
            DetailTab::Personal => DetailTab::Accounts,
            DetailTab::Accounts => DetailTab::Metadata,
            DetailTab::Metadata => DetailTab::Work,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            DetailTab::Work => "Work",
            DetailTab::Personal => "Personal",
            DetailTab::Accounts => "Accounts",
            DetailTab::Metadata => "Full Metadata",
        }
    }
}