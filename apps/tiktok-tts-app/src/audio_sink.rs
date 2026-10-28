//! Audio playback sink abstraction for background playback.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub trait AudioSink: Send + Sync {
    /// Plays raw audio samples with a master `volume` multiplier (`0.0..=1.0`).
    /// Playback should monitor `cancel` and abort mid-sentence
    /// immediately when `cancel.load(Ordering::SeqCst)` becomes true.
    fn play(
        &self,
        seq: u64,
        samples: &[f32],
        sample_rate: u32,
        volume: f32,
        cancel: Arc<AtomicBool>,
    ) -> Result<(), String>;

    /// Immediately halts the currently playing audio buffer.
    fn stop(&self);
}

/// Scales normalized `f32` audio samples by `volume` (`0.0..=1.0`) and clamps to `[-1.0, 1.0]`.
pub fn scale_samples(samples: &[f32], volume: f32) -> Vec<f32> {
    let gain = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    };
    samples
        .iter()
        .map(|sample| (sample * gain).clamp(-1.0, 1.0))
        .collect()
}

/// Helper to convert normalized f32 audio samples into 16-bit little-endian PCM bytes
/// scaled by `volume` (`0.0..=1.0`).
pub fn samples_to_pcm16_le_with_volume(samples: &[f32], volume: f32) -> Vec<u8> {
    let gain = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut out = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let scaled = ((sample * gain).clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    out
}

/// Helper to convert normalized f32 audio samples into 16-bit little-endian PCM bytes.
pub fn samples_to_pcm16_le(samples: &[f32]) -> Vec<u8> {
    samples_to_pcm16_le_with_volume(samples, 1.0)
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
        _volume: f32,
        _cancel: Arc<AtomicBool>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn stop(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_clamps_pcm_samples() {
        let samples = [0.5f32, -0.5, 2.0, -2.0];
        let muted = samples_to_pcm16_le_with_volume(&samples, 0.0);
        for chunk in muted.chunks_exact(2) {
            assert_eq!(i16::from_le_bytes([chunk[0], chunk[1]]), 0);
        }

        let half = scale_samples(&samples, 0.5);
        assert!((half[0] - 0.25).abs() < 1e-6);
        assert!((half[1] + 0.25).abs() < 1e-6);
        assert!((half[2] - 1.0).abs() < 1e-6);
        assert!((half[3] + 1.0).abs() < 1e-6);
    }
}

