//! Byte-compatible port of the KittenTTS `TextCleaner` tokenizer.
//!
//! The symbol table (and therefore every token id) must match the Python
//! implementation exactly; it is baked into the ONNX model.

use std::collections::HashMap;

use regex::Regex;

const PAD: &str = "$";
const PUNCTUATION: &str = ";:,.!?¡¿—…\"«»\"\" ";
const LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const LETTERS_IPA: &str = "ɑɐɒæɓʙβɔɕçɗɖðʤəɘɚɛɜɝɞɟʄɡɠɢʛɦɧħɥʜɨɪʝɭɬɫɮʟɱɯɰŋɳɲɴøɵɸθœɶʘɹɺɾɻʀʁɽʂʃʈʧʉʊʋⱱʌɣɤʍχʎʏʑʐʒʔʡʕʢǀǁǂǃˈˌːˑʼʴʰʱʲʷˠˤ˞↓↑→↗↘'̩'ᵻ";

pub const START_TOKEN: i64 = 0;
/// Index of `…` in the symbol table; KittenTTS appends it before the final pad.
pub const END_TOKEN: i64 = 10;
pub const PAD_TOKEN: i64 = 0;

pub struct Tokenizer {
    symbols: HashMap<char, i64>,
    words: Regex,
}

impl Default for Tokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Tokenizer {
    pub fn new() -> Self {
        let mut symbols = HashMap::new();
        let all = format!("{PAD}{PUNCTUATION}{LETTERS}{LETTERS_IPA}");
        for (index, ch) in all.chars().enumerate() {
            // Mirrors Python's `dict[symbol] = i`: later duplicates win.
            symbols.insert(ch, index as i64);
        }
        Self {
            symbols,
            words: Regex::new(r"\w+|[^\w\s]").expect("token regex"),
        }
    }

    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    pub fn token_of(&self, ch: char) -> Option<i64> {
        self.symbols.get(&ch).copied()
    }

    /// `basic_english_tokenize` + `TextCleaner.__call__` + start/end tokens.
    pub fn encode_phonemes(&self, phonemes: &str) -> Vec<i64> {
        let mut tokens = Vec::with_capacity(phonemes.len() / 2 + 4);
        tokens.push(START_TOKEN);

        for piece in self.words.find_iter(phonemes) {
            for ch in piece.as_str().chars() {
                if let Some(id) = self.symbols.get(&ch) {
                    tokens.push(*id);
                }
            }
            if let Some(space) = self.symbols.get(&' ') {
                tokens.push(*space);
            }
        }
        if tokens.last() == self.symbols.get(&' ') {
            tokens.pop();
        }

        tokens.push(END_TOKEN);
        tokens.push(PAD_TOKEN);
        tokens
    }

    /// Maps every known character and drops unknown ones (like the Python cleaner).
    pub fn encode_chars(&self, text: &str) -> Vec<i64> {
        text.chars()
            .filter_map(|ch| self.symbols.get(&ch).copied())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_table_matches_reference_offsets() {
        let t = Tokenizer::new();
        assert_eq!(t.token_of('$'), Some(0));
        assert_eq!(t.token_of('…'), Some(10));
        assert_eq!(t.token_of('"'), Some(15));
        assert_eq!(t.token_of(' '), Some(16));
        assert_eq!(t.token_of('a'), Some(43));
        assert_eq!(t.token_of('ˈ'), Some(156));
        assert_eq!(END_TOKEN, t.token_of('…').unwrap());
    }

    #[test]
    fn encoding_wraps_with_start_and_end() {
        let t = Tokenizer::new();
        let ids = t.encode_phonemes("h ɛ l oʊ");
        assert_eq!(ids.first(), Some(&START_TOKEN));
        assert_eq!(ids[ids.len() - 2], END_TOKEN);
        assert_eq!(*ids.last().unwrap(), PAD_TOKEN);
        assert!(ids.len() > 4);
        assert!(ids.contains(&16));
    }
}
