use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use strsim::jaro_winkler;

use vcard4::property::TextProperty;
use vcard4::Vcard;

use super::simhash_index::{NameSource, SimHashIndex};
use crate::config::Config;
use crate::crypto::CryptoProvider;
use crate::db::{compute_simhash, Database};
use crate::search;
use crate::vcard_io;
use crate::vdir;

const BEGIN_VCARD: &str = "BEGIN:VCARD";
const END_VCARD: &str = "END:VCARD";

/// Result of Google import operation
pub struct ImportResult {
    pub imported: usize,
    pub merged: Vec<MergeInfo>,
    pub skipped: usize,
}

/// Information about a merged contact
pub struct MergeInfo {
    pub email: String,
    pub name: String,
    pub merged_into: String,
    pub score: f64,
}

pub fn import_google_contacts(
    input: &Path,
    config: &Config,
    book: Option<&str>,
    automerge_threshold: Option<f64>,
    db: &mut Database,
    provider: &dyn CryptoProvider,
) -> Result<ImportResult> {
    let content = fs::read_to_string(input).with_context(|| {
        format!(
            "failed to read Google Contacts export at {}",
            input.display()
        )
    })?;

    let cards = split_cards(&content);
    if cards.is_empty() {
        return Err(anyhow!("no vCards found in Google export"));
    }

    let target_dir = match book {
        Some(name) => config.vdir.join(name),
        None => config.vdir.clone(),
    };

    fs::create_dir_all(&target_dir).with_context(|| {
        format!(
            "failed to ensure target address book directory {}",
            target_dir.display()
        )
    })?;

    let mut used_names = vdir::existing_stems(&target_dir)?;
    let mut imported = 0usize;
    let mut merged = Vec::new();
    let mut skipped = 0usize;

    // Build SimHash index for automerge if enabled
    let simhash_index = if automerge_threshold.is_some() {
        let existing_simhashes = db.list_all_simhashes()?;
        Some(SimHashIndex::new(existing_simhashes))
    } else {
        None
    };
    let simhash_threshold = config.maildir_import.simhash_threshold;

    for (index, card_lines) in cards.iter().enumerate() {
        match convert_google_card(card_lines, config.phone_region.as_deref()) {
            Ok(mut card) => {
                // Get the FN and primary email for automerge check
                let fn_value = card
                    .formatted_name
                    .first()
                    .map(|p| p.value.clone())
                    .unwrap_or_default();

                let primary_email = card.email.first().map(|e| e.value.clone());

                // Collect nicknames from the incoming card
                let nicknames: Vec<&str> = card
                    .nickname
                    .iter()
                    .map(|n| n.value.as_str())
                    .collect();

                // Skip if primary email already exists
                if let Some(ref email) = primary_email {
                    if db.email_exists(email)? {
                        skipped += 1;
                        continue;
                    }
                }

                // Try automerge if enabled
                if let Some(threshold) = automerge_threshold {
                    if !fn_value.is_empty() {
                        if let Some(ref simhash_idx) = simhash_index {
                            if let Some((path, display_fn, score)) = find_merge_candidate(
                                simhash_idx,
                                &fn_value,
                                &nicknames,
                                threshold,
                                simhash_threshold,
                                &config.maildir_import,
                            ) {
                                // Merge into existing contact
                                if merge_card_into_existing(
                                    &path,
                                    &card,
                                    config.phone_region.as_deref(),
                                    provider,
                                )? {
                                    merged.push(MergeInfo {
                                        email: primary_email.unwrap_or_default(),
                                        name: fn_value,
                                        merged_into: display_fn,
                                        score,
                                    });
                                    continue;
                                }
                            }
                        }
                    }
                }

                // Create new contact
                let uuid = vcard_io::ensure_uuid_uid(&mut card)?;
                vcard_io::touch_rev(&mut card);

                let filename = vdir::select_filename(&uuid, &mut used_names, None);
                let path = vdir::vcf_target_path(&target_dir, &filename, provider.encryption_type());
                let bytes = vcard_io::card_to_bytes(&card);
                let encrypted = provider.encrypt(&bytes)
                    .with_context(|| format!("failed to encrypt vCard for {}", path.display()))?;
                vdir::write_atomic(&path, &encrypted)?;
                imported += 1;
            }
            Err(err) => {
                eprintln!(
                    "warning: skipping contact #{}, conversion failed: {err}",
                    index + 1
                );
                skipped += 1;
            }
        }
    }

    Ok(ImportResult {
        imported,
        merged,
        skipped,
    })
}

