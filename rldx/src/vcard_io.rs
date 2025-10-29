use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use uuid::Uuid;
use vcard4::property::{DateTimeProperty, TextOrUriProperty, TextProperty};
use vcard4::{parse, DateTime, Vcard};

/// Representation of a parsed vCard alongside metadata derived from the
/// original source text.
#[derive(Debug, Clone)]
pub struct CardWithSource {
    pub card: Vcard,
    pub raw_block: String,
    pub is_v4: bool,
}

/// Parse a UTF-8 encoded vCard file into `Vcard` values.
pub fn parse_file(path: &Path) -> Result<Vec<Vcard>> {
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read vCard file at {}", path.display()))?;
    parse_str(&input)
}

/// Parse a UTF-8 string into `Vcard` values.
pub fn parse_str(input: &str) -> Result<Vec<Vcard>> {
    parse(input).map_err(|err| anyhow!(err)).context("parsing vCard data")
}

/// Parse a UTF-8 string and also capture the raw block for each vCard.
pub fn parse_str_with_source(input: &str) -> Result<Vec<CardWithSource>> {
    let cards = parse_str(input)?;
    let blocks = extract_card_blocks(input);

    if cards.len() != blocks.len() {
        return Err(anyhow!(
            "parsed {} cards but located {} BEGIN/END blocks",
            cards.len(),
            blocks.len()
        ));
    }

    Ok(cards
        .into_iter()
        .zip(blocks.into_iter())
        .map(|(card, raw_block)| {
            let is_v4 = raw_block
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case("VERSION:4.0"));
            CardWithSource {
                card,
                raw_block,
                is_v4,
            }
        })
        .collect())
}

fn extract_card_blocks(input: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut collecting = false;
    let mut current: Vec<String> = Vec::new();

    for line in input.lines() {
        let line_no_cr = line.trim_end_matches('\r');
        if line_no_cr.eq_ignore_ascii_case("BEGIN:VCARD") {
            if collecting && !current.is_empty() {
                blocks.push(current.join("\n"));
                current.clear();
            }
            collecting = true;
        }

        if collecting {
            current.push(line_no_cr.to_string());
            if line_no_cr.eq_ignore_ascii_case("END:VCARD") {
                blocks.push(current.join("\n"));
                current.clear();
                collecting = false;
            }
        }
    }

    if collecting && !current.is_empty() {
        blocks.push(current.join("\n"));
    }

    blocks
}

/// Ensure the provided card has a UUID-based UID property.
///
/// Returns the UUID that is now stored in the card.
pub fn ensure_uuid_uid(card: &mut Vcard) -> Result<Uuid> {
    if let Some(uid) = card_uid(card) {
        if let Ok(uuid) = Uuid::parse_str(&uid) {
            return Ok(uuid);
        }
    }

    let uuid = Uuid::new_v4();
    set_card_uid(card, uuid);
    Ok(uuid)
}

/// Retrieve the UID value as a string if present.
pub fn card_uid(card: &Vcard) -> Option<String> {
    match &card.uid {
        Some(TextOrUriProperty::Text(text)) => Some(text.value.clone()),
        Some(TextOrUriProperty::Uri(uri)) => Some(uri.value.to_string()),
        None => None,
    }
}

/// Set the card UID to the provided UUID value.
pub fn set_card_uid(card: &mut Vcard, uuid: Uuid) {
    let value = uuid.to_string();
    card.uid = Some(TextOrUriProperty::Text(TextProperty {
        group: None,
        value,
        parameters: None,
    }));
}

/// Update the REV property to the current UTC timestamp.
pub fn touch_rev(card: &mut Vcard) {
    let now = DateTime::now_utc();
    card.rev = Some(DateTimeProperty {
        group: None,
        value: now,
        parameters: None,
    });
}

/// Render the card to its canonical textual form.
pub fn card_to_bytes(card: &Vcard) -> Vec<u8> {
    card.to_string().into_bytes()
}