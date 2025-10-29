use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use uuid::Uuid;
use vcard4::parameter::{Parameters, TimeZoneParameter, TypeParameter};
use vcard4::property::{DateTimeProperty, Property, TextOrUriProperty};
use vcard4::Vcard;

use crate::db::{IndexedItem, IndexedProp};
use crate::vcard_io;
use crate::vdir::FileState;

#[derive(Debug, Clone)]
pub struct FnVariant {
    pub value: String,
    pub language: Option<String>,
    pub pref: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct IndexedRecord {
    pub item: IndexedItem,
    pub props: Vec<IndexedProp>,
    pub aliases: Vec<String>,
    pub fn_variants: Vec<FnVariant>,
}

pub fn build_record(
    path: &Path,
    card: &Vcard,
    state: &FileState,
    preferred_language: Option<&str>,
) -> Result<IndexedRecord> {
    let uuid_str = vcard_io::card_uid(card)
        .ok_or_else(|| anyhow!("card missing UID: {}", path.display()))?;
    let uuid = Uuid::parse_str(&uuid_str)
        .map_err(|_| anyhow!("card UID is not a UUID: {}", uuid_str))?;

    let (display_fn, display_lang, fn_variants) = select_display_fn(card, preferred_language);

    let rev = card.rev.as_ref().map(|rev| date_time_property_to_string(rev));
    let has_photo = !card.photo.is_empty();
    let has_logo = !card.logo.is_empty();

    let mut counters = HashMap::<String, i64>::new();
    let mut props: Vec<IndexedProp> = Vec::new();

    collect_fn_props(card, &display_fn, &mut counters, &mut props);
    collect_name_props(card, &mut counters, &mut props);
    collect_nickname_props(card, &mut counters, &mut props);
    collect_org_props(card, &mut counters, &mut props);
    collect_title_role_props(card, &mut counters, &mut props);
    collect_email_props(card, &mut counters, &mut props);
    collect_tel_props(card, &mut counters, &mut props);
    collect_address_props(card, &mut counters, &mut props);
    collect_url_props(card, &mut counters, &mut props);
    collect_note_props(card, &mut counters, &mut props);
    collect_related_props(card, &mut counters, &mut props);
    collect_photo_props(card, &mut counters, &mut props);
    collect_logo_props(card, &mut counters, &mut props);
    collect_misc_props(card, &mut counters, &mut props);
    collect_extension_props(card, &mut counters, &mut props);

    let aliases = compute_aliases(card, &display_fn);

    let item = IndexedItem {
        uuid: uuid.to_string(),
        path: path.to_path_buf(),
        display_fn: display_fn.clone(),
        rev,
        has_photo,
        has_logo,
        sha1: state.sha1.clone(),
        mtime: state.mtime,
        lang_pref: display_lang,
    };

    Ok(IndexedRecord {
        item,
        props,
        aliases,
        fn_variants,
    })
}

fn select_display_fn(
    card: &Vcard,
    preferred_language: Option<&str>,
) -> (String, Option<String>, Vec<FnVariant>) {
    let mut variants = Vec::new();
    let mut best_index: Option<usize> = None;
    let mut best_pref: u8 = u8::MAX;
    let mut best_lang_match = false;

    for (idx, prop) in card.formatted_name.iter().enumerate() {
        let pref = prop
            .parameters
            .as_ref()
            .and_then(|p| p.pref)
            .unwrap_or(u8::MAX);
        let lang = prop
            .parameters
            .as_ref()
            .and_then(|p| p.language.clone());
        let lang_match = preferred_language
            .map(|pref_lang| lang.as_deref().map(|l| l.eq_ignore_ascii_case(pref_lang)).unwrap_or(false))
            .unwrap_or(false);

        variants.push(FnVariant {
            value: prop.value.clone(),
            language: lang.clone(),
            pref: prop.parameters.as_ref().and_then(|p| p.pref),
        });

        let replace = match best_index {
            None => true,
            Some(_) if pref < best_pref => true,
            Some(_) if pref == best_pref && lang_match && !best_lang_match => true,
            _ => false,
        };

        if replace {
            best_index = Some(idx);
            best_pref = pref;
            best_lang_match = lang_match;
        }
    }

    let default_name = "Unnamed".to_string();
    if let Some(index) = best_index {
        let prop = &card.formatted_name[index];
        let language = prop
            .parameters
            .as_ref()
            .and_then(|p| p.language.clone());
        (prop.value.clone(), language, variants)
    } else {
        (default_name, None, variants)
    }
}

fn collect_fn_props(
    card: &Vcard,
    display_fn: &str,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.formatted_name {
        push_prop(
            props,
            counters,
            "FN",
            prop.value.clone(),
            prop.parameters.as_ref(),
            display_fn,
        );
    }
}

fn collect_name_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    if let Some(name) = &card.name {
        let value = name.value.join(";");
        push_prop(
            props,
            counters,
            "N",
            value,
            name.parameters.as_ref(),
            card
                .formatted_name
                .first()
                .map(|p| p.value.as_str())
                .unwrap_or(""),
        );
    }
}

