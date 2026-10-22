//! Synthesize one sentence with a local KittenTTS model.
//!
//! ```text
//! cargo run -p kittentts-rs --example say -- "hello from yapper" --voice Luna
//! ```

use std::path::PathBuf;

use kittentts_rs::{wav_bytes, KittenEngine, SynthRequest, TtsEngine, SAMPLE_RATE};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut text = String::new();
    let mut voice = "Bella".to_string();
    let mut speed = 1.0f32;
    let mut out = PathBuf::from("yapper-sample.wav");
    let mut model = std::env::var("YAPPER_MODEL_DIR")
        .map(PathBuf::from)
        .ok()
        .filter(|path| path.is_dir())
        .or_else(|| {
            [
                "models/kitten-tts-mini-0.8",
                "../../models/kitten-tts-mini-0.8",
            ]
            .iter()
            .map(PathBuf::from)
            .find(|path| path.is_dir())
        });

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--voice" => voice = args.next().unwrap_or(voice),
            "--speed" => {
                speed = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(speed)
            }
            "--model" => model = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            other => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(other);
            }
        }
    }
    if text.trim().is_empty() {
        text = "Hello from Yapper. This is the Kitten TTS voice running locally.".into();
    }
    let model = model.ok_or("no model directory found; pass --model <dir>")?;

    eprintln!("loading {}", model.display());
    let engine = KittenEngine::from_dir(&model)?;
    eprintln!("synthesizing with {voice} at {speed:.2}x");
    let audio = engine.synthesize(&SynthRequest {
        text: text.clone(),
        voice,
        speed,
    })?;
    let seconds = audio.len() as f32 / SAMPLE_RATE as f32;
    let peak = audio
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let wav = wav_bytes(&audio, SAMPLE_RATE);
    std::fs::write(&out, wav)?;
    println!(
        "{} samples ({seconds:.2}s), peak {peak:.3}, wrote {}",
        audio.len(),
        out.display()
    );
    Ok(())
}
