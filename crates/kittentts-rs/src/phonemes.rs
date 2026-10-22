//! Phonemizer abstraction and a built-in English letter-to-sound fallback.
//!
//! KittenTTS 0.8 was trained on eSpeak-NG IPA phonemes. Shipping eSpeak as a C
//! dependency is awkward on Android, so the default [`EnglishG2p`] produces IPA
//! from the same symbol set with a rule engine and a small exception lexicon.
//! Any other engine (eSpeak-NG FFI, Piper, …) can be plugged in by implementing
//! [`Phonemizer`].

use std::collections::HashMap;

use crate::error::{Result, TtsError};

pub trait Phonemizer: Send + Sync {
    fn name(&self) -> &'static str;
    /// IPA phonemes, words separated by spaces. Punctuation is preserved.
    fn phonemize(&self, text: &str) -> Result<String>;
}

pub struct EnglishG2p {
    lexicon: HashMap<&'static str, &'static str>,
}

impl Default for EnglishG2p {
    fn default() -> Self {
        Self::new()
    }
}

impl EnglishG2p {
    pub fn new() -> Self {
        Self { lexicon: lexicon() }
    }

    pub fn phonemize_words(&self, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Err(TtsError::BadText("empty text".into()));
        }

        let mut out = String::with_capacity(text.len() * 2);
        let mut word = String::new();
        let mut pending_space = false;

        let flush_word = |word: &mut String, out: &mut String, pending_space: &mut bool| {
            if word.is_empty() {
                return;
            }
            if *pending_space && !out.is_empty() {
                out.push(' ');
            }
            *pending_space = false;
            let lower = word.to_ascii_lowercase();
            let lookup = lower.as_str();
            if let Some(ipa) = self.lexicon.get(lookup) {
                out.push_str(ipa);
            } else if let Some(n) = parse_number(lookup) {
                out.push_str(&number_to_ipa(n, &self.lexicon));
            } else {
                out.push_str(&word_to_ipa(lookup));
            }
            word.clear();
        };

        for raw in text.chars() {
            if raw.is_alphanumeric() || raw == '\'' || raw == '’' {
                word.push(if raw == '’' { '\'' } else { raw });
            } else if raw.is_whitespace() {
                flush_word(&mut word, &mut out, &mut pending_space);
                pending_space = true;
            } else if let Some(punct) = map_punctuation(raw) {
                flush_word(&mut word, &mut out, &mut pending_space);
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push(punct);
                pending_space = true;
            }
        }
        flush_word(&mut word, &mut out, &mut pending_space);

        if out.trim().is_empty() {
            return Err(TtsError::BadText(
                "no pronounceable characters after filtering".into(),
            ));
        }
        Ok(out)
    }
}

impl Phonemizer for EnglishG2p {
    fn name(&self) -> &'static str {
        "english-g2p"
    }

    fn phonemize(&self, text: &str) -> Result<String> {
        self.phonemize_words(text)
    }
}

fn map_punctuation(ch: char) -> Option<char> {
    match ch {
        ',' | '.' | '!' | '?' | ';' | ':' => Some(ch),
        '…' => Some('.'),
        '—' | '–' | '-' | '/' | '(' | ')' | '[' | ']' | '"' | '“' | '”' => None,
        _ => None,
    }
}

fn parse_number(word: &str) -> Option<u64> {
    if word.is_empty() || word.len() > 9 || !word.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    word.parse().ok()
}

fn number_to_words(mut n: u64) -> String {
    const ONES: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];

    if n < 20 {
        return ONES[n as usize].to_string();
    }
    if n < 100 {
        let (t, r) = (n / 10, n % 10);
        return if r == 0 {
            TENS[t as usize].to_string()
        } else {
            format!("{} {}", TENS[t as usize], ONES[r as usize])
        };
    }
    if n < 1000 {
        let rest = n % 100;
        let head = format!("{} hundred", ONES[(n / 100) as usize]);
        return if rest == 0 {
            head
        } else {
            format!("{head} {}", number_to_words(rest))
        };
    }
    if n < 1_000_000 {
        let rest = n % 1000;
        let head = format!("{} thousand", number_to_words(n / 1000));
        return if rest == 0 {
            head
        } else {
            format!("{head} {}", number_to_words(rest))
        };
    }
    n = n.min(999_999_999);
    let rest = n % 1_000_000;
    let head = format!("{} million", number_to_words(n / 1_000_000));
    if rest == 0 {
        head
    } else {
        format!("{head} {}", number_to_words(rest))
    }
}

