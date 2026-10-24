//! Audio playback sink abstraction for background playback.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub trait AudioSink: Send + Sync {
    /// Plays raw audio samples. Playback should monitor `cancel` and abort mid-sentence
    /// immediately when `cancel.load(Ordering::SeqCst)` becomes true.
    fn play(
        &self,
        seq: u64,
        samples: &[f32],
        sample_rate: u32,
        cancel: Arc<AtomicBool>,
    ) -> Result<(), String>;

    /// Immediately halts the currently playing audio buffer.
    fn stop(&self);
}

/// Helper to convert normalized f32 audio samples into 16-bit little-endian PCM bytes.
pub fn samples_to_pcm16_le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    out
}

/// Fallback no-op sink used when audio is delegated to the Web Audio frontend.
#[derive(Debug, Default)]
pub struct NoopAudioSink;

impl AudioSink for NoopAudioSink {
    fn play(
        &self,
        _seq: u64,
        _samples: &[f32],
        _sample_rate: u32,
        _cancel: Arc<AtomicBool>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn stop(&self) {}
}
