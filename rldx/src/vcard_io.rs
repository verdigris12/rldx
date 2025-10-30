use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rlibphonenumber::{region_code::RegionCode, PhoneNumber, PhoneNumberFormat, PHONE_NUMBER_UTIL};
use uuid::Uuid;
use vcard4::property::{DateTimeProperty, TextOrUriProperty, TextProperty};
use vcard4::{parse, DateTime, Uri, Vcard};

/// Representation of a parsed vCard alongside metadata derived from the
/// original source text.
#[derive(Debug, Clone)]
pub struct CardWithSource {
    pub card: Vcard,
    pub is_v4: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedCards {
    pub cards: Vec<Vcard>,
    pub changed: bool,
}

/// Parse a UTF-8 encoded vCard file into `Vcard` values.
pub fn parse_file(path: &Path, default_region: Option<&str>) -> Result<ParsedCards> {
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read vCard file at {}", path.display()))?;
    let parsed = parse_str(&input, default_region)?;
    if parsed.changed {
        write_cards_to_path(path, &parsed.cards)?;
    }
    Ok(parsed)
}

/// Parse a UTF-8 string into `Vcard` values.
pub fn parse_str(input: &str, default_region: Option<&str>) -> Result<ParsedCards> {
    let mut cards = parse(input)
        .map_err(|err| anyhow!(err))
        .context("parsing vCard data")?;
    let changed = normalize_cards(&mut cards, default_region);
    Ok(ParsedCards { cards, changed })
}

/// Parse a UTF-8 string and also capture the raw block for each vCard.
pub fn parse_str_with_source(
    input: &str,
    default_region: Option<&str>,
) -> Result<Vec<CardWithSource>> {
    let ParsedCards { cards, .. } = parse_str(input, default_region)?;
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
            CardWithSource { card, is_v4 }
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

fn normalize_cards(cards: &mut [Vcard], default_region: Option<&str>) -> bool {
    let mut changed = false;
    for card in cards {
        if normalize_card_phone_numbers(card, default_region) {
            changed = true;
        }
    }
    changed
}

fn normalize_card_phone_numbers(card: &mut Vcard, default_region: Option<&str>) -> bool {
    normalize_tel_properties(&mut card.tel, default_region)
}

fn normalize_tel_properties(props: &mut [TextOrUriProperty], default_region: Option<&str>) -> bool {
    let mut changed = false;
    for prop in props {
        match prop {
            TextOrUriProperty::Text(text) => {
                if let Some(normalized) = normalize_phone_value(&text.value, default_region) {
                    if text.value != normalized.value {
                        text.value = normalized.value;
                        changed = true;
                    }
                }
            }
            TextOrUriProperty::Uri(uri_prop) => {
                let original = uri_prop.value.to_string();
                if let Some(normalized) = normalize_phone_value(&original, default_region) {
                    if normalized.had_tel_scheme {
                        let new_uri = format!("tel:{}", normalized.value);
                        if new_uri != original {
                            if let Ok(parsed) = new_uri.parse::<Uri>() {
                                uri_prop.value = parsed;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }
    changed
}

struct NormalizedPhone {
    value: String,
    had_tel_scheme: bool,
}

fn normalize_phone_value(raw: &str, default_region: Option<&str>) -> Option<NormalizedPhone> {
    let trimmed = raw.trim();
    let (had_tel_scheme, remainder) = strip_tel_scheme(trimmed);

    if remainder.is_empty() {
        return Some(NormalizedPhone {
            value: String::new(),
            had_tel_scheme,
        });
    }

    if let Some(normalized) = parse_with_regions(remainder, default_region) {
        return Some(NormalizedPhone {
            value: normalized,
            had_tel_scheme,
        });
    }

    let fallback = if had_tel_scheme { remainder } else { trimmed };
    if fallback != raw {
        return Some(NormalizedPhone {
            value: fallback.to_string(),
            had_tel_scheme,
        });
    }

    None
}

fn parse_with_regions(input: &str, default_region: Option<&str>) -> Option<String> {
    let util = &*PHONE_NUMBER_UTIL;
    let mut candidates: Vec<&str> = Vec::new();

    if let Some(region) = default_region {
        if !region.is_empty() {
            candidates.push(region);
        }
    }

    let unknown = RegionCode::get_unknown();
    if candidates
        .iter()
        .all(|candidate| !candidate.eq_ignore_ascii_case(unknown))
    {
        candidates.push(unknown);
    }

    for region in candidates {
        if let Ok(parsed) = util.parse(input, region) {
            return Some(format_parsed_number(&parsed));
        }
    }

    None
}

fn format_parsed_number(number: &PhoneNumber) -> String {
    let mut normalized = PHONE_NUMBER_UTIL
        .format(number, PhoneNumberFormat::E164)
        .into_owned();

    if number.has_extension() {
        let ext = number.extension();
        if !ext.is_empty() {
            normalized.push_str(";ext=");
            normalized.push_str(ext);
        }
    }

    normalized
}

fn has_tel_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 4 {
        return false;
    }

    bytes[0].eq_ignore_ascii_case(&b't')
        && bytes[1].eq_ignore_ascii_case(&b'e')
        && bytes[2].eq_ignore_ascii_case(&b'l')
        && bytes[3] == b':'
}

fn strip_tel_scheme(value: &str) -> (bool, &str) {
    if has_tel_scheme(value) {
        (true, value[4..].trim())
    } else {
        (false, value)
    }
}

pub fn phone_display_value(raw: &str, default_region: Option<&str>) -> String {
    let trimmed = raw.trim();
    let (_, remainder) = strip_tel_scheme(trimmed);
    normalize_phone_value(raw, default_region)
        .map(|n| n.value)
        .unwrap_or_else(|| remainder.to_string())
}

fn write_cards_to_path(path: &Path, cards: &[Vcard]) -> Result<()> {
    let mut output = String::new();
    for (idx, card) in cards.iter().enumerate() {
        if idx > 0 {
            output.push_str("\r\n");
        }
        let mut card_text = card.to_string();
        if !card_text.ends_with("\r\n") {
            card_text.push_str("\r\n");
        }
        output.push_str(&card_text);
    }
    fs::write(path, output.as_bytes())
        .with_context(|| format!("failed to write normalized vCard to {}", path.display()))?;
    Ok(())
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