fn number_to_ipa(n: u64, lexicon: &HashMap<&'static str, &'static str>) -> String {
    number_to_words(n)
        .split_whitespace()
        .map(|w| {
            lexicon
                .get(w)
                .map(|ipa| ipa.to_string())
                .unwrap_or_else(|| word_to_ipa(w))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_vowel(ch: char) -> bool {
    matches!(ch, 'a' | 'e' | 'i' | 'o' | 'u')
}

fn single(ch: char, next: Option<char>, prev: Option<char>) -> &'static str {
    let soft = matches!(next, Some('e' | 'i' | 'y'));
    match ch {
        'a' => "æ",
        'b' => "b",
        'c' if soft => "s",
        'c' => "k",
        'd' => "d",
        'e' => "ɛ",
        'f' => "f",
        'g' if soft => "dʒ",
        'g' => "ɡ",
        'h' => "h",
        'i' => "ɪ",
        'j' => "dʒ",
        'k' => "k",
        'l' => "l",
        'm' => "m",
        'n' => "n",
        'o' => "ɑ",
        'p' => "p",
        'q' => "k",
        'r' => "ɹ",
        's' => {
            if prev.map(|p| !p.is_alphanumeric()).unwrap_or(true) && next.is_some() {
                "s"
            } else {
                "s"
            }
        }
        't' => "t",
        'u' => "ʌ",
        'v' => "v",
        'w' => "w",
        'x' => "ks",
        'y' if prev.is_some() && next.is_none() => "i",
        'y' => "j",
        'z' => "z",
        _ => "",
    }
}

fn word_to_ipa(word: &str) -> String {
    let mut w: Vec<char> = word
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic() || *c == '\'')
        .collect();
    if w.is_empty() {
        return String::new();
    }

    // Drop possessive / contraction apostrophes: "it's" -> "its", "don't" -> "dont".
    w.retain(|c| *c != '\'');

    let mut magic_e = false;
    if w.len() > 2 && w.last() == Some(&'e') {
        let tail: String = w[w.len().saturating_sub(3)..].iter().collect();
        if !matches!(
            tail.as_str(),
            "ee" | "ie" | "ue" | "oe" | "le" | "re" | "ve"
        ) {
            magic_e = true;
            w.pop();
        }
    }

    let mut out = String::new();
    let mut i = 0;
    while i < w.len() {
        let rest: String = w[i..].iter().collect();
        let next = w.get(i + 1).copied();
        let prev = i.checked_sub(1).map(|p| w[p]);

        // Silent letters & common endings.
        if rest.starts_with("tion") {
            out.push_str("ʃən");
            i += 4;
            continue;
        }
        if rest.starts_with("sion") {
            out.push_str("ʒən");
            i += 4;
            continue;
        }
        if rest == "ture" {
            out.push_str("tʃɚ");
            i += 4;
            continue;
        }
        if rest.starts_with("ough") || rest.starts_with("augh") {
            out.push_str("ɔː");
            i += 4;
            continue;
        }
        if rest.starts_with("eigh") {
            out.push_str("eɪ");
            i += 4;
            continue;
        }
        if rest.starts_with("igh") {
            out.push_str("aɪ");
            i += 3;
            continue;
        }
        if rest.starts_with("dge") {
            out.push_str("dʒ");
            i += 3;
            continue;
        }
        if rest.starts_with("tch") {
            out.push_str("tʃ");
            i += 3;
            continue;
        }
        if rest.starts_with("ing") && rest == "ing" {
            out.push_str("ɪŋ");
            i += 3;
            continue;
        }
        if rest.starts_with("kn") && i == 0 {
            out.push_str("n");
            i += 2;
            continue;
        }
        if rest.starts_with("wr") && i == 0 {
            out.push_str("ɹ");
            i += 2;
            continue;
        }
        if rest.starts_with("wh") {
            out.push_str("w");
            i += 2;
            continue;
        }
        if rest.starts_with("ph") {
            out.push_str("f");
            i += 2;
            continue;
        }
        if rest.starts_with("sh") {
            out.push_str("ʃ");
            i += 2;
            continue;
        }
        if rest.starts_with("ch") {
            out.push_str("tʃ");
            i += 2;
            continue;
        }
        if rest.starts_with("th") {
            out.push_str("θ");
            i += 2;
            continue;
        }
        if rest.starts_with("ck") {
            out.push_str("k");
            i += 2;
            continue;
        }
        if rest.starts_with("qu") {
            out.push_str("kw");
            i += 2;
            continue;
        }
        if rest.starts_with("ng") {
            out.push_str("ŋ");
            i += 2;
            continue;
        }
        if rest.starts_with("ee") || rest.starts_with("ea") {
            out.push_str("iː");
            i += 2;
            continue;
        }
        if rest.starts_with("oo") {
            out.push_str("uː");
            i += 2;
            continue;
        }
        if rest.starts_with("ai") || rest.starts_with("ay") || rest.starts_with("ei") {
            out.push_str("eɪ");
            i += 2;
            continue;
        }
        if rest.starts_with("oa") || rest.starts_with("oe") {
            out.push_str("oʊ");
            i += 2;
            continue;
        }
        if rest.starts_with("ou") {
            out.push_str("aʊ");
            i += 2;
            continue;
        }
        if rest.starts_with("ow") {
            out.push_str(if rest.len() <= 3 { "oʊ" } else { "aʊ" });
            i += 2;
            continue;
        }
        if rest.starts_with("oi") || rest.starts_with("oy") {
            out.push_str("ɔɪ");
            i += 2;
            continue;
        }
        if rest.starts_with("au") || rest.starts_with("aw") {
            out.push_str("ɔː");
            i += 2;
            continue;
        }
        if rest.starts_with("ie") {
            out.push_str("aɪ");
            i += 2;
            continue;
        }
        if rest.starts_with("ue") || rest.starts_with("ui") {
            out.push_str("uː");
            i += 2;
            continue;
        }

        // R-coloured vowels before a consonant or at the end.
        if is_vowel(w[i]) && next == Some('r') && (i + 2 >= w.len() || !is_vowel(w[i + 2])) {
            match w[i] {
                'a' => out.push_str("ɑːɹ"),
                'o' => out.push_str("ɔːɹ"),
                'e' | 'i' | 'u' => out.push_str("ɝ"),
                _ => unreachable!(),
            }
            i += 2;
            continue;
        }

        // Silent final e lengthens the last vowel.
        if magic_e && i + 1 == w.len() && is_vowel(w[i]) {
            match w[i] {
                'a' => out.push_str("eɪ"),
                'e' => out.push_str("iː"),
                'i' => out.push_str("aɪ"),
                'o' => out.push_str("oʊ"),
                'u' => out.push_str("uː"),
                _ => unreachable!(),
            }
            i += 1;
            continue;
        }

        // Collapse doubled consonants ("hello" -> "helo").
        if Some(w[i]) == next && !is_vowel(w[i]) {
            i += 1;
            continue;
        }

        out.push_str(single(w[i], next, prev));
        i += 1;
    }
    out
}

fn lexicon() -> HashMap<&'static str, &'static str> {
    // Common chat words where the rule engine would be badly wrong.
    let pairs: &[(&str, &str)] = &[
        ("a", "ə"),
        ("an", "æn"),
        ("the", "ðə"),
        ("to", "tə"),
        ("of", "ʌv"),
        ("and", "ænd"),
        ("or", "ɔːɹ"),
        ("is", "ɪz"),
        ("are", "ɑːɹ"),
        ("was", "wʌz"),
        ("were", "wɝ"),
        ("be", "biː"),
        ("been", "bɪn"),
        ("am", "æm"),
        ("i", "aɪ"),
        ("i'm", "aɪm"),
        ("you", "juː"),
        ("your", "jɔːɹ"),
        ("you're", "jʊɹ"),
        ("we", "wiː"),
        ("they", "ðeɪ"),
        ("he", "hiː"),
        ("she", "ʃiː"),
        ("it", "ɪt"),
        ("it's", "ɪts"),
        ("my", "maɪ"),
        ("me", "miː"),
        ("our", "aʊɚ"),
        ("their", "ðɛɹ"),
        ("there", "ðɛɹ"),
        ("here", "hɪɹ"),
        ("hi", "haɪ"),
        ("hello", "hɛloʊ"),
        ("hey", "heɪ"),
        ("yo", "joʊ"),
        ("wow", "waʊ"),
        ("lol", "lɑl"),
        ("omg", "oʊ ɛm dʒiː"),
        ("ok", "oʊkeɪ"),
        ("okay", "oʊkeɪ"),
        ("yes", "jɛs"),
        ("yeah", "jɛə"),
        ("no", "noʊ"),
        ("nope", "noʊp"),
        ("please", "pliːz"),
        ("thanks", "θæŋks"),
        ("thank", "θæŋk"),
        ("ty", "tiː waɪ"),
        ("thx", "θæŋks"),
        ("love", "lʌv"),
        ("like", "laɪk"),
        ("follow", "fɑloʊ"),
        ("gift", "ɡɪft"),
        ("rose", "ɹoʊz"),
        ("tiktok", "tɪktɑk"),
        ("live", "laɪv"),
        ("welcome", "wɛlkəm"),
        ("good", "ɡʊd"),
        ("great", "ɡɹeɪt"),
        ("nice", "naɪs"),
        ("cool", "kuːl"),
        ("awesome", "ɔːsəm"),
        ("friend", "fɹɛnd"),
        ("bro", "bɹoʊ"),
        ("sis", "sɪs"),
        ("what", "wʌt"),
        ("why", "waɪ"),
        ("how", "haʊ"),
        ("who", "huː"),
        ("when", "wɛn"),
        ("where", "wɛɹ"),
        ("one", "wʌn"),
        ("two", "tuː"),
        ("three", "θɹiː"),
        ("four", "fɔːɹ"),
        ("five", "faɪv"),
        ("six", "sɪks"),
        ("seven", "sɛvən"),
        ("eight", "eɪt"),
        ("nine", "naɪn"),
        ("ten", "tɛn"),
        ("hundred", "hʌndɹəd"),
        ("thousand", "θaʊzənd"),
        ("million", "mɪljən"),
        ("zero", "zɪɹoʊ"),
        ("eleven", "ɪlɛvən"),
        ("twelve", "twɛlv"),
        ("thirteen", "θɝtiːn"),
        ("fourteen", "fɔːɹtiːn"),
        ("fifteen", "fɪftiːn"),
        ("sixteen", "sɪkstiːn"),
        ("seventeen", "sɛvəntiːn"),
        ("eighteen", "eɪtiːn"),
        ("nineteen", "naɪntiːn"),
        ("twenty", "twɛnti"),
        ("thirty", "θɝti"),
        ("forty", "fɔːɹti"),
        ("fifty", "fɪfti"),
        ("sixty", "sɪksti"),
        ("seventy", "sɛvənti"),
        ("eighty", "eɪti"),
        ("ninety", "naɪnti"),
    ];
    pairs.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_words_are_phonemized() {
        let g2p = EnglishG2p::new();
        let ipa = g2p
            .phonemize("Hello friend, thanks for the follow!")
            .unwrap();
        assert_eq!(ipa, "hɛloʊ fɹɛnd , θæŋks fɔːɹ ðə fɑloʊ !");
    }

    #[test]
    fn numbers_are_spoken() {
        let g2p = EnglishG2p::new();
        let ipa = g2p.phonemize("42").unwrap();
        assert!(ipa.contains(' '));
    }

    #[test]
    fn rejects_empty_text() {
        let g2p = EnglishG2p::new();
        assert!(g2p.phonemize("   ").is_err());
    }
}
