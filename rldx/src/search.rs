pub fn normalize_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_lowercase())
    }
}

pub fn like_pattern(normalized: &str) -> String {
    let escaped = normalized
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{}%", escaped)
}