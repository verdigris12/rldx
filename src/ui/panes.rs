/// Panel identifiers for the new 3-panel layout
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    /// Panel 1: Contact card (name, alias, phone, email)
    Card,
    /// Panel 2: Details (scrollable sections: Notes, Contacts, Job, Personal, Extras)
    Details,
    /// Panel 3: Profile image
    Image,
}

#[allow(dead_code)]
impl Panel {
    pub const ALL: [Panel; 3] = [Panel::Card, Panel::Details, Panel::Image];

    pub const COUNT: usize = 3;

    pub fn title(self) -> &'static str {
        match self {
            Panel::Card => "CARD",
            Panel::Details => "DETAILS",
            Panel::Image => "IMAGE",
        }
    }

    pub fn digit(self) -> char {
        match self {
            Panel::Card => '1',
            Panel::Details => '2',
            Panel::Image => '3',
        }
    }

    pub fn index(self) -> usize {
        match self {
            Panel::Card => 0,
            Panel::Details => 1,
            Panel::Image => 2,
        }
    }

    pub fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '1' => Some(Panel::Card),
            '2' => Some(Panel::Details),
            '3' => Some(Panel::Image),
            _ => None,
        }
    }

    /// Get the next panel, or None if at the end
    pub fn next(self) -> Option<Self> {
        match self {
            Panel::Card => Some(Panel::Details),
            Panel::Details => Some(Panel::Image),
            Panel::Image => None,
        }
    }

    /// Get the previous panel, or None if at the beginning
    pub fn prev(self) -> Option<Self> {
        match self {
            Panel::Card => None,
            Panel::Details => Some(Panel::Card),
            Panel::Image => Some(Panel::Details),
        }
    }
}