/// Match result with source information for preferencing
struct MatchCandidate {
    path: PathBuf,
    display_fn: String,
    score: f64,
    source: NameSource,
}

/// Find a merge candidate using SimHash BK-tree for fast lookup, then Jaro-Winkler verification.
/// Checks both the FN and nicknames against existing FNs and nicknames.
/// Prefers FN matches over nickname matches.
fn find_merge_candidate(
    index: &SimHashIndex,
    fn_value: &str,
    nicknames: &[&str],
    jw_threshold: f64,
    simhash_threshold: u32,
    import_config: &crate::config::MaildirImportConfig,
) -> Option<(PathBuf, String, f64)> {
    let mut all_candidates: Vec<MatchCandidate> = Vec::new();

    // Check FN
    if import_config.is_valid_fn_for_merge(fn_value) {
        collect_candidates(
            index,
            fn_value,
            jw_threshold,
            simhash_threshold,
            &mut all_candidates,
        );
    }

    // Check nicknames
    for nickname in nicknames {
        if import_config.is_valid_nickname_for_merge(nickname) {
            collect_candidates(
                index,
                nickname,
                jw_threshold,
                simhash_threshold,
                &mut all_candidates,
            );
        }
    }

    if all_candidates.is_empty() {
        return None;
    }

    // Sort candidates: FN matches first, then by score descending
    all_candidates.sort_by(|a, b| {
        match (&a.source, &b.source) {
            (NameSource::FN, NameSource::Nickname) => std::cmp::Ordering::Less,
            (NameSource::Nickname, NameSource::FN) => std::cmp::Ordering::Greater,
            _ => b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal),
        }
    });

    all_candidates
        .into_iter()
        .next()
        .map(|c| (c.path, c.display_fn, c.score))
}

/// Collect matching candidates for a single name
fn collect_candidates(
    index: &SimHashIndex,
    name: &str,
    jw_threshold: f64,
    simhash_threshold: u32,
    results: &mut Vec<MatchCandidate>,
) {
    let name_norm = search::normalize(name);
    let name_simhash = compute_simhash(&name_norm);

    let candidates = index.find_candidates(name_simhash, simhash_threshold);

    for entry in candidates {
        let score = jaro_winkler(&name_norm, &entry.matched_norm);

        if score >= jw_threshold {
            let dominated = results.iter().any(|r| {
                r.path == entry.path
                    && (r.source == NameSource::FN || entry.source == NameSource::Nickname)
                    && r.score >= score
            });

            if !dominated {
                results.retain(|r| {
                    r.path != entry.path
                        || (r.source == NameSource::FN && entry.source == NameSource::Nickname)
                        || r.score > score
                });

                results.push(MatchCandidate {
                    path: entry.path.clone(),
                    display_fn: entry.display_fn.clone(),
                    score,
                    source: entry.source,
                });
            }
        }
    }
}

/// Merge a card's data into an existing vCard
fn merge_card_into_existing(
    path: &Path,
    source: &Vcard,
    default_region: Option<&str>,
    provider: &dyn CryptoProvider,
) -> Result<bool> {
    let parsed = vcard_io::parse_file(path, default_region, provider)?;
    let Some(mut card) = parsed.cards.into_iter().next() else {
        return Ok(false);
    };

    let mut changed = false;

    // Merge emails
    let existing_emails: HashSet<String> = card
        .email
        .iter()
        .map(|e| e.value.to_lowercase())
        .collect();

    for email in &source.email {
        if !existing_emails.contains(&email.value.to_lowercase()) {
            card.email.push(email.clone());
            changed = true;
        }
    }

    // Merge phone numbers
    let existing_phones: HashSet<String> = card
        .tel
        .iter()
        .map(|t| match t {
            vcard4::property::TextOrUriProperty::Text(p) => p.value.clone(),
            vcard4::property::TextOrUriProperty::Uri(u) => u.value.to_string(),
        })
        .collect();

    for tel in &source.tel {
        let tel_value = match tel {
            vcard4::property::TextOrUriProperty::Text(p) => p.value.clone(),
            vcard4::property::TextOrUriProperty::Uri(u) => u.value.to_string(),
        };
        if !existing_phones.contains(&tel_value) {
            card.tel.push(tel.clone());
            changed = true;
        }
    }

    // Merge nicknames
    let existing_nicknames: HashSet<String> = card
        .nickname
        .iter()
        .map(|n| n.value.to_lowercase())
        .collect();

    for nickname in &source.nickname {
        if !existing_nicknames.contains(&nickname.value.to_lowercase()) {
            card.nickname.push(nickname.clone());
            changed = true;
        }
    }

    // Merge additional FN values as nicknames (if different from primary)
    let card_fn = card
        .formatted_name
        .first()
        .map(|p| p.value.to_lowercase())
        .unwrap_or_default();

    for fn_prop in source.formatted_name.iter().skip(1) {
        let fn_lower = fn_prop.value.to_lowercase();
        if fn_lower != card_fn && !existing_nicknames.contains(&fn_lower) {
            card.nickname.push(TextProperty {
                group: None,
                value: fn_prop.value.clone(),
                parameters: None,
            });
            changed = true;
        }
    }

    if changed {
        vcard_io::touch_rev(&mut card);
        let bytes = vcard_io::card_to_bytes(&card);
        let encrypted = provider.encrypt(&bytes)
            .with_context(|| format!("failed to encrypt vCard for {}", path.display()))?;
        vdir::write_atomic(path, &encrypted)?;
    }

    Ok(changed)
}

