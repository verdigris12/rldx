//! Transliteration support for non-Latin scripts.
//!
//! This module provides functions to detect non-Latin scripts in text
//! and transliterate them to ASCII/Latin equivalents, enabling better
//! searchability while preserving the original text with language tags.

use deunicode::deunicode;
use unicode_script::{Script, UnicodeScript};

/// Check if a string contains only Latin characters (plus common punctuation/digits).
/// Returns true if the string is "safe" and doesn't need transliteration.
pub fn is_all_latin(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_ascii()
            || c.script() == Script::Latin
            || c.script() == Script::Common
            || c.script() == Script::Inherited
    })
}

/// Detect the first non-Latin script in a string.
/// Returns None if the string contains only Latin/Common/Inherited scripts.
pub fn detect_non_latin_script(s: &str) -> Option<Script> {
    for c in s.chars() {
        let script = c.script();
        match script {
            Script::Latin | Script::Common | Script::Inherited => continue,
            _ => return Some(script),
        }
    }
    None
}

/// Map a Unicode script to a BCP 47 language tag.
/// Uses reasonable defaults for scripts that map to multiple languages.
pub fn script_to_lang(script: Script) -> &'static str {
    match script {
        Script::Cyrillic => "ru",
        Script::Arabic => "ar",
        Script::Han => "zh",
        Script::Hiragana | Script::Katakana => "ja",
        Script::Hangul => "ko",
        Script::Greek => "el",
        Script::Hebrew => "he",
        Script::Thai => "th",
        Script::Devanagari => "hi",
        Script::Armenian => "hy",
        Script::Georgian => "ka",
        Script::Bengali => "bn",
        Script::Tamil => "ta",
        Script::Telugu => "te",
        Script::Gujarati => "gu",
        Script::Kannada => "kn",
        Script::Malayalam => "ml",
        Script::Oriya => "or",
        Script::Gurmukhi => "pa",
        Script::Sinhala => "si",
        Script::Myanmar => "my",
        Script::Khmer => "km",
        Script::Lao => "lo",
        Script::Tibetan => "bo",
        Script::Ethiopic => "am",
        _ => "und", // BCP 47 "undetermined"
    }
}

/// Transliterate a string to ASCII/Latin.
/// Uses deunicode for broad script coverage.
pub fn transliterate(s: &str) -> String {
    let result = deunicode(s);
    // Clean up: collapse multiple spaces, trim
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Check if transliteration would produce a meaningfully different result.
/// Returns false if the string is already Latin or would transliterate to itself.
pub fn needs_transliteration(s: &str) -> bool {
    if is_all_latin(s) {
        return false;
    }
    let transliterated = transliterate(s);
    // Check if transliteration actually changed anything meaningful
    !transliterated.is_empty() && transliterated != s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_all_latin() {
        assert!(is_all_latin("John Doe"));
        assert!(is_all_latin("José García"));
        assert!(is_all_latin("123-456-7890"));
        assert!(!is_all_latin("Иван Петров"));
        assert!(!is_all_latin("田中太郎"));
        assert!(!is_all_latin("John Иванов")); // Mixed
    }

    #[test]
    fn test_detect_non_latin_script() {
        assert_eq!(detect_non_latin_script("John Doe"), None);
        assert_eq!(
            detect_non_latin_script("Иван Петров"),
            Some(Script::Cyrillic)
        );
        assert_eq!(detect_non_latin_script("田中太郎"), Some(Script::Han));
        assert_eq!(
            detect_non_latin_script("John Иванов"),
            Some(Script::Cyrillic)
        );
    }

    #[test]
    fn test_script_to_lang() {
        assert_eq!(script_to_lang(Script::Cyrillic), "ru");
        assert_eq!(script_to_lang(Script::Han), "zh");
        assert_eq!(script_to_lang(Script::Arabic), "ar");
    }

    #[test]
    fn test_transliterate() {
        assert_eq!(transliterate("Иван Петров"), "Ivan Petrov");
        assert_eq!(transliterate("José García"), "Jose Garcia");
    }

    #[test]
    fn test_needs_transliteration() {
        assert!(!needs_transliteration("John Doe"));
        assert!(needs_transliteration("Иван Петров"));
        assert!(needs_transliteration("John Иванов"));
    }
}
