use crate::translit;

/// Normalize a string for search indexing and querying.
/// Applies lowercase and transliteration (e.g., "Иван" -> "ivan").
pub fn normalize(s: &str) -> String {
    translit::transliterate(s).to_lowercase()
}

pub fn normalize_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(normalize(trimmed))
    }
}

pub fn like_pattern(normalized: &str) -> String {
    let escaped = normalized.replace('%', "\\%").replace('_', "\\_");
    format!("%{}%", escaped)
}
