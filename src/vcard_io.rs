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

/// Delete a field entry by field name and sequence number
pub fn delete_card_field(card: &mut Vcard, field: &str, seq: usize) -> bool {
    match field.to_ascii_uppercase().as_str() {
        "TEL" => {
            if seq < card.tel.len() {
                card.tel.remove(seq);
                true
            } else {
                false
            }
        }
        "EMAIL" => {
            if seq < card.email.len() {
                card.email.remove(seq);
                true
            } else {
                false
            }
        }
        "NICKNAME" => delete_nickname_entry(card, seq),
        "NOTE" => {
            if seq < card.note.len() {
                card.note.remove(seq);
                true
            } else {
                false
            }
        }
        "URL" => {
            if seq < card.url.len() {
                card.url.remove(seq);
                true
            } else {
                false
            }
        }
        "ADR" => {
            if seq < card.address.len() {
                card.address.remove(seq);
                true
            } else {
                false
            }
        }
        "ORG" => {
            if seq < card.org.len() {
                card.org.remove(seq);
                true
            } else {
                false
            }
        }
        "TITLE" => {
            if seq < card.title.len() {
                card.title.remove(seq);
                true
            } else {
                false
            }
        }
        "ROLE" => {
            if seq < card.role.len() {
                card.role.remove(seq);
                true
            } else {
                false
            }
        }
        "IMPP" => {
            if seq < card.impp.len() {
                card.impp.remove(seq);
                true
            } else {
                false
            }
        }
        "PHOTO" => {
            if seq < card.photo.len() {
                card.photo.remove(seq);
                true
            } else {
                false
            }
        }
        _ => {
            // Try extension properties
            let field_upper = field.to_ascii_uppercase();
            let mut found_idx = None;
            let mut current_seq = 0usize;
            for (idx, ext) in card.extensions.iter().enumerate() {
                if ext.name.eq_ignore_ascii_case(&field_upper) {
                    if current_seq == seq {
                        found_idx = Some(idx);
                        break;
                    }
                    current_seq += 1;
                }
            }
            if let Some(idx) = found_idx {
                card.extensions.remove(idx);
                true
            } else {
                false
            }
        }
    }
}

/// Add a new field to the card
pub fn add_card_field(
    card: &mut Vcard,
    field: &str,
    value: &str,
    type_param: Option<&str>,
) -> bool {
    use vcard4::property::{TextProperty, UriProperty};
    use vcard4::parameter::{Parameters, TypeParameter};
    
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return false;
    }
    
    // Build parameters if type is specified
    let parameters = type_param.map(|t| {
        let mut params = Parameters::default();
        // Parse type parameter
        let type_val = match t.to_ascii_lowercase().as_str() {
            "work" => TypeParameter::Work,
            "home" => TypeParameter::Home,
            "cell" | "mobile" => TypeParameter::Telephone(vcard4::parameter::TelephoneType::Cell),
            "voice" => TypeParameter::Telephone(vcard4::parameter::TelephoneType::Voice),
            "fax" => TypeParameter::Telephone(vcard4::parameter::TelephoneType::Fax),
            "pager" => TypeParameter::Telephone(vcard4::parameter::TelephoneType::Pager),
            "text" => TypeParameter::Telephone(vcard4::parameter::TelephoneType::Text),
            "video" => TypeParameter::Telephone(vcard4::parameter::TelephoneType::Video),
            _ => TypeParameter::Extension(t.to_string()),
        };
        params.types = Some(vec![type_val]);
        params
    });
    
    match field.to_ascii_uppercase().as_str() {
        "TEL" => {
            card.tel.push(vcard4::property::TextOrUriProperty::Text(TextProperty {
                group: None,
                value: trimmed,
                parameters,
            }));
            true
        }
        "EMAIL" => {
            card.email.push(TextProperty {
                group: None,
                value: trimmed,
                parameters,
            });
            true
        }
        "NICKNAME" => {
            card.nickname.push(TextProperty {
                group: None,
                value: trimmed,
                parameters: None,
            });
            true
        }
        "NOTE" => {
            card.note.push(TextProperty {
                group: None,
                value: trimmed,
                parameters: None,
            });
            true
        }
        "URL" => {
            if let Ok(uri) = trimmed.parse::<Uri>() {
                card.url.push(UriProperty {
                    group: None,
                    value: uri,
                    parameters,
                });
                true
            } else {
                false
            }
        }
        "ORG" => {
            card.org.push(vcard4::property::TextListProperty::new_semi_colon(vec![trimmed]));
            true
        }
        "TITLE" => {
            card.title.push(TextProperty {
                group: None,
                value: trimmed,
                parameters: None,
            });
            true
        }
        "ROLE" => {
            card.role.push(TextProperty {
                group: None,
                value: trimmed,
                parameters: None,
            });
            true
        }
        _ if field.to_ascii_uppercase().starts_with("X-") => {
            // Extension property
            card.extensions.push(vcard4::property::ExtensionProperty {
                group: None,
                name: field.to_ascii_uppercase(),
                value: vcard4::property::AnyProperty::Text(trimmed),
                parameters: None,
            });
            true
        }
        _ => false,
    }
}

