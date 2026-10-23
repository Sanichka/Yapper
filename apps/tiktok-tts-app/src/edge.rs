//! Microsoft Edge online voice engine for languages KittenTTS does not cover.
//!
//! KittenTTS 0.8 only speaks English, so Ukrainian chat is synthesized with
//! Microsoft Edge neural voices over the internet (no API key, no login). The
//! returned MP3 is decoded to 24 kHz mono PCM, the same format the rest of the
//! pipeline uses.

use kittentts_rs::{EngineKind, EngineStatus, SynthRequest, TtsEngine, TtsError, SAMPLE_RATE};
use msedge_tts::tts::{client::connect, SpeechConfig};

const EDGE_AUDIO_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// rustls ships both ring and aws-lc crypto in this workspace, so the default
/// provider must be installed explicitly before `msedge-tts` builds a client.
pub fn ensure_default_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
    });
}

pub struct EdgeEngine;

impl Default for EdgeEngine {
    fn default() -> Self {
        Self
    }
}

impl TtsEngine for EdgeEngine {
    fn status(&self) -> EngineStatus {
        EngineStatus {
            kind: EngineKind::Edge,
            model: "Microsoft Edge Read Aloud (online)".into(),
            sample_rate: SAMPLE_RATE,
            voices: kittentts_rs::language::UKRAINIAN_VOICES
                .iter()
                .map(|v| v.to_string())
                .collect(),
            threads: 0,
        }
    }

    fn synthesize(&self, request: &SynthRequest) -> kittentts_rs::Result<Vec<f32>> {
        let text = request.text.trim();
        if text.is_empty() {
            return Err(TtsError::BadText("empty text".into()));
        }
        let rate = (((request.speed.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32).clamp(-50, 100);

        let mut client = connect().map_err(|e| TtsError::Service(e.to_string()))?;
        let config = SpeechConfig {
            voice_name: request.voice.clone(),
            audio_format: EDGE_AUDIO_FORMAT.into(),
            pitch: 0,
            rate,
            volume: 0,
        };
        let audio = client
            .synthesize(text, &config)
            .map_err(|e| TtsError::Service(e.to_string()))?;
        if audio.audio_bytes.is_empty() {
            return Err(TtsError::EmptyAudio);
        }

        let decoded = nanomp3::decode_all::<f32>(&audio.audio_bytes);
        if decoded.samples.is_empty() || decoded.sample_rate == 0 {
            return Err(TtsError::EmptyAudio);
        }
        let mut samples = match decoded.channels {
            Some(nanomp3::Channels::Stereo) => decoded
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| (pair[0] + pair[1]) * 0.5)
                .collect::<Vec<f32>>(),
            _ => decoded.samples,
        };
        if decoded.sample_rate != SAMPLE_RATE {
            samples = resample(&samples, decoded.sample_rate, SAMPLE_RATE);
        }
        if samples.is_empty() {
            return Err(TtsError::EmptyAudio);
        }
        Ok(samples)
    }
}

/// Simple linear resampler; Edge usually returns 24 kHz already, so this only
/// matters if the output format ever changes.
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 {
        return samples.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = ((samples.len() as f64) / ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = samples.get(idx).copied().unwrap_or(0.0);
        let b = samples.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}