fn split_cards(content: &str) -> Vec<Vec<String>> {
    let mut cards: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut inside = false;

    for raw_line in content.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.eq_ignore_ascii_case(BEGIN_VCARD) {
            if inside && !current.is_empty() {
                cards.push(std::mem::take(&mut current));
            }
            inside = true;
        }

        if inside {
            current.push(line.to_string());
            if line.eq_ignore_ascii_case(END_VCARD) {
                cards.push(std::mem::take(&mut current));
                inside = false;
            }
        }
    }

    if inside && !current.is_empty() {
        cards.push(current);
    }

    cards
}

fn convert_google_card(lines: &[String], default_region: Option<&str>) -> Result<Vcard> {
    let unfolded = unfold_lines(lines);

    let mut output: Vec<String> = Vec::new();
    output.push(BEGIN_VCARD.to_string());
    output.push("VERSION:4.0".to_string());

    for line in unfolded.iter() {
        if line.eq_ignore_ascii_case(BEGIN_VCARD) || line.eq_ignore_ascii_case(END_VCARD) {
            continue;
        }

        if let Some((lhs, value)) = line.split_once(':') {
            if lhs.eq_ignore_ascii_case("VERSION") {
                continue;
            }

            let converted = convert_property(lhs, value)?;
            if let Some(prop_line) = converted {
                output.push(prop_line);
            }
        }
    }

    output.push(END_VCARD.to_string());
    let joined = output.join("\r\n");

    let parsed = vcard_io::parse_str(&joined, default_region)?;
    parsed
        .cards
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("converted card failed to parse"))
}

fn unfold_lines(lines: &[String]) -> Vec<String> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in lines {
        let mut handled = false;
        if let Some(last) = unfolded.last_mut() {
            if line.starts_with(' ') || line.starts_with('\t') {
                if last.ends_with('=') && has_quoted_printable_encoding(last) {
                    last.pop();
                }
                let tail = line.trim_start_matches([' ', '\t']);
                last.push_str(tail);
                handled = true;
            } else if last.ends_with('=') && has_quoted_printable_encoding(last) {
                last.pop();
                last.push_str(line);
                handled = true;
            }
        }

        if !handled {
            unfolded.push(line.clone());
        }
    }
    unfolded
}

fn has_quoted_printable_encoding(line: &str) -> bool {
    if let Some((prefix, _)) = line.split_once(':') {
        for part in prefix.split(';').skip(1) {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some((name, value)) = trimmed.split_once('=') {
                if name.trim().eq_ignore_ascii_case("ENCODING")
                    && value.trim().eq_ignore_ascii_case("QUOTED-PRINTABLE")
                {
                    return true;
                }
            } else if trimmed.eq_ignore_ascii_case("QUOTED-PRINTABLE") {
                return true;
            }
        }
    }

    false
}

fn convert_property(lhs: &str, value: &str) -> Result<Option<String>> {
    let mut parts = lhs.split(';');
    let property_part = parts
        .next()
        .ok_or_else(|| anyhow!("property without name"))?;

    let (group, name) = split_group(property_part);
    let upper_name = name.to_ascii_uppercase();

    let mut parsed = parse_parameters(parts.collect(), &upper_name);

    let processed_value = process_value(value, &parsed)?;

    if parsed.add_pref {
        parsed
            .params
            .push(Parameter::new("PREF", vec!["1".to_string()]));
    }
    if let Some(media) = parsed.photo_media_type.clone() {
        parsed
            .params
            .push(Parameter::new("MEDIATYPE", vec![media.to_lowercase()]));
    }

    let line = format_property_line(group, &upper_name, &parsed.params, &processed_value);
    Ok(Some(line))
}