fn collect_nickname_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.nickname {
        push_prop(
            props,
            counters,
            "NICKNAME",
            prop.value.clone(),
            prop.parameters.as_ref(),
            &prop.value,
        );
    }
}

fn collect_org_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.org {
        let value = prop.value.join(";");
        push_prop(
            props,
            counters,
            "ORG",
            value,
            prop.parameters.as_ref(),
            prop.value.first().map(|s| s.as_str()).unwrap_or(""),
        );
    }
}

fn collect_title_role_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.title {
        push_prop(
            props,
            counters,
            "TITLE",
            prop.value.clone(),
            prop.parameters.as_ref(),
            &prop.value,
        );
    }
    for prop in &card.role {
        push_prop(
            props,
            counters,
            "ROLE",
            prop.value.clone(),
            prop.parameters.as_ref(),
            &prop.value,
        );
    }
}

fn collect_email_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.email {
        push_prop(
            props,
            counters,
            "EMAIL",
            prop.value.clone(),
            prop.parameters.as_ref(),
            &prop.value,
        );
    }
}

fn collect_tel_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.tel {
        match prop {
            TextOrUriProperty::Text(text) => push_prop(
                props,
                counters,
                "TEL",
                text.value.clone(),
                text.parameters.as_ref(),
                &text.value,
            ),
            TextOrUriProperty::Uri(uri) => push_prop(
                props,
                counters,
                "TEL",
                uri.value.to_string(),
                uri.parameters.as_ref(),
                &uri.value.to_string(),
            ),
        }
    }
}

fn collect_address_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.address {
        let value = prop
            .parameters
            .as_ref()
            .and_then(|p| p.label.clone())
            .unwrap_or_else(|| prop.value.to_string());
        push_prop(
            props,
            counters,
            "ADR",
            value,
            prop.parameters.as_ref(),
            "",
        );
    }
}

fn collect_url_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.url {
        push_prop(
            props,
            counters,
            "URL",
            prop.value.to_string(),
            prop.parameters.as_ref(),
            &prop.value.to_string(),
        );
    }
}

fn collect_note_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.note {
        push_prop(
            props,
            counters,
            "NOTE",
            prop.value.clone(),
            prop.parameters.as_ref(),
            &prop.value,
        );
    }
}

fn collect_related_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.related {
        match prop {
            TextOrUriProperty::Text(text) => push_prop(
                props,
                counters,
                "RELATED",
                text.value.clone(),
                text.parameters.as_ref(),
                &text.value,
            ),
            TextOrUriProperty::Uri(uri) => push_prop(
                props,
                counters,
                "RELATED",
                uri.value.to_string(),
                uri.parameters.as_ref(),
                &uri.value.to_string(),
            ),
        }
    }
}

fn collect_photo_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.photo {
        match prop {
            TextOrUriProperty::Text(text) => push_prop(
                props,
                counters,
                "PHOTO",
                text.value.clone(),
                text.parameters.as_ref(),
                &text.value,
            ),
            TextOrUriProperty::Uri(uri) => push_prop(
                props,
                counters,
                "PHOTO",
                uri.value.to_string(),
                uri.parameters.as_ref(),
                &uri.value.to_string(),
            ),
        }
    }
}

fn collect_logo_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for prop in &card.logo {
        push_prop(
            props,
            counters,
            "LOGO",
            prop.value.to_string(),
            prop.parameters.as_ref(),
            &prop.value.to_string(),
        );
    }
}

fn collect_misc_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    if let Some(kind) = &card.kind {
        push_prop(
            props,
            counters,
            "KIND",
            kind.value.to_string(),
            kind.parameters.as_ref(),
            &kind.value.to_string(),
        );
    }
    if let Some(bday) = &card.bday {
        push_prop(
            props,
            counters,
            "BDAY",
            bday.to_string(),
            bday.parameters(),
            &bday.to_string(),
        );
    }
    if let Some(anniv) = &card.anniversary {
        push_prop(
            props,
            counters,
            "ANNIVERSARY",
            anniv.to_string(),
            anniv.parameters(),
            &anniv.to_string(),
        );
    }
    for prop in &card.categories {
        let value = prop.value.join(",");
        push_prop(
            props,
            counters,
            "CATEGORIES",
            value,
            prop.parameters.as_ref(),
            "",
        );
    }
    if let Some(gender) = &card.gender {
        push_prop(
            props,
            counters,
            "GENDER",
            gender.value.to_string(),
            gender.parameters.as_ref(),
            &gender.value.to_string(),
        );
    }
    for prop in &card.impp {
        push_prop(
            props,
            counters,
            "IMPP",
            prop.value.to_string(),
            prop.parameters.as_ref(),
            &prop.value.to_string(),
        );
    }
    for prop in &card.member {
        push_prop(
            props,
            counters,
            "MEMBER",
            prop.value.to_string(),
            prop.parameters.as_ref(),
            &prop.value.to_string(),
        );
    }
}

