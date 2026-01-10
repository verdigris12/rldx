#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailTab {
    Work,
    Personal,
    Accounts,
    Metadata,
}

impl DetailTab {
    pub const ALL: [DetailTab; 4] = [
        DetailTab::Work,
        DetailTab::Personal,
        DetailTab::Accounts,
        DetailTab::Metadata,
    ];

    pub const COUNT: usize = 4;

    pub fn title(self) -> &'static str {
        match self {
            DetailTab::Work => "Work",
            DetailTab::Personal => "Personal",
            DetailTab::Accounts => "Accounts",
            DetailTab::Metadata => "Full Metadata",
        }
    }

    pub fn digit(self) -> char {
        match self {
            DetailTab::Work => '2',
            DetailTab::Personal => '3',
            DetailTab::Accounts => '4',
            DetailTab::Metadata => '5',
        }
    }

    pub fn index(self) -> usize {
        match self {
            DetailTab::Work => 0,
            DetailTab::Personal => 1,
            DetailTab::Accounts => 2,
            DetailTab::Metadata => 3,
        }
    }

    pub fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '2' => Some(DetailTab::Work),
            '3' => Some(DetailTab::Personal),
            '4' => Some(DetailTab::Accounts),
            '5' => Some(DetailTab::Metadata),
            _ => None,
        }
    }

    /// Get the next tab, or None if at the end
    pub fn next(self) -> Option<Self> {
        match self {
            DetailTab::Work => Some(DetailTab::Personal),
            DetailTab::Personal => Some(DetailTab::Accounts),
            DetailTab::Accounts => Some(DetailTab::Metadata),
            DetailTab::Metadata => None,
        }
    }

    /// Get the previous tab, or None if at the beginning
    pub fn prev(self) -> Option<Self> {
        match self {
            DetailTab::Work => None,
            DetailTab::Personal => Some(DetailTab::Work),
            DetailTab::Accounts => Some(DetailTab::Personal),
            DetailTab::Metadata => Some(DetailTab::Accounts),
        }
    }
}