fn split_group<'a>(property: &'a str) -> (Option<&'a str>, &'a str) {
    if let Some(pos) = property.find('.') {
        let (group, name) = property.split_at(pos);
        (Some(group), &name[1..])
    } else {
        (None, property)
    }
}

#[derive(Clone, Debug)]
struct Parameter {
    name: String,
    values: Vec<String>,
}

impl Parameter {
    fn new(name: impl Into<String>, values: Vec<String>) -> Self {
        Self {
            name: name.into(),
            values,
        }
    }
}

impl ToString for Parameter {
    fn to_string(&self) -> String {
        if self.values.is_empty() {
            return self.name.clone();
        }

        let formatted_values = if self.values.len() == 1 {
            format_param_value(&self.values[0])
        } else {
            self.values
                .iter()
                .map(|v| v.trim().to_string())
                .collect::<Vec<_>>()
                .join(",")
        };

        format!("{}={}", self.name, formatted_values)
    }
}

struct ParsedParameters {
    params: Vec<Parameter>,
    add_pref: bool,
    photo_media_type: Option<String>,
    quoted_printable: bool,
    base64: bool,
}

impl ParsedParameters {
    fn new() -> Self {
        Self {
            params: Vec::new(),
            add_pref: false,
            photo_media_type: None,
            quoted_printable: false,
            base64: false,
        }
    }
}

fn parse_parameters(raw_params: Vec<&str>, property_name: &str) -> ParsedParameters {
    let mut parsed = ParsedParameters::new();

    for param in raw_params {
        let trimmed = param.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((name, value)) = trimmed.split_once('=') {
            handle_named_parameter(name, value, property_name, &mut parsed);
        } else {
            handle_positional_parameter(trimmed, property_name, &mut parsed);
        }
    }

    parsed
}

fn handle_named_parameter(
    name: &str,
    value: &str,
    property_name: &str,
    parsed: &mut ParsedParameters,
) {
    let upper_name = name.trim().to_ascii_uppercase();
    match upper_name.as_str() {
        "CHARSET" => {
            // drop
        }
        "ENCODING" => {
            let upper_value = value.trim().to_ascii_uppercase();
            if upper_value == "QUOTED-PRINTABLE" {
                parsed.quoted_printable = true;
            } else if upper_value == "B" || upper_value == "BASE64" {
                parsed.base64 = true;
            }
        }
        "TYPE" => {
            let mut values: Vec<String> = Vec::new();
            for part in value.split(',') {
                let item = part.trim();
                if item.eq_ignore_ascii_case("PREF") {
                    parsed.add_pref = true;
                    continue;
                }
                if property_name.eq_ignore_ascii_case("PHOTO") {
                    if let Some(media) = media_type_from_extension(item) {
                        parsed.photo_media_type = Some(media);
                        continue;
                    }
                }
                values.push(item.to_string());
            }
            if !values.is_empty() {
                parsed.params.push(Parameter::new("TYPE", values));
            }
        }
        "PREF" => {
            parsed.add_pref = true;
        }
        _ => {
            parsed
                .params
                .push(Parameter::new(upper_name, vec![clean_quotes(value)]));
        }
    }
}

fn handle_positional_parameter(param: &str, property_name: &str, parsed: &mut ParsedParameters) {
    if param.eq_ignore_ascii_case("PREF") {
        parsed.add_pref = true;
    } else if param.eq_ignore_ascii_case("BASE64") {
        parsed.base64 = true;
    } else if param.eq_ignore_ascii_case("QUOTED-PRINTABLE") {
        parsed.quoted_printable = true;
    } else if property_name.eq_ignore_ascii_case("PHOTO") {
        if let Some(media) = media_type_from_extension(param) {
            parsed.photo_media_type = Some(media);
        }
    } else {
        parsed
            .params
            .push(Parameter::new("TYPE", vec![param.to_string()]));
    }
}

