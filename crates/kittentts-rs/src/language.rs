//! Spoken-language selection and voice routing.
//!
//! KittenTTS 0.8 is English-only. Ukrainian chat is routed to Microsoft Edge
//! neural voices, and `auto` picks a language per message or splits mixed
//! messages so each language is spoken by its native voice.

pub const AUTO: &str = "auto";
pub const ENGLISH: &str = "en";
pub const UKRAINIAN: &str = "uk";

pub const UKRAINIAN_VOICES: [&str; 2] = ["uk-UA-PolinaNeural", "uk-UA-OstapNeural"];

pub fn normalize_language(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        AUTO => AUTO,
        "uk" | "ua" | "uk-ua" | "ukrainian" | "українська" => UKRAINIAN,
        _ => ENGLISH,
    }
}

/// A segment of text mapped to its language (`en` or `uk`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageSegment {
    pub language: &'static str,
    pub text: String,
}

/// Resolves `auto` to a concrete language (`en`, `uk`, or `auto` when mixed).
pub fn resolve_language(setting: &str, text: &str) -> &'static str {
    match normalize_language(setting) {
        AUTO => {
            if is_mixed(text) {
                AUTO
            } else if has_cyrillic(text) {
                UKRAINIAN
            } else {
                ENGLISH
            }
        }
        language => language,
    }
}

pub fn is_cyrillic(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0400..=0x052F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F
    )
}

pub fn has_cyrillic(text: &str) -> bool {
    text.chars().any(is_cyrillic)
}

pub fn has_latin(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_alphabetic())
}

pub fn is_mixed(text: &str) -> bool {
    has_cyrillic(text) && has_latin(text)
}

/// Splits a message into contiguous segments of English and Ukrainian text.
pub fn split_by_language(text: &str) -> Vec<LanguageSegment> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut segments: Vec<LanguageSegment> = Vec::new();
    let mut current_lang: Option<&'static str> = None;
    let mut current_words: Vec<&str> = Vec::new();

    for word in text.split_whitespace() {
        let cyr_count = word.chars().filter(|&c| is_cyrillic(c)).count();
        let lat_count = word.chars().filter(|c| c.is_ascii_alphabetic()).count();

        let word_lang = if cyr_count > 0 && lat_count > 0 {
            if cyr_count >= lat_count {
                Some(UKRAINIAN)
            } else {
                Some(ENGLISH)
            }
        } else if cyr_count > 0 {
            Some(UKRAINIAN)
        } else if lat_count > 0 {
            Some(ENGLISH)
        } else {
            None
        };

        match (current_lang, word_lang) {
            (None, Some(lang)) => {
                current_lang = Some(lang);
                current_words.push(word);
            }
            (Some(curr), Some(new_lang)) if curr != new_lang => {
                if !current_words.is_empty() {
                    segments.push(LanguageSegment {
                        language: curr,
                        text: current_words.join(" "),
                    });
                    current_words.clear();
                }
                current_lang = Some(new_lang);
                current_words.push(word);
            }
            _ => {
                current_words.push(word);
                if current_lang.is_none() && word_lang.is_some() {
                    current_lang = word_lang;
                }
            }
        }
    }

    if !current_words.is_empty() {
        let lang = current_lang.unwrap_or(ENGLISH);
        segments.push(LanguageSegment {
            language: lang,
            text: current_words.join(" "),
        });
    }

    segments
}

/// Picks Ukrainian when Cyrillic is at least as common as Latin in the text.
pub fn detect_language(text: &str) -> &'static str {
    let mut cyrillic = 0usize;
    let mut latin = 0usize;
    for ch in text.chars() {
        if is_cyrillic(ch) {
            cyrillic += 1;
        } else if ch.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cyrillic > 0 && cyrillic >= latin {
        UKRAINIAN
    } else {
        ENGLISH
    }
}

pub fn default_voice(language: &str) -> &'static str {
    match normalize_language(language) {
        UKRAINIAN => "uk-UA-PolinaNeural",
        _ => "Bella",
    }
}

pub fn voice_for_language(language: &str, voice: &str) -> String {
    let language = normalize_language(language);
    let voice = voice.trim();
    if language == UKRAINIAN {
        if voice.starts_with("uk-UA-") {
            voice.to_string()
        } else {
            default_voice(language).to_string()
        }
    } else if voice.is_empty() || voice.starts_with("uk-UA-") {
        default_voice(language).to_string()
    } else {
        voice.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cyrillic_as_ukrainian() {
        assert_eq!(detect_language("Привіт, як справи?"), UKRAINIAN);
        assert_eq!(detect_language("hello everyone"), ENGLISH);
        assert_eq!(detect_language("Привіт chat"), UKRAINIAN);
        assert_eq!(detect_language("hello привіт chat"), ENGLISH);
    }

    #[test]
    fn resolves_language_with_mixed_support() {
        assert_eq!(resolve_language(AUTO, "hello everyone"), ENGLISH);
        assert_eq!(resolve_language(AUTO, "Привіт, як справи?"), UKRAINIAN);
        assert_eq!(
            resolve_language(
                AUTO,
                "Hi Dear це тестування змішування language so it can be both languages simultaneously"
            ),
            AUTO
        );
        assert_eq!(
            resolve_language(AUTO, "Hi Dear це тестування змішування language"),
            AUTO
        );
        assert_eq!(resolve_language(ENGLISH, "Привіт"), ENGLISH);
        assert_eq!(resolve_language(UKRAINIAN, "Hello"), UKRAINIAN);
    }

    #[test]
    fn splits_mixed_sentences_correctly() {
        let segs1 = split_by_language(
            "Hi Dear це тестування змішування language so it can be both languages simultaneously",
        );
        assert_eq!(segs1.len(), 3);
        assert_eq!(segs1[0].language, ENGLISH);
        assert_eq!(segs1[0].text, "Hi Dear");
        assert_eq!(segs1[1].language, UKRAINIAN);
        assert_eq!(segs1[1].text, "це тестування змішування");
        assert_eq!(segs1[2].language, ENGLISH);
        assert_eq!(
            segs1[2].text,
            "language so it can be both languages simultaneously"
        );

        let segs2 = split_by_language("Hi Dear це тестування змішування language");
        assert_eq!(segs2.len(), 3);
        assert_eq!(segs2[0].language, ENGLISH);
        assert_eq!(segs2[0].text, "Hi Dear");
        assert_eq!(segs2[1].language, UKRAINIAN);
        assert_eq!(segs2[1].text, "це тестування змішування");
        assert_eq!(segs2[2].language, ENGLISH);
        assert_eq!(segs2[2].text, "language");
    }

    #[test]
    fn normalizes_language_names() {
        assert_eq!(normalize_language("UA"), UKRAINIAN);
        assert_eq!(normalize_language("ukrainian"), UKRAINIAN);
        assert_eq!(normalize_language("auto"), AUTO);
        assert_eq!(normalize_language("de"), ENGLISH);
    }

    #[test]
    fn routes_voices_to_matching_language() {
        assert_eq!(voice_for_language(UKRAINIAN, "Bella"), "uk-UA-PolinaNeural");
        assert_eq!(
            voice_for_language(UKRAINIAN, "uk-UA-OstapNeural"),
            "uk-UA-OstapNeural"
        );
        assert_eq!(voice_for_language(ENGLISH, "uk-UA-OstapNeural"), "Bella");
        assert_eq!(voice_for_language(ENGLISH, "Luna"), "Luna");
    }
}
