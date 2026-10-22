//! Voice style embeddings (`voices.npz`).
//!
//! Each voice is a matrix of `[N, 256]` style vectors; KittenTTS indexes it by
//! the input length, exactly like the Python reference
//! (`ref_id = min(len(text), rows - 1)`).

use std::collections::HashMap;
use std::path::Path;

use npyz::npz::NpzArchive;

use crate::error::{Result, TtsError};

#[derive(Debug, Clone)]
pub struct StyleMatrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f32>,
}

pub struct VoiceBank {
    styles: HashMap<String, StyleMatrix>,
    aliases: HashMap<String, String>,
}

impl VoiceBank {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let archive = NpzArchive::open(path)
            .map_err(|e| TtsError::Voices(format!("{}: {e}", path.display())))?;
        let names: Vec<String> = archive.array_names().map(|s| s.to_string()).collect();
        if names.is_empty() {
            return Err(TtsError::Voices("archive has no arrays".into()));
        }

        let mut styles = HashMap::new();
        for name in names {
            let style = read_style(path, &name)?;
            styles.insert(name, style);
        }

        Ok(Self {
            styles,
            aliases: default_aliases(),
        })
    }

    pub fn with_aliases(mut self, aliases: HashMap<String, String>) -> Self {
        self.aliases.extend(aliases);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    pub fn canonical<'a>(&'a self, voice: &'a str) -> Option<&'a str> {
        if self.styles.contains_key(voice) {
            return Some(voice);
        }
        if let Some(canonical) = self.aliases.get(voice) {
            if self.styles.contains_key(canonical) {
                return Some(canonical);
            }
        }
        // Case-insensitive alias match ("bella" -> "Bella" -> expr-voice-2-f).
        let wanted = voice.to_ascii_lowercase();
        for (display, canonical) in &self.aliases {
            if display.to_ascii_lowercase() == wanted && self.styles.contains_key(canonical) {
                return Some(canonical);
            }
        }
        self.styles
            .keys()
            .find(|key| key.eq_ignore_ascii_case(voice))
            .map(|key| key.as_str())
    }

    /// Display names for the voices actually present in the archive.
    pub fn available_voices(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (display, canonical) in &self.aliases {
            if self.styles.contains_key(canonical) && !out.contains(display) {
                out.push(display.clone());
            }
        }
        for key in self.styles.keys() {
            if !out.iter().any(|d| self.aliases.get(d) == Some(key)) {
                out.push(key.clone());
            }
        }
        out.sort();
        out
    }

    pub fn style_for(&self, voice: &str, _char_count: usize) -> Result<&StyleMatrix> {
        let canonical = self
            .canonical(voice)
            .ok_or_else(|| TtsError::UnknownVoice(voice.to_string()))?;
        self.styles
            .get(canonical)
            .ok_or_else(|| TtsError::UnknownVoice(voice.to_string()))
    }

    /// The style row used as the model's `style` input.
    pub fn style_row(&self, voice: &str, char_count: usize) -> Result<Vec<f32>> {
        let style = self.style_for(voice, char_count)?;
        let row = char_count.min(style.rows.saturating_sub(1));
        let start = row * style.cols;
        let end = start + style.cols;
        if end > style.data.len() || style.cols == 0 {
            return Err(TtsError::Voices(format!(
                "voice `{voice}` has an invalid shape ({}x{})",
                style.rows, style.cols
            )));
        }
        Ok(style.data[start..end].to_vec())
    }

    pub fn speed_prior(&self, _voice: &str) -> f32 {
        1.0
    }
}

fn read_style(path: &Path, name: &str) -> Result<StyleMatrix> {
    match read_style_as::<f32>(path, name) {
        Ok(style) => Ok(style),
        Err(_) => read_style_as::<f64>(path, name),
    }
}

fn read_style_as<T>(path: &Path, name: &str) -> Result<StyleMatrix>
where
    T: npyz::Deserialize + Copy + Into<f64>,
{
    let mut archive =
        NpzArchive::open(path).map_err(|e| TtsError::Voices(format!("{}: {e}", path.display())))?;
    let array = archive
        .by_name(name)
        .map_err(|e| TtsError::Voices(format!("{name}: {e}")))?
        .ok_or_else(|| TtsError::Voices(format!("missing array `{name}`")))?;

    let shape: Vec<u64> = array.shape().to_vec();
    if shape.len() != 2 {
        return Err(TtsError::Voices(format!(
            "`{name}` expected a 2-D style matrix, got shape {shape:?}"
        )));
    }
    let rows = shape[0] as usize;
    let cols = shape[1] as usize;
    let raw = array
        .into_vec::<T>()
        .map_err(|e| TtsError::Voices(format!("{name}: {e}")))?;
    if raw.len() != rows * cols {
        return Err(TtsError::Voices(format!(
            "`{name}` data length {} does not match {rows}x{cols}",
            raw.len()
        )));
    }
    let data: Vec<f32> = raw.into_iter().map(|v| v.into() as f32).collect();
    Ok(StyleMatrix { rows, cols, data })
}

pub fn default_aliases() -> HashMap<String, String> {
    [
        ("Bella", "expr-voice-2-f"),
        ("Jasper", "expr-voice-2-m"),
        ("Luna", "expr-voice-3-f"),
        ("Bruno", "expr-voice-3-m"),
        ("Rosie", "expr-voice-4-f"),
        ("Hugo", "expr-voice-4-m"),
        ("Kiki", "expr-voice-5-f"),
        ("Leo", "expr-voice-5-m"),
    ]
    .into_iter()
    .map(|(display, canonical)| (display.to_string(), canonical.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_case_insensitively() {
        let mut bank = VoiceBank {
            styles: HashMap::new(),
            aliases: default_aliases(),
        };
        bank.styles.insert(
            "expr-voice-2-f".into(),
            StyleMatrix {
                rows: 2,
                cols: 2,
                data: vec![0.0; 4],
            },
        );
        assert_eq!(bank.canonical("Bella"), Some("expr-voice-2-f"));
        assert_eq!(bank.canonical("bella"), Some("expr-voice-2-f"));
        assert_eq!(bank.canonical("Unknown"), None);
        assert_eq!(bank.style_row("bella", 99).unwrap().len(), 2);
    }
}