fn process_value(value: &str, params: &ParsedParameters) -> Result<String> {
    let mut out = value.trim().to_string();

    if params.quoted_printable {
        out = decode_quoted_printable(&out)?;
    }

    if params.base64 {
        out = out.replace(['\n', '\r', ' '], "");
        if params.photo_media_type.is_some() {
            let media = params
                .photo_media_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            out = format!("data:{};base64,{}", media, out);
        }
    }

    if params.quoted_printable {
        out = out.replace('\r', "");
        out = out.replace('\n', "\\n");
    }

    Ok(out)
}

fn format_property_line(
    group: Option<&str>,
    name: &str,
    params: &[Parameter],
    value: &str,
) -> String {
    let mut buffer = String::new();
    if let Some(group) = group {
        buffer.push_str(group);
        buffer.push('.');
    }
    buffer.push_str(name);
    for param in params {
        buffer.push(';');
        buffer.push_str(&param.to_string());
    }
    buffer.push(':');
    buffer.push_str(value);
    buffer
}

fn decode_quoted_printable(input: &str) -> Result<String> {
    let mut bytes: Vec<u8> = Vec::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            '=' => {
                if i + 1 >= chars.len() {
                    // Trailing soft line break
                    break;
                }

                match chars[i + 1] {
                    '\r' => {
                        i += 2;
                        if i < chars.len() && chars[i] == '\n' {
                            i += 1;
                        }
                    }
                    '\n' => {
                        i += 2;
                    }
                    _ => {
                        if i + 2 >= chars.len() {
                            return Err(anyhow!("truncated quoted-printable escape"));
                        }

                        let a = chars[i + 1];
                        let b = chars[i + 2];
                        let value = decode_hex_pair(a, b).ok_or_else(|| {
                            anyhow!("invalid quoted-printable escape: ={}{}", a, b)
                        })?;
                        bytes.push(value);
                        i += 3;
                        continue;
                    }
                }
            }
            ch => {
                bytes.push(ch as u8);
                i += 1;
                continue;
            }
        }

        // Continue to next character after handling soft line breaks.
    }

    String::from_utf8(bytes).map_err(|err| anyhow!("invalid UTF-8 in quoted-printable: {err}"))
}

fn decode_hex_pair(a: char, b: char) -> Option<u8> {
    let high = a.to_digit(16)?;
    let low = b.to_digit(16)?;
    Some(((high << 4) | low) as u8)
}

fn clean_quotes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn format_param_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.contains(',') || trimmed.contains(';') || trimmed.contains(':') {
        let escaped = trimmed.replace('"', "\"");
        format!("\"{}\"", escaped)
    } else {
        trimmed.to_string()
    }
}

fn media_type_from_extension(value: &str) -> Option<String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "JPEG" | "JPG" => Some("image/jpeg".to_string()),
        "PNG" => Some("image/png".to_string()),
        "GIF" => Some("image/gif".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfold_lines_handles_quoted_printable_soft_breaks() {
        let lines = vec![
            "NOTE;ENCODING=QUOTED-PRINTABLE:Hello=".to_string(),
            "World".to_string(),
        ];

        let unfolded = unfold_lines(&lines);
        assert_eq!(
            unfolded,
            vec!["NOTE;ENCODING=QUOTED-PRINTABLE:HelloWorld".to_string()]
        );
    }

    #[test]
    fn unfold_lines_handles_soft_breaks_with_leading_whitespace() {
        let lines = vec![
            "NOTE;ENCODING=QUOTED-PRINTABLE:Hello=".to_string(),
            " World".to_string(),
        ];

        let unfolded = unfold_lines(&lines);
        assert_eq!(
            unfolded,
            vec!["NOTE;ENCODING=QUOTED-PRINTABLE:HelloWorld".to_string()]
        );
    }

    #[test]
    fn unfold_lines_does_not_merge_non_qp_lines() {
        let lines = vec![
            "PHOTO;ENCODING=BASE64:abc=".to_string(),
            "END:VCARD".to_string(),
        ];

        let unfolded = unfold_lines(&lines);
        assert_eq!(unfolded, lines);
    }

    #[test]
    fn decode_quoted_printable_handles_soft_breaks() {
        let decoded = decode_quoted_printable("Soft=\nBreak").unwrap();
        assert_eq!(decoded, "SoftBreak");
    }

    #[test]
    fn decode_quoted_printable_handles_trailing_equals() {
        let decoded = decode_quoted_printable("Trailing=").unwrap();
        assert_eq!(decoded, "Trailing");
    }

    #[test]
    fn decode_quoted_printable_decodes_hex_pairs() {
        let decoded = decode_quoted_printable("Line=3D1").unwrap();
        assert_eq!(decoded, "Line=1");
    }
}
