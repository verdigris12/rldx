use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rlibphonenumber::{region_code::RegionCode, PhoneNumber, PhoneNumberFormat, PHONE_NUMBER_UTIL};
use uuid::Uuid;
use vcard4::parameter::Parameters;
use vcard4::property::{DateTimeProperty, TextListProperty, TextOrUriProperty, TextProperty};
use vcard4::{parse, DateTime, Uri, Vcard};

use crate::crypto::CryptoProvider;
use crate::translit;

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
/// Decrypts the file using the provided crypto provider.
pub fn parse_file(
    path: &Path,
    default_region: Option<&str>,
    provider: &dyn CryptoProvider,
) -> Result<ParsedCards> {
    // Read and decrypt file
    let encrypted = fs::read(path)
        .with_context(|| format!("failed to read vCard file at {}", path.display()))?;
    let decrypted = provider
        .decrypt(&encrypted)
        .with_context(|| format!("failed to decrypt vCard file at {}", path.display()))?;
    let input = String::from_utf8(decrypted)
        .with_context(|| format!("vCard file {} contains invalid UTF-8", path.display()))?;

    let parsed = parse_str(&input, default_region)?;
    if parsed.changed {
        write_cards(path, &parsed.cards, provider)?;
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

/// Write cards to an encrypted file using the given provider
pub fn write_cards(path: &Path, cards: &[Vcard], provider: &dyn CryptoProvider) -> Result<()> {
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
    let encrypted = provider
        .encrypt(output.as_bytes())
        .with_context(|| format!("failed to encrypt vCard for {}", path.display()))?;
    crate::vdir::write_atomic(path, &encrypted)
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

pub fn update_card_field(
    card: &mut Vcard,
    field: &str,
    seq: i64,
    component: Option<usize>,
    new_value: &str,
    default_region: Option<&str>,
) -> Result<bool> {
    match field.to_ascii_uppercase().as_str() {
        "TEL" => update_tel_value(card, seq, new_value, default_region),
        "EMAIL" => Ok(update_email_value(card, seq, new_value)),
        "FN" => Ok(update_fn_value(card, seq, new_value)),
        "N" => Ok(update_n_value(card, component, new_value)),
        "NICKNAME" => Ok(update_nickname_value(card, seq, new_value)),
        _ => Ok(false),
    }
}

pub fn promote_tel_entry(card: &mut Vcard, index: usize) -> bool {
    if index >= card.tel.len() {
        return false;
    }

    if index == 0 {
        return true;
    }

    let entry = card.tel.remove(index);
    card.tel.insert(0, entry);
    true
}

pub fn promote_email_entry(card: &mut Vcard, index: usize) -> bool {
    if index >= card.email.len() {
        return false;
    }

    if index == 0 {
        return true;
    }

    let entry = card.email.remove(index);
    card.email.insert(0, entry);
    true
}

fn update_tel_value(
    card: &mut Vcard,
    seq: i64,
    new_value: &str,
    default_region: Option<&str>,
) -> Result<bool> {
    let normalized = normalize_phone_value(new_value, default_region)
        .map(|n| n.value)
        .unwrap_or_else(|| new_value.trim().to_string());

    let mut index = 0;
    for prop in &mut card.tel {
        if index == seq {
            match prop {
                TextOrUriProperty::Text(text) => {
                    text.value = normalized.clone();
                }
                TextOrUriProperty::Uri(uri_prop) => {
                    let uri_text = format!("tel:{}", normalized);
                    let parsed = uri_text
                        .parse::<Uri>()
                        .with_context(|| format!("invalid telephone URI: {}", uri_text))?;
                    uri_prop.value = parsed;
                }
            }
            return Ok(true);
        }
        index += 1;
    }

    Ok(false)
}

fn update_email_value(card: &mut Vcard, seq: i64, new_value: &str) -> bool {
    let trimmed = new_value.trim();
    let mut index = 0;
    for prop in &mut card.email {
        if index == seq {
            prop.value = trimmed.to_string();
            return true;
        }
        index += 1;
    }
    false
}

fn update_fn_value(card: &mut Vcard, seq: i64, new_value: &str) -> bool {
    if seq < 0 {
        return false;
    }

    let idx = match usize::try_from(seq) {
        Ok(value) => value,
        Err(_) => return false,
    };

    let trimmed = new_value.trim().to_string();
    if card.formatted_name.len() <= idx {
        card.formatted_name.resize_with(idx + 1, || TextProperty {
            group: None,
            value: String::new(),
            parameters: None,
        });
    }

    if let Some(prop) = card.formatted_name.get_mut(idx) {
        prop.value = trimmed;
        true
    } else {
        false
    }
}

fn update_n_value(card: &mut Vcard, component: Option<usize>, new_value: &str) -> bool {
    let Some(index) = component else {
        return false;
    };

    let trimmed = new_value.trim().to_string();
    let entry = card
        .name
        .get_or_insert_with(|| TextListProperty::new_semi_colon(Vec::new()));

    if entry.value.len() <= index {
        entry.value.resize(index + 1, String::new());
    }

    entry.value[index] = trimmed;
    true
}

fn update_nickname_value(card: &mut Vcard, seq: i64, new_value: &str) -> bool {
    let trimmed = new_value.trim().to_string();
    let mut index = 0i64;
    for prop in &mut card.nickname {
        if index == seq {
            prop.value = trimmed;
            return true;
        }
        index += 1;
    }
    false
}

/// Delete a nickname entry by index
pub fn delete_nickname_entry(card: &mut Vcard, index: usize) -> bool {
    if index >= card.nickname.len() {
        return false;
    }
    card.nickname.remove(index);
    true
}

// =============================================================================
// Transliteration support (ALTID/LANGUAGE)
// =============================================================================

/// Find the maximum ALTID value currently used in a card.
/// Returns 0 if no ALTID is found.
fn max_altid(card: &Vcard) -> u32 {
    let mut max = 0u32;

    // Check FN properties
    for prop in &card.formatted_name {
        if let Some(params) = &prop.parameters {
            if let Some(alt_id) = &params.alt_id {
                if let Ok(n) = alt_id.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }

    // Check N property
    if let Some(prop) = &card.name {
        if let Some(params) = &prop.parameters {
            if let Some(alt_id) = &params.alt_id {
                if let Ok(n) = alt_id.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }

    // Check NICKNAME properties
    for prop in &card.nickname {
        if let Some(params) = &prop.parameters {
            if let Some(alt_id) = &params.alt_id {
                if let Ok(n) = alt_id.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }

    // Check ORG properties
    for prop in &card.org {
        if let Some(params) = &prop.parameters {
            if let Some(alt_id) = &params.alt_id {
                if let Ok(n) = alt_id.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }

    max
}

/// Check if a list of TextProperty has a Latin alternative with given ALTID.
fn has_latin_alternative_text(props: &[TextProperty], alt_id: &str) -> bool {
    for prop in props {
        if let Some(params) = &prop.parameters {
            if params.alt_id.as_deref() == Some(alt_id) && translit::is_all_latin(&prop.value) {
                return true;
            }
        }
    }
    false
}

/// Check if a list of TextListProperty has a Latin alternative with given ALTID.
fn has_latin_alternative_list(props: &[TextListProperty], alt_id: &str) -> bool {
    for prop in props {
        if let Some(params) = &prop.parameters {
            let combined: String = prop.value.join(" ");
            if params.alt_id.as_deref() == Some(alt_id) && translit::is_all_latin(&combined) {
                return true;
            }
        }
    }
    false
}

/// Create parameters with ALTID and optionally LANGUAGE.
fn make_params(alt_id: &str, language: Option<&str>, pref: Option<u8>) -> Parameters {
    let mut params = Parameters::default();
    params.alt_id = Some(alt_id.to_string());
    params.language = language.map(|s| s.to_string());
    params.pref = pref;
    params
}

/// Apply transliteration to FN (formatted name) properties.
/// Returns true if any changes were made.
fn transliterate_fn(card: &mut Vcard, next_altid: &mut u32) -> bool {
    // First pass: collect info about what needs transliteration
    let mut to_process: Vec<(usize, String, Option<String>)> = Vec::new();

    for (idx, prop) in card.formatted_name.iter().enumerate() {
        let value = &prop.value;

        if !translit::needs_transliteration(value) {
            continue;
        }

        let existing_altid = prop
            .parameters
            .as_ref()
            .and_then(|p| p.alt_id.as_ref())
            .cloned();

        // Check if Latin alternative already exists
        if let Some(ref alt_id) = existing_altid {
            if has_latin_alternative_text(&card.formatted_name, alt_id) {
                continue;
            }
        }

        to_process.push((idx, value.clone(), existing_altid));
    }

    if to_process.is_empty() {
        return false;
    }

    // Second pass: apply changes
    let mut new_props: Vec<TextProperty> = Vec::new();
    let mut processed_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (idx, value, existing_altid) in &to_process {
        processed_indices.insert(*idx);

        let script = translit::detect_non_latin_script(value);
        let lang = script.map(translit::script_to_lang);
        let transliterated = translit::transliterate(value);

        let alt_id = existing_altid.clone().unwrap_or_else(|| {
            let id = next_altid.to_string();
            *next_altid += 1;
            id
        });

        // Clone original and update with ALTID and LANGUAGE
        let mut orig = card.formatted_name[*idx].clone();
        orig.parameters = Some(make_params(&alt_id, lang, Some(1)));
        new_props.push(orig.clone());

        // Add transliterated Latin version
        new_props.push(TextProperty {
            group: orig.group.clone(),
            value: transliterated,
            parameters: Some(make_params(&alt_id, None, Some(2))),
        });
    }

    // Add unprocessed properties
    for (idx, prop) in card.formatted_name.iter().enumerate() {
        if !processed_indices.contains(&idx) {
            new_props.push(prop.clone());
        }
    }

    card.formatted_name = new_props;
    true
}

/// Apply transliteration to N (name) property.
/// Returns true if any changes were made.
fn transliterate_n(card: &mut Vcard, next_altid: &mut u32) -> bool {
    let Some(ref mut name_prop) = card.name else {
        return false;
    };

    // Combine all name components to check if transliteration is needed
    let combined: String = name_prop.value.iter().cloned().collect::<Vec<_>>().join(" ");
    if !translit::needs_transliteration(&combined) {
        return false;
    }

    // Check if already has ALTID with Latin alternative
    // For N, we can't easily check alternatives since there's only one N property
    // So we check if the property already has ALTID + LANGUAGE set
    if let Some(ref params) = name_prop.parameters {
        if params.alt_id.is_some() && params.language.is_some() {
            // Already processed
            return false;
        }
    }

    // Detect language
    let script = translit::detect_non_latin_script(&combined);
    let lang = script.map(translit::script_to_lang);

    // Assign ALTID
    let alt_id = name_prop
        .parameters
        .as_ref()
        .and_then(|p| p.alt_id.clone())
        .unwrap_or_else(|| {
            let id = next_altid.to_string();
            *next_altid += 1;
            id
        });

    // Update with ALTID and LANGUAGE
    let params = name_prop
        .parameters
        .get_or_insert_with(Parameters::default);
    params.alt_id = Some(alt_id);
    params.language = lang.map(|s| s.to_string());

    // Note: For N property, we don't add a separate Latin alternative
    // because vCard typically has only one N property. The transliteration
    // is used for FN which is the display name.
    true
}

/// Apply transliteration to NICKNAME properties (Vec<TextProperty>).
/// Returns true if any changes were made.
fn transliterate_nickname(card: &mut Vcard, next_altid: &mut u32) -> bool {
    // First pass: collect info
    let mut to_process: Vec<(usize, String, Option<String>)> = Vec::new();

    for (idx, prop) in card.nickname.iter().enumerate() {
        let value = &prop.value;

        if !translit::needs_transliteration(value) {
            continue;
        }

        let existing_altid = prop
            .parameters
            .as_ref()
            .and_then(|p| p.alt_id.as_ref())
            .cloned();

        if let Some(ref alt_id) = existing_altid {
            if has_latin_alternative_text(&card.nickname, alt_id) {
                continue;
            }
        }

        to_process.push((idx, value.clone(), existing_altid));
    }

    if to_process.is_empty() {
        return false;
    }

    // Second pass: apply changes
    let mut new_props: Vec<TextProperty> = Vec::new();
    let mut processed_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (idx, value, existing_altid) in &to_process {
        processed_indices.insert(*idx);

        let script = translit::detect_non_latin_script(value);
        let lang = script.map(translit::script_to_lang);
        let transliterated = translit::transliterate(value);

        let alt_id = existing_altid.clone().unwrap_or_else(|| {
            let id = next_altid.to_string();
            *next_altid += 1;
            id
        });

        let mut orig = card.nickname[*idx].clone();
        orig.parameters = Some(make_params(&alt_id, lang, Some(1)));
        new_props.push(orig.clone());

        new_props.push(TextProperty {
            group: orig.group.clone(),
            value: transliterated,
            parameters: Some(make_params(&alt_id, None, Some(2))),
        });
    }

    for (idx, prop) in card.nickname.iter().enumerate() {
        if !processed_indices.contains(&idx) {
            new_props.push(prop.clone());
        }
    }

    card.nickname = new_props;
    true
}

/// Apply transliteration to ORG properties (Vec<TextListProperty>).
/// Returns true if any changes were made.
fn transliterate_org(card: &mut Vcard, next_altid: &mut u32) -> bool {
    // First pass: collect info
    let mut to_process: Vec<(usize, Vec<String>, Option<String>)> = Vec::new();

    for (idx, prop) in card.org.iter().enumerate() {
        let combined: String = prop.value.join(" ");

        if !translit::needs_transliteration(&combined) {
            continue;
        }

        let existing_altid = prop
            .parameters
            .as_ref()
            .and_then(|p| p.alt_id.as_ref())
            .cloned();

        if let Some(ref alt_id) = existing_altid {
            if has_latin_alternative_list(&card.org, alt_id) {
                continue;
            }
        }

        to_process.push((idx, prop.value.clone(), existing_altid));
    }

    if to_process.is_empty() {
        return false;
    }

    // Second pass: apply changes
    let mut new_props: Vec<TextListProperty> = Vec::new();
    let mut processed_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (idx, value, existing_altid) in &to_process {
        processed_indices.insert(*idx);

        let combined: String = value.join(" ");
        let script = translit::detect_non_latin_script(&combined);
        let lang = script.map(translit::script_to_lang);
        let transliterated: Vec<String> = value.iter().map(|s| translit::transliterate(s)).collect();

        let alt_id = existing_altid.clone().unwrap_or_else(|| {
            let id = next_altid.to_string();
            *next_altid += 1;
            id
        });

        let mut orig = card.org[*idx].clone();
        orig.parameters = Some(make_params(&alt_id, lang, Some(1)));
        new_props.push(orig.clone());

        new_props.push(TextListProperty {
            group: orig.group.clone(),
            value: transliterated,
            parameters: Some(make_params(&alt_id, None, Some(2))),
            delimiter: orig.delimiter.clone(),
        });
    }

    for (idx, prop) in card.org.iter().enumerate() {
        if !processed_indices.contains(&idx) {
            new_props.push(prop.clone());
        }
    }

    card.org = new_props;
    true
}

/// Apply transliteration to all applicable properties in a card.
/// Returns true if any changes were made.
pub fn transliterate_card(card: &mut Vcard) -> bool {
    let mut next_altid = max_altid(card) + 1;
    let mut changed = false;

    changed |= transliterate_fn(card, &mut next_altid);
    changed |= transliterate_n(card, &mut next_altid);
    changed |= transliterate_nickname(card, &mut next_altid);
    changed |= transliterate_org(card, &mut next_altid);

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transliterate_card_cyrillic_fn() {
        let vcard_str = r#"BEGIN:VCARD
VERSION:4.0
FN:Иван Петров
END:VCARD"#;

        let parsed = parse_str(vcard_str, None).unwrap();
        let mut card = parsed.cards.into_iter().next().unwrap();

        let changed = transliterate_card(&mut card);
        assert!(changed, "transliterate_card should return true");

        // Should now have 2 FN properties
        assert_eq!(card.formatted_name.len(), 2);

        // First should be original with LANGUAGE=ru
        let orig = &card.formatted_name[0];
        assert_eq!(orig.value, "Иван Петров");
        let orig_params = orig.parameters.as_ref().unwrap();
        assert_eq!(orig_params.language.as_deref(), Some("ru"));
        assert_eq!(orig_params.alt_id.as_deref(), Some("1"));
        assert_eq!(orig_params.pref, Some(1));

        // Second should be transliterated Latin
        let latin = &card.formatted_name[1];
        assert_eq!(latin.value, "Ivan Petrov");
        let latin_params = latin.parameters.as_ref().unwrap();
        assert_eq!(latin_params.language, None);
        assert_eq!(latin_params.alt_id.as_deref(), Some("1"));
        assert_eq!(latin_params.pref, Some(2));
    }

    #[test]
    fn test_transliterate_card_already_latin() {
        let vcard_str = r#"BEGIN:VCARD
VERSION:4.0
FN:John Doe
END:VCARD"#;

        let parsed = parse_str(vcard_str, None).unwrap();
        let mut card = parsed.cards.into_iter().next().unwrap();

        let changed = transliterate_card(&mut card);
        assert!(!changed, "should not change already Latin names");
        assert_eq!(card.formatted_name.len(), 1);
    }

    #[test]
    fn test_transliterate_card_mixed_script() {
        let vcard_str = r#"BEGIN:VCARD
VERSION:4.0
FN:John Иванов
END:VCARD"#;

        let parsed = parse_str(vcard_str, None).unwrap();
        let mut card = parsed.cards.into_iter().next().unwrap();

        let changed = transliterate_card(&mut card);
        assert!(changed);
        assert_eq!(card.formatted_name.len(), 2);

        // Original with detected Cyrillic -> ru
        let orig = &card.formatted_name[0];
        assert_eq!(orig.value, "John Иванов");
        let orig_params = orig.parameters.as_ref().unwrap();
        assert_eq!(orig_params.language.as_deref(), Some("ru"));

        // Transliterated
        let latin = &card.formatted_name[1];
        assert_eq!(latin.value, "John Ivanov");
    }

    #[test]
    fn test_transliterate_idempotent() {
        let vcard_str = r#"BEGIN:VCARD
VERSION:4.0
FN:Иван Петров
END:VCARD"#;

        let parsed = parse_str(vcard_str, None).unwrap();
        let mut card = parsed.cards.into_iter().next().unwrap();

        // First transliteration
        let changed1 = transliterate_card(&mut card);
        assert!(changed1);
        assert_eq!(card.formatted_name.len(), 2);

        // Second transliteration should be no-op
        let changed2 = transliterate_card(&mut card);
        assert!(!changed2, "second run should not change anything");
        assert_eq!(card.formatted_name.len(), 2);
    }
}