/// Set the PHOTO property with a data URI
pub fn set_photo(card: &mut Vcard, data_uri: &str) {
    use vcard4::property::{TextOrUriProperty, UriProperty};
    
    // Clear existing photos
    card.photo.clear();
    
    // Parse as URI - data URIs are valid URIs
    if let Ok(uri) = data_uri.parse::<Uri>() {
        card.photo.push(TextOrUriProperty::Uri(UriProperty {
            group: None,
            value: uri,
            parameters: None,
        }));
    }
}

/// Delete all PHOTO properties
pub fn delete_photo(card: &mut Vcard) {
    card.photo.clear();
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

// =============================================================================
// Merge functionality
// =============================================================================

/// Merge multiple vCards into a single card.
/// The first card is used as the base, and properties from subsequent cards
/// are merged in.
pub fn merge_cards(cards: Vec<Vcard>) -> Option<Vcard> {
    if cards.is_empty() {
        return None;
    }
    
    let mut cards = cards;
    let mut merged = cards.remove(0);
    
    for other in cards {
        merged = merge_two_cards(merged, other);
    }
    
    Some(merged)
}

/// Merge two vCards into one.
/// Card `a` is the base; properties from `b` are added if not already present.
pub fn merge_two_cards(mut a: Vcard, b: Vcard) -> Vcard {
    // FN: use longer FN, add shorter as nickname
    let a_fn = a.formatted_name.first().map(|p| p.value.clone()).unwrap_or_default();
    let b_fn = b.formatted_name.first().map(|p| p.value.clone()).unwrap_or_default();
    
    let a_fn_trimmed = a_fn.trim();
    let b_fn_trimmed = b_fn.trim();
    
    if !b_fn_trimmed.is_empty() {
        if b_fn_trimmed.len() > a_fn_trimmed.len() {
            // B's FN is longer - use it as primary, A's FN becomes nickname
            if let Some(b_fn_prop) = b.formatted_name.first() {
                // Replace A's FN with B's
                if a.formatted_name.is_empty() {
                    a.formatted_name.push(b_fn_prop.clone());
                } else {
                    a.formatted_name[0] = b_fn_prop.clone();
                }
            }
            // Add A's FN as nickname if not already present
            if !a_fn_trimmed.is_empty() {
                add_nickname_if_unique(&mut a, a_fn_trimmed);
            }
        } else if !a_fn_trimmed.eq_ignore_ascii_case(b_fn_trimmed) {
            // A's FN is longer or equal but different - add B's FN as nickname
            add_nickname_if_unique(&mut a, b_fn_trimmed);
        }
    }

    // N: component-wise merge with conflict detection
    // Components: [0]=family, [1]=given, [2]=additional, [3]=prefix, [4]=suffix
    let mut had_conflict = false;
    
    if let Some(ref b_name) = b.name {
        if a.name.is_none() {
            // A has no name, use B's entirely
            a.name = Some(b_name.clone());
        } else if let Some(ref mut a_name) = a.name {
            // Both have names - merge component by component
            // Ensure both have 5 components
            while a_name.value.len() < 5 {
                a_name.value.push(String::new());
            }
            
            for i in 0..5 {
                let a_comp = a_name.value.get(i).map(|s| s.trim()).unwrap_or("");
                let b_comp = b_name.value.get(i).map(|s| s.trim()).unwrap_or("");
                
                if a_comp.is_empty() && !b_comp.is_empty() {
                    // A is empty, use B
                    a_name.value[i] = b_comp.to_string();
                } else if !a_comp.is_empty() && !b_comp.is_empty() 
                    && !a_comp.eq_ignore_ascii_case(b_comp) 
                {
                    // Conflict: both have values and they differ
                    had_conflict = true;
                    // Keep A's value (already there)
                }
            }
            
            // If there was a conflict, add B's full name as alias
            if had_conflict {
                let b_display = name_to_display_string(b_name);
                if !b_display.is_empty() {
                    add_nickname_if_unique(&mut a, &b_display);
                }
            }
        }
    }

    // NICKNAME: merge uniques from b
    for nick in b.nickname.iter() {
        let val = nick.value.trim();
        if !val.is_empty() {
            add_nickname_if_unique(&mut a, val);
        }
    }

    // Append remaining list properties
    a.photo.extend(b.photo.clone());
    if b.bday.is_some() && a.bday.is_none() {
        a.bday = b.bday.clone();
    }
    if b.anniversary.is_some() && a.anniversary.is_none() {
        a.anniversary = b.anniversary.clone();
    }
    if b.gender.is_some() && a.gender.is_none() {
        a.gender = b.gender.clone();
    }
    a.url.extend(b.url.clone());
    a.address.extend(b.address.clone());
    a.tel.extend(b.tel.clone());
    a.email.extend(b.email.clone());
    a.impp.extend(b.impp.clone());
    a.lang.extend(b.lang.clone());
    a.title.extend(b.title.clone());
    a.role.extend(b.role.clone());
    a.logo.extend(b.logo.clone());
    a.org.extend(b.org.clone());
    a.member.extend(b.member.clone());
    a.related.extend(b.related.clone());
    a.timezone.extend(b.timezone.clone());
    a.geo.extend(b.geo.clone());
    a.categories.extend(b.categories.clone());
    a.note.extend(b.note.clone());
    if a.prod_id.is_none() {
        a.prod_id = b.prod_id.clone();
    }
    a.sound.extend(b.sound.clone());
    a.key.extend(b.key.clone());
    a.fburl.extend(b.fburl.clone());
    a.cal_adr_uri.extend(b.cal_adr_uri.clone());
    a.cal_uri.extend(b.cal_uri.clone());
    a.extensions.extend(b.extensions.clone());

    a
}

/// Add a nickname to the card if not already present (case-insensitive check)
fn add_nickname_if_unique(card: &mut Vcard, nickname: &str) {
    let dominated = std::iter::once(
        card.formatted_name.first().map(|p| p.value.as_str()).unwrap_or("")
    ).chain(card.nickname.iter().map(|p| p.value.as_str()));
    
    if !eq_ignore_ascii_case_any(nickname, dominated) {
        card.nickname.push(TextProperty {
            group: None,
            value: nickname.to_string(),
            parameters: None,
        });
    }
}

/// Convert a structured name (N property) to a display string.
/// Format: "prefix given additional family suffix" (skipping empty components)
fn name_to_display_string(name: &vcard4::property::TextListProperty) -> String {
    // N components: [0]=family, [1]=given, [2]=additional, [3]=prefix, [4]=suffix
    // Display order: prefix given additional family suffix
    let family = name.value.get(0).map(|s| s.trim()).unwrap_or("");
    let given = name.value.get(1).map(|s| s.trim()).unwrap_or("");
    let additional = name.value.get(2).map(|s| s.trim()).unwrap_or("");
    let prefix = name.value.get(3).map(|s| s.trim()).unwrap_or("");
    let suffix = name.value.get(4).map(|s| s.trim()).unwrap_or("");
    
    // Build in display order: prefix given additional family suffix
    let parts: Vec<&str> = [prefix, given, additional, family, suffix]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    
    parts.join(" ")
}

/// Result of a merge operation
pub struct MergeResult {
    /// The merged vCard
    pub card: Vcard,
    /// The path where the merged card was written
    pub path: std::path::PathBuf,
}

/// Merge multiple vCard files into a single encrypted file.
/// 
/// - Parses all input files
/// - Merges cards (first card is base)
/// - Generates new UID and REV
/// - Writes to target directory with correct encrypted extension
/// - Returns the merged card and output path
pub fn merge_vcard_files(
    paths: &[std::path::PathBuf],
    target_dir: &std::path::Path,
    provider: &dyn CryptoProvider,
    phone_region: Option<&str>,
) -> Result<MergeResult> {
    use crate::vdir;
    
    if paths.len() < 2 {
        anyhow::bail!("need at least 2 files to merge");
    }
    
    // Parse all cards
    let mut cards: Vec<Vcard> = Vec::new();
    for path in paths {
        let parsed = parse_file(path, phone_region, provider)?;
        if let Some(card) = parsed.cards.into_iter().next() {
            cards.push(card);
        }
    }
    
    if cards.len() < 2 {
        anyhow::bail!("could not parse at least 2 cards");
    }
    
    // Merge
    let mut merged = merge_cards(cards).ok_or_else(|| anyhow!("merge failed"))?;
    
    // Ensure UID and REV
    let uuid = ensure_uuid_uid(&mut merged)?;
    touch_rev(&mut merged);
    
    // Determine target path with correct extension
    let mut used = vdir::existing_stems(target_dir)?;
    let stem = vdir::select_filename(&uuid, &mut used, None);
    let target = vdir::vcf_target_path(target_dir, &stem, provider.encryption_type());
    
    // Write encrypted
    write_cards(&target, &[merged.clone()], provider)?;
    
    Ok(MergeResult {
        card: merged,
        path: target,
    })
}

fn eq_ignore_ascii_case_any<'a, I>(needle: &str, hay: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    for item in hay {
        if needle.eq_ignore_ascii_case(item) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use tempfile::TempDir;
    use crate::crypto::AgeProvider;

    fn make_card(fn_name: &str, email: Option<&str>, tel: Option<&str>) -> Vcard {
        let vcard_str = format!(
            r#"BEGIN:VCARD
VERSION:4.0
FN:{}
UID:{}
{}{}END:VCARD"#,
            fn_name,
            uuid::Uuid::new_v4(),
            email.map(|e| format!("EMAIL:{}\r\n", e)).unwrap_or_default(),
            tel.map(|t| format!("TEL:{}\r\n", t)).unwrap_or_default(),
        );
        parse_str(&vcard_str, None).unwrap().cards.into_iter().next().unwrap()
    }

    #[test]
    fn test_merge_two_cards_combines_emails() {
        let card1 = make_card("Alice Smith", Some("alice@example.com"), None);
        let card2 = make_card("Alice S.", Some("alice@work.com"), None);

        let merged = merge_two_cards(card1, card2);

        // Should have both emails
        assert_eq!(merged.email.len(), 2);
        let emails: Vec<_> = merged.email.iter().map(|e| e.value.to_string()).collect();
        assert!(emails.contains(&"alice@example.com".to_string()));
        assert!(emails.contains(&"alice@work.com".to_string()));

        // First FN kept, second FN added as nickname
        assert_eq!(merged.formatted_name.len(), 1);
        assert_eq!(merged.formatted_name[0].value, "Alice Smith");
        assert!(merged.nickname.iter().any(|n| n.value == "Alice S."));
    }

    #[test]
    fn test_merge_two_cards_combines_phones() {
        let card1 = make_card("Bob Jones", None, Some("+1234567890"));
        let card2 = make_card("Robert Jones", None, Some("+0987654321"));

        let merged = merge_two_cards(card1, card2);

        // Should have both phones
        assert_eq!(merged.tel.len(), 2);
    }

    #[test]
    fn test_merge_cards_multiple() {
        let cards = vec![
            make_card("Person A", Some("a@test.com"), None),
            make_card("Person B", Some("b@test.com"), None),
            make_card("Person C", Some("c@test.com"), None),
        ];

        let merged = merge_cards(cards).unwrap();

        // Should have all 3 emails
        assert_eq!(merged.email.len(), 3);
        
        // First FN kept
        assert_eq!(merged.formatted_name[0].value, "Person A");
        
        // Other FNs added as nicknames
        assert_eq!(merged.nickname.len(), 2);
    }

    #[test]
    fn test_merge_cards_empty_returns_none() {
        let result = merge_cards(vec![]);
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_vcard_files_creates_encrypted_file() {
        let temp_dir = TempDir::new().unwrap();
        let vdir = temp_dir.path();

        // Create age provider with ephemeral key
        let provider = AgeProvider::new_ephemeral(vdir).unwrap();

        // Create two vCard files
        let card1 = make_card("Test User 1", Some("user1@test.com"), None);
        let card2 = make_card("Test User 2", Some("user2@test.com"), None);

        let path1 = crate::vdir::vcf_target_path(vdir, "contact1", provider.encryption_type());
        let path2 = crate::vdir::vcf_target_path(vdir, "contact2", provider.encryption_type());

        write_cards(&path1, &[card1], &provider).unwrap();
        write_cards(&path2, &[card2], &provider).unwrap();

        // Merge
        let result = merge_vcard_files(
            &[path1.clone(), path2.clone()],
            vdir,
            &provider,
            None,
        ).unwrap();

        // Verify: output file has .vcf.age extension
        assert!(
            result.path.to_string_lossy().ends_with(".vcf.age"),
            "merged file should have .vcf.age extension, got: {}",
            result.path.display()
        );

        // Verify: output file can be decrypted
        let parsed = parse_file(&result.path, None, &provider).unwrap();
        assert_eq!(parsed.cards.len(), 1);

        // Verify: merged card has both emails
        let merged_card = &parsed.cards[0];
        assert_eq!(merged_card.email.len(), 2);
    }

    #[test]
    fn test_merge_vcard_files_preserves_data() {
        let temp_dir = TempDir::new().unwrap();
        let vdir = temp_dir.path();

        let provider = AgeProvider::new_ephemeral(vdir).unwrap();

        // Create cards with distinct data
        let mut card1 = make_card("Main Contact", Some("main@test.com"), Some("+111"));
        card1.note.push(vcard4::property::TextProperty {
            group: None,
            value: "Important note".to_string(),
            parameters: None,
        });

        let card2 = make_card("Secondary", Some("secondary@test.com"), Some("+222"));

        let path1 = crate::vdir::vcf_target_path(vdir, "main", provider.encryption_type());
        let path2 = crate::vdir::vcf_target_path(vdir, "secondary", provider.encryption_type());

        write_cards(&path1, &[card1], &provider).unwrap();
        write_cards(&path2, &[card2], &provider).unwrap();

        let result = merge_vcard_files(&[path1, path2], vdir, &provider, None).unwrap();

        // Parse and verify
        let parsed = parse_file(&result.path, None, &provider).unwrap();
        let merged = &parsed.cards[0];

        // Main FN preserved
        assert_eq!(merged.formatted_name[0].value, "Main Contact");

        // Both emails
        assert_eq!(merged.email.len(), 2);

        // Both phones
        assert_eq!(merged.tel.len(), 2);

        // Note preserved
        assert_eq!(merged.note.len(), 1);
        assert_eq!(merged.note[0].value, "Important note");

        // Secondary FN as nickname
        assert!(merged.nickname.iter().any(|n| n.value == "Secondary"));
    }

    // =========================================================================
    // FN merge tests - use longer FN
    // =========================================================================

    #[test]
    fn test_fn_merge_uses_longer_fn() {
        let card1 = make_card("John", None, None);
        let card2 = make_card("John William Smith", None, None);

        let merged = merge_two_cards(card1, card2);

        // Longer FN should be primary
        assert_eq!(merged.formatted_name[0].value, "John William Smith");
        // Shorter FN should be nickname
        assert!(merged.nickname.iter().any(|n| n.value == "John"));
    }

    #[test]
    fn test_fn_merge_keeps_longer_a() {
        let card1 = make_card("Dr. John William Smith Jr.", None, None);
        let card2 = make_card("John Smith", None, None);

        let merged = merge_two_cards(card1, card2);

        // A's FN is longer, should be kept
        assert_eq!(merged.formatted_name[0].value, "Dr. John William Smith Jr.");
        // B's FN should be nickname
        assert!(merged.nickname.iter().any(|n| n.value == "John Smith"));
    }

    #[test]
    fn test_fn_merge_equal_length_same_no_nickname() {
        let card1 = make_card("John Smith", None, None);
        let card2 = make_card("John Smith", None, None);

        let merged = merge_two_cards(card1, card2);

        // Same FN, no nickname added
        assert_eq!(merged.formatted_name[0].value, "John Smith");
        assert!(merged.nickname.is_empty());
    }

    #[test]
    fn test_fn_merge_equal_length_case_insensitive() {
        let card1 = make_card("John Smith", None, None);
        let card2 = make_card("JOHN SMITH", None, None);

        let merged = merge_two_cards(card1, card2);

        // Same FN (case-insensitive), no nickname added
        assert_eq!(merged.formatted_name[0].value, "John Smith");
        assert!(merged.nickname.is_empty());
    }

    #[test]
    fn test_fn_merge_strips_whitespace_for_comparison() {
        let card1 = make_card("  John  ", None, None);
        let card2 = make_card("John William Smith", None, None);

        let merged = merge_two_cards(card1, card2);

        // "John William Smith" is longer than "John" (after trimming)
        assert_eq!(merged.formatted_name[0].value, "John William Smith");
    }

    // =========================================================================
    // N (structured name) merge tests
    // =========================================================================

    fn make_card_with_name(fn_name: &str, name_components: &[&str]) -> Vcard {
        // N components: family;given;additional;prefix;suffix
        let n_value = name_components.join(";");
        let vcard_str = format!(
            r#"BEGIN:VCARD
VERSION:4.0
FN:{}
N:{}
UID:{}
END:VCARD"#,
            fn_name,
            n_value,
            uuid::Uuid::new_v4(),
        );
        parse_str(&vcard_str, None).unwrap().cards.into_iter().next().unwrap()
    }

    #[test]
    fn test_n_merge_fills_empty_components() {
        // A has only given name, B has family name
        let card1 = make_card_with_name("John", &["", "John", "", "", ""]);
        let card2 = make_card_with_name("John Smith", &["Smith", "", "", "", ""]);

        let merged = merge_two_cards(card1, card2);

        // Should have both family and given filled
        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(0).map(|s| s.as_str()), Some("Smith")); // family from B
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));  // given from A
    }

    #[test]
    fn test_n_merge_fills_prefix_suffix() {
        let card1 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);
        let card2 = make_card_with_name("Dr. John Smith Jr.", &["Smith", "John", "William", "Dr.", "Jr."]);

        let merged = merge_two_cards(card1, card2);

        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(0).map(|s| s.as_str()), Some("Smith"));   // family
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));    // given
        assert_eq!(name.value.get(2).map(|s| s.as_str()), Some("William")); // additional from B
        assert_eq!(name.value.get(3).map(|s| s.as_str()), Some("Dr."));     // prefix from B
        assert_eq!(name.value.get(4).map(|s| s.as_str()), Some("Jr."));     // suffix from B
    }

    #[test]
    fn test_n_merge_conflict_keeps_a_adds_alias() {
        // Conflict on given name: John vs Jonathan
        // Use same-length FNs to avoid FN swap complicating the test
        let card1 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);
        let card2 = make_card_with_name("Jon. Smith", &["Smith", "Jonathan", "", "", ""]);

        let merged = merge_two_cards(card1, card2);

        // A's FN kept (same length, A wins), B's FN added as nickname
        assert_eq!(merged.formatted_name[0].value, "John Smith");
        assert!(merged.nickname.iter().any(|n| n.value == "Jon. Smith"));

        // A's given name kept in N
        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));

        // B's full N as alias: "Jonathan Smith" (different from FN "Jon. Smith")
        assert!(
            merged.nickname.iter().any(|n| n.value == "Jonathan Smith"),
            "Should have 'Jonathan Smith' as nickname, got: {:?}",
            merged.nickname.iter().map(|n| &n.value).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_n_merge_conflict_full_name_alias() {
        // A: Smith;John;;;
        // B: Jones;Jonathan;William;Dr.;Jr.
        // B's FN is longer, so it becomes primary FN
        let card1 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);
        let card2 = make_card_with_name("Dr. Jonathan William Jones Jr.", &["Jones", "Jonathan", "William", "Dr.", "Jr."]);

        let merged = merge_two_cards(card1, card2);

        // B's FN is longer, so it becomes primary
        assert_eq!(merged.formatted_name[0].value, "Dr. Jonathan William Jones Jr.");
        // A's FN added as nickname
        assert!(merged.nickname.iter().any(|n| n.value == "John Smith"));

        // A's values kept where present (A is base for N merge)
        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(0).map(|s| s.as_str()), Some("Smith")); // A's family
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));  // A's given

        // B's values used for empty components
        assert_eq!(name.value.get(2).map(|s| s.as_str()), Some("William")); // B's additional
        assert_eq!(name.value.get(3).map(|s| s.as_str()), Some("Dr."));     // B's prefix
        assert_eq!(name.value.get(4).map(|s| s.as_str()), Some("Jr."));     // B's suffix

        // B's N-based alias would be "Dr. Jonathan William Jones Jr." but it matches FN
        // so it's not added as duplicate. Instead we should have A's FN as nickname.
        // The conflict is recorded, but the alias happens to match FN.
        // This is correct behavior - no duplicate FN in nicknames.
    }

    #[test]
    fn test_n_merge_no_conflict_no_alias() {
        // Same names, different components filled - no conflict
        let card1 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);
        let card2 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);

        let merged = merge_two_cards(card1, card2);

        // No nickname from N merge (only from FN if different)
        // Since FNs are same, no nicknames at all
        assert!(merged.nickname.is_empty());
    }

    #[test]
    fn test_n_merge_case_insensitive_no_conflict() {
        let card1 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);
        let card2 = make_card_with_name("John Smith", &["SMITH", "JOHN", "", "", ""]);

        let merged = merge_two_cards(card1, card2);

        // Same names case-insensitively, no conflict
        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(0).map(|s| s.as_str()), Some("Smith")); // A's case preserved
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));

        // No alias added
        assert!(merged.nickname.is_empty());
    }

    #[test]
    fn test_n_merge_b_has_no_name() {
        let card1 = make_card_with_name("John Smith", &["Smith", "John", "", "", ""]);
        let card2 = make_card("Johnny", None, None); // No N property

        let merged = merge_two_cards(card1, card2);

        // A's name preserved
        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(0).map(|s| s.as_str()), Some("Smith"));
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));
    }

    #[test]
    fn test_n_merge_a_has_no_name() {
        let card1 = make_card("John", None, None); // No N property
        let card2 = make_card_with_name("John Smith", &["Smith", "John", "W", "Dr.", "Jr."]);

        let merged = merge_two_cards(card1, card2);

        // B's name used entirely
        let name = merged.name.as_ref().unwrap();
        assert_eq!(name.value.get(0).map(|s| s.as_str()), Some("Smith"));
        assert_eq!(name.value.get(1).map(|s| s.as_str()), Some("John"));
        assert_eq!(name.value.get(2).map(|s| s.as_str()), Some("W"));
        assert_eq!(name.value.get(3).map(|s| s.as_str()), Some("Dr."));
        assert_eq!(name.value.get(4).map(|s| s.as_str()), Some("Jr."));
    }

    #[test]
    fn test_name_to_display_string() {
        // Test the helper function directly
        let name = vcard4::property::TextListProperty {
            group: None,
            value: vec![
                "Smith".to_string(),     // family
                "John".to_string(),      // given
                "William".to_string(),   // additional
                "Dr.".to_string(),       // prefix
                "Jr.".to_string(),       // suffix
            ],
            parameters: None,
            delimiter: vcard4::property::TextListDelimiter::SemiColon,
        };

        let display = name_to_display_string(&name);
        assert_eq!(display, "Dr. John William Smith Jr.");
    }

    #[test]
    fn test_name_to_display_string_skips_empty() {
        let name = vcard4::property::TextListProperty {
            group: None,
            value: vec![
                "Smith".to_string(),  // family
                "John".to_string(),   // given
                "".to_string(),       // additional (empty)
                "".to_string(),       // prefix (empty)
                "".to_string(),       // suffix (empty)
            ],
            parameters: None,
            delimiter: vcard4::property::TextListDelimiter::SemiColon,
        };

        let display = name_to_display_string(&name);
        assert_eq!(display, "John Smith");
    }
}
