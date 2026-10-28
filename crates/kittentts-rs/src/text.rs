//! Chat text pipeline: sanitizing, filtering, transliteration and templating.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use any_ascii::any_ascii;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::language::{self, resolve_language};
use crate::MAX_LEN;

pub const VOICES: [&str; 8] = [
    "Bella", "Jasper", "Luna", "Bruno", "Rosie", "Hugo", "Kiki", "Leo",
];

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:https?://|www\.)\S+|\b\S+\.(?:com|net|org|io|gg|co|me|tv|link|ly|app)\b\S*",
    )
    .expect("url regex")
});
static SPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("space regex"));

fn default_volume() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TtsSettings {
    /// `auto`, `en`, or `uk`.
    pub language: String,
    pub voice: String,
    pub speed: f32,
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Chat template, e.g. `"{user} says: {text}"`.
    pub template: String,
    pub blacklist: Vec<String>,
    /// Truncate messages longer than this many characters.
    pub max_len: usize,
    /// Per-viewer cooldown between spoken chat messages.
    pub cooldown_secs: u64,
    /// Transliterate non-Latin scripts to Latin so English voices can say them.
    pub transliterate: bool,
    /// Read completed gift streaks out loud.
    pub read_gifts: bool,
    pub gift_template: String,
    /// Ignore gifts worth fewer than this many diamonds.
    pub min_gift_diamonds: u64,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self {
            language: language::AUTO.into(),
            voice: "Bella".into(),
            speed: 1.0,
            volume: default_volume(),
            template: "{user} says: {text}".into(),
            blacklist: Vec::new(),
            max_len: MAX_LEN,
            cooldown_secs: 3,
            transliterate: true,
            read_gifts: true,
            gift_template: "{user} sent {count} {gift}".into(),
            min_gift_diamonds: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum FilterReason {
    Empty,
    TooShort,
    Blacklisted { word: String },
    Cooldown { remaining_ms: u64 },
    GiftsDisabled,
    GiftTooSmall,
}

impl FilterReason {
    pub fn message(&self) -> String {
        match self {
            FilterReason::Empty => "message was empty after sanitizing".into(),
            FilterReason::TooShort => "message was too short to speak".into(),
            FilterReason::Blacklisted { word } => format!("blocked by blacklist word `{word}`"),
            FilterReason::Cooldown { remaining_ms } => {
                format!("viewer is on cooldown for another {remaining_ms} ms")
            }
            FilterReason::GiftsDisabled => "gift reading is disabled".into(),
            FilterReason::GiftTooSmall => "gift is below the diamond threshold".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Prepared {
    /// The original (sanitized) comment, before any transliteration.
    pub raw: String,
    /// Final text sent to the TTS engine, including the template.
    pub spoken: String,
    /// Resolved spoken language: `en` or `uk`.
    pub language: String,
    pub truncated: bool,
    pub transliterated: bool,
}

/// Stateful filter: owns per-viewer cooldown timers.
pub struct TextPipeline {
    last_spoken: Mutex<HashMap<String, Instant>>,
}

impl Default for TextPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl TextPipeline {
    pub fn new() -> Self {
        Self {
            last_spoken: Mutex::new(HashMap::new()),
        }
    }

    pub fn reset_cooldowns(&self) {
        self.last_spoken.lock().expect("cooldowns").clear();
    }

    pub fn prepare_chat(
        &self,
        settings: &TtsSettings,
        user: &str,
        unique_id: &str,
        text: &str,
    ) -> Result<Prepared, FilterReason> {
        self.enforce_cooldown(settings, unique_id, Instant::now())?;

        let (raw, truncated) = clean_text(text, settings.max_len);
        if raw.is_empty() {
            return Err(FilterReason::Empty);
        }
        if raw.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
            return Err(FilterReason::TooShort);
        }
        if let Some(word) = blacklisted(&raw, &settings.blacklist) {
            return Err(FilterReason::Blacklisted { word });
        }

        let language = resolve_language(&settings.language, &raw);
        let rendered = render_template(&settings.template, user, unique_id, &raw);
        let (spoken, transliterated) =
            maybe_transliterate(&rendered, language, settings.transliterate);
        Ok(Prepared {
            raw,
            spoken,
            language: language.into(),
            truncated,
            transliterated,
        })
    }

    pub fn prepare_gift(
        &self,
        settings: &TtsSettings,
        user: &str,
        unique_id: &str,
        gift: &str,
        count: u64,
        diamonds: u64,
    ) -> Result<Prepared, FilterReason> {
        if !settings.read_gifts {
            return Err(FilterReason::GiftsDisabled);
        }
        if diamonds < settings.min_gift_diamonds {
            return Err(FilterReason::GiftTooSmall);
        }
        let line = settings
            .gift_template
            .replace("{user}", user)
            .replace("{username}", unique_id)
            .replace("{gift}", gift)
            .replace("{count}", &count.to_string());
        let line = SPACE_RE.replace_all(line.trim(), " ").to_string();
        if line.is_empty() {
            return Err(FilterReason::Empty);
        }
        let language = resolve_language(&settings.language, &line);
        let (spoken, transliterated) = maybe_transliterate(&line, language, settings.transliterate);
        Ok(Prepared {
            raw: format!("{gift} x{count}"),
            spoken,
            language: language.into(),
            truncated: false,
            transliterated,
        })
    }

    fn enforce_cooldown(
        &self,
        settings: &TtsSettings,
        unique_id: &str,
        now: Instant,
    ) -> Result<(), FilterReason> {
        let cooldown = Duration::from_secs(settings.cooldown_secs);
        if cooldown.is_zero() || unique_id.is_empty() {
            return Ok(());
        }
        let mut timers = self.last_spoken.lock().expect("cooldowns");
        if let Some(last) = timers.get(unique_id) {
            let elapsed = now.saturating_duration_since(*last);
            if elapsed < cooldown {
                return Err(FilterReason::Cooldown {
                    remaining_ms: (cooldown - elapsed).as_millis() as u64,
                });
            }
        }
        timers.insert(unique_id.to_string(), now);
        // Keep the map from growing without bound on huge chats.
        if timers.len() > 4096 {
            timers.retain(|_, at| now.saturating_duration_since(*at) < cooldown);
        }
        Ok(())
    }
}

/// Transliterates only when a message will be spoken by the English
/// KittenTTS voice; Ukrainian text is always kept as written.
fn maybe_transliterate(line: &str, language: &str, transliterate: bool) -> (String, bool) {
    if transliterate && language == language::ENGLISH && !line.is_ascii() {
        let ascii = any_ascii(line);
        if !ascii.trim().is_empty() {
            return (ascii, true);
        }
    }
    (line.to_string(), false)
}

/// Strips URLs, emojis, zero-width junk and spam runs, then truncates.
/// The original script is preserved (no transliteration here).
/// Returns the cleaned text and whether it was truncated.
pub fn clean_text(text: &str, max_len: usize) -> (String, bool) {
    let mut cleaned = URL_RE.replace_all(text, " ").into_owned();
    cleaned.retain(|ch| {
        !matches!(ch as u32,
            0x1F000..=0x1FAFF  // emoji & pictographs
            | 0x2600..=0x27BF   // misc symbols & dingbats
            | 0x2B00..=0x2BFF   // misc symbols and arrows
            | 0xFE00..=0xFE0F   // variation selectors
            | 0x200B..=0x200F   // zero-width and direction marks
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0xFEFF            // BOM
        ) && !ch.is_control()
    });
    cleaned = collapse_repeats(&cleaned);
    cleaned = SPACE_RE.replace_all(cleaned.trim(), " ").into_owned();
    cleaned = cleaned
        .trim_matches(|c: char| c == '"' || c == '\'')
        .trim()
        .to_string();

    let limit = max_len.max(1);
    let mut truncated = false;
    if cleaned.chars().count() > limit {
        truncated = true;
        let mut cut: String = cleaned.chars().take(limit).collect();
        if let Some(idx) = cut.rfind(' ') {
            if idx > 0 && idx as f32 > limit as f32 * 0.6 {
                cut.truncate(idx);
            }
        }
        cleaned = cut.trim_end_matches([' ', ',', ';', ':']).to_string();
    }

    (cleaned, truncated)
}

/// Collapses runs of three or more identical characters to two
/// (`"heyyyy" -> "heyy"`), which tames spam without breaking `"hmm"` or `"!!"`.
fn collapse_repeats(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last: Option<char> = None;
    let mut run = 0usize;
    for ch in input.chars() {
        if Some(ch) == last {
            run += 1;
        } else {
            last = Some(ch);
            run = 1;
        }
        if run <= 2 {
            out.push(ch);
        }
    }
    out
}

/// Backwards-compatible stateless sanitizer.
pub fn sanitize(text: &str, max_len: usize) -> String {
    clean_text(text, max_len).0
}

fn blacklisted(text: &str, blacklist: &[String]) -> Option<String> {
    if blacklist.is_empty() {
        return None;
    }
    let haystack = text.to_lowercase();
    for word in blacklist {
        let needle = word.trim().to_lowercase();
        if needle.is_empty() {
            continue;
        }
        if haystack.contains(&needle) {
            return Some(needle);
        }
    }
    None
}

pub fn render_template(template: &str, user: &str, unique_id: &str, text: &str) -> String {
    if template.trim().is_empty() {
        return text.to_string();
    }
    let mut out = template
        .replace("{user}", user)
        .replace("{username}", unique_id)
        .replace("{text}", text);
    if !template.contains("{text}") {
        out.push(' ');
        out.push_str(text);
    }
    SPACE_RE.replace_all(out.trim(), " ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline() -> TextPipeline {
        TextPipeline::new()
    }

    #[test]
    fn strips_urls_emojis_and_spam() {
        let (text, _) = clean_text(
            "hi 😂😂 check https://spam.example/x and blah.com now!!!!!!!",
            MAX_LEN,
        );
        assert!(!text.contains("http"));
        assert!(!text.contains("😂"));
        assert!(!text.contains("!!!!"));
        assert!(text.contains("hi"));
        assert!(text.contains("now!!"));
    }

    #[test]
    fn truncates_at_word_boundary() {
        let long = "word ".repeat(100);
        let (text, truncated) = clean_text(&long, 20);
        assert!(truncated);
        assert!(text.chars().count() <= 20);
        assert!(!text.ends_with(' '));
    }

    #[test]
    fn cooldown_blocks_repeat_chat() {
        let p = pipeline();
        let settings = TtsSettings::default();
        assert!(p
            .prepare_chat(&settings, "Bo", "bo", "first message")
            .is_ok());
        match p.prepare_chat(&settings, "Bo", "bo", "second message") {
            Err(FilterReason::Cooldown { .. }) => {}
            other => panic!("expected cooldown, got {other:?}"),
        }
    }

    #[test]
    fn blacklist_matches_case_insensitively() {
        let p = pipeline();
        let settings = TtsSettings {
            cooldown_secs: 0,
            blacklist: vec!["badword".into()],
            ..Default::default()
        };
        match p.prepare_chat(&settings, "Bo", "bo", "this has BadWord inside") {
            Err(FilterReason::Blacklisted { word }) => assert_eq!(word, "badword"),
            other => panic!("expected blacklist, got {other:?}"),
        }
    }

    #[test]
    fn auto_routes_cyrillic_to_ukrainian_without_transliteration() {
        let p = pipeline();
        let settings = TtsSettings {
            cooldown_secs: 0,
            ..Default::default()
        };
        let prepared = p
            .prepare_chat(&settings, "Іван", "ivan", "Привіт, як справи?")
            .unwrap();
        assert_eq!(prepared.language, language::UKRAINIAN);
        assert!(!prepared.transliterated);
        assert!(prepared.raw.contains("Привіт"), "raw: {}", prepared.raw);
        assert!(prepared.spoken.contains("Привіт"));
        assert!(prepared.spoken.contains("says:"));
    }

    #[test]
    fn english_keeps_original_display_but_transliterates_speech() {
        let p = pipeline();
        let settings = TtsSettings {
            cooldown_secs: 0,
            language: language::ENGLISH.into(),
            ..Default::default()
        };
        let prepared = p
            .prepare_chat(&settings, "Иван", "ivan", "Привет как дела")
            .unwrap();
        assert_eq!(prepared.language, language::ENGLISH);
        assert!(prepared.transliterated);
        assert!(prepared.raw.contains("Привет"), "raw: {}", prepared.raw);
        assert!(prepared.spoken.contains("says:"));
        assert!(prepared.spoken.is_ascii(), "spoken: {}", prepared.spoken);
    }

    #[test]
    fn template_without_text_slot_appends() {
        assert_eq!(
            render_template("{user} says:", "Bo", "bo", "hello"),
            "Bo says: hello"
        );
    }
}
