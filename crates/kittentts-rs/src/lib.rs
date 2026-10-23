//! Native KittenTTS inference, chat text filtering and a bounded TTS queue.
//!
//! The ONNX models are the lightweight KittenTTS 0.8 family
//! (`kitten-tts-nano/micro/mini`): a phoneme-to-waveform network that takes
//! `input_ids`, a per-voice `style` embedding and a `speed` scalar and returns
//! 24 kHz audio.
//!
//! ```no_run
//! use kittentts_rs::{KittenEngine, SynthRequest, TtsEngine};
//!
//! # fn main() -> kittentts_rs::Result<()> {
//! let engine = KittenEngine::from_dir("models/kitten-tts-mini-0.8")?;
//! let audio = engine.synthesize(&SynthRequest {
//!     text: "hello from Rust".into(),
//!     voice: "Bella".into(),
//!     speed: 1.0,
//! })?;
//! assert_eq!(engine.sample_rate(), 24_000);
//! # Ok(())
//! # }
//! ```

pub mod engine;
pub mod error;
pub mod language;
pub mod phonemes;
pub mod queue;
pub mod text;
pub mod tokenizer;
pub mod voices;
pub mod wav;

pub use engine::{
    create_engine, EngineKind, EngineStatus, KittenEngine, MockEngine, SynthRequest, TtsEngine,
};
pub use error::{Result, TtsError};
pub use phonemes::{EnglishG2p, Phonemizer};
pub use queue::{BoundedQueue, Priority, PushOutcome, QueueItem, QueueSnapshot};
pub use text::{
    render_template, sanitize, FilterReason, Prepared, TextPipeline, TtsSettings, VOICES,
};
pub use tokenizer::Tokenizer;
pub use voices::VoiceBank;
pub use wav::wav_bytes;

/// Audio sample rate of every KittenTTS 0.8 model.
pub const SAMPLE_RATE: u32 = 24_000;

/// Maximum characters accepted from one chat message.
pub const MAX_LEN: usize = 150;

/// Maximum number of items waiting for synthesis.
pub const MAX_QUEUE_SIZE: usize = 15;
