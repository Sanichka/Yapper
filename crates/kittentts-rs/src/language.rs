//! Spoken-language selection and voice routing.
//!
//! KittenTTS 0.8 is English-only. Ukrainian chat is routed to Microsoft Edge
//! neural voices, and `auto` picks a language per message from its script.

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

/// Resolves `auto` to a concrete language for one message.
pub fn resolve_language(setting: &str, text: &str) -> &'static str {
    match normalize_language(setting) {
        AUTO => detect_language(text),
        language => language,
    }
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

pub fn is_cyrillic(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0400..=0x052F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F
    )
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