fn collect_extension_props(
    card: &Vcard,
    counters: &mut HashMap<String, i64>,
    props: &mut Vec<IndexedProp>,
) {
    for ext in &card.extensions {
        let value = ext.value.to_string();
        let field = ext.name.to_uppercase();
        push_prop(
            props,
            counters,
            &field,
            value,
            ext.parameters.as_ref(),
            "",
        );
    }
}

fn push_prop(
    props: &mut Vec<IndexedProp>,
    counters: &mut HashMap<String, i64>,
    field: &str,
    value: String,
    parameters: Option<&Parameters>,
    _fn_value: &str,
) {
    let seq = next_seq(counters, field);
    props.push(IndexedProp {
        field: field.to_string(),
        value,
        params: parameters_to_json(parameters),
        seq,
    });
    // fn_value preserved via items table
}

fn next_seq(counters: &mut HashMap<String, i64>, field: &str) -> i64 {
    let entry = counters.entry(field.to_string()).or_insert(0);
    let seq = *entry;
    *entry += 1;
    seq
}

fn parameters_to_json(parameters: Option<&Parameters>) -> Value {
    let mut object = Map::new();
    if let Some(params) = parameters {
        if let Some(language) = &params.language {
            object.insert("language".to_string(), Value::String(language.clone()));
        }
        if let Some(value_type) = &params.value {
            object.insert("value".to_string(), Value::String(value_type.to_string()));
        }
        if let Some(pref) = params.pref {
            object.insert("pref".to_string(), Value::from(pref));
        }
        if let Some(alt_id) = &params.alt_id {
            object.insert("alt_id".to_string(), Value::String(alt_id.clone()));
        }
        if let Some(pid) = &params.pid {
            let values = pid.iter().map(|p| p.to_string()).collect::<Vec<_>>();
            object.insert("pid".to_string(), Value::Array(values.into_iter().map(Value::String).collect()));
        }
        if let Some(types) = &params.types {
            let values = types
                .iter()
                .map(|t| Value::String(type_parameter_to_string(t)))
                .collect::<Vec<_>>();
            object.insert("type".to_string(), Value::Array(values));
        }
        if let Some(media_type) = &params.media_type {
            object.insert("media_type".to_string(), Value::String(media_type.clone()));
        }
        if let Some(calscale) = &params.calscale {
            object.insert("calscale".to_string(), Value::String(calscale.clone()));
        }
        if let Some(sort_as) = &params.sort_as {
            object.insert(
                "sort_as".to_string(),
                Value::Array(sort_as.clone().into_iter().map(Value::String).collect()),
            );
        }
        if let Some(geo) = &params.geo {
            object.insert("geo".to_string(), Value::String(geo.to_string()));
        }
        if let Some(tz) = &params.timezone {
            object.insert("timezone".to_string(), Value::String(timezone_to_string(tz)));
        }
        if let Some(label) = &params.label {
            object.insert("label".to_string(), Value::String(label.clone()));
        }
        if let Some(extensions) = &params.extensions {
            let values = extensions
                .iter()
                .map(|(name, vals)| json!({"name": name, "values": vals}))
                .collect();
            object.insert("extensions".to_string(), Value::Array(values));
        }
    }
    Value::Object(object)
}

fn type_parameter_to_string(param: &TypeParameter) -> String {
    match param {
        TypeParameter::Telephone(value) => value.to_string(),
        TypeParameter::Related(value) => value.to_string(),
        TypeParameter::Home => "home".to_string(),
        TypeParameter::Work => "work".to_string(),
        TypeParameter::Extension(value) => format!("X-{}", value),
    }
}

fn timezone_to_string(param: &TimeZoneParameter) -> String {
    match param {
        TimeZoneParameter::Text(value) => value.clone(),
        TimeZoneParameter::Uri(value) => value.to_string(),
        TimeZoneParameter::UtcOffset(offset) => offset.to_string(),
    }
}

fn compute_aliases(card: &Vcard, display_fn: &str) -> Vec<String> {
    let mut aliases = Vec::new();
    for prop in &card.nickname {
        if !prop.value.is_empty() {
            aliases.push(prop.value.clone());
        }
    }
    for prop in &card.formatted_name {
        if prop.value != display_fn && !prop.value.is_empty() {
            aliases.push(prop.value.clone());
        }
    }
    for ext in &card.extensions {
        let upper = ext.name.to_uppercase();
        if upper.contains("NAME") || upper.contains("DISPLAY") {
            let value = ext.value.to_string();
            if !value.is_empty() {
                aliases.push(value);
            }
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn date_time_property_to_string(prop: &DateTimeProperty) -> String {
    prop.to_string()
}