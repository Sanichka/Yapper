//! KittenTTS ONNX engine, engine selection and a deterministic mock voice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

use crate::error::{Result, TtsError};
use crate::phonemes::{EnglishG2p, Phonemizer};
use crate::text::VOICES;
use crate::tokenizer::Tokenizer;
use crate::voices::VoiceBank;
use crate::{MAX_LEN, SAMPLE_RATE};

/// One synthesis job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SynthRequest {
    pub text: String,
    pub voice: String,
    pub speed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Onnx,
    Mock,
    Edge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub kind: EngineKind,
    /// Model file name, or `tone` for the mock engine.
    pub model: String,
    pub sample_rate: u32,
    pub voices: Vec<String>,
    pub threads: usize,
}

pub trait TtsEngine: Send + Sync {
    fn status(&self) -> EngineStatus;

    fn synthesize(&self, request: &SynthRequest) -> Result<Vec<f32>>;

    fn sample_rate(&self) -> u32 {
        self.status().sample_rate
    }

    fn is_mock(&self) -> bool {
        self.status().kind == EngineKind::Mock
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ModelConfig {
    model_file: Option<String>,
    voices: Option<String>,
    voice_aliases: HashMap<String, String>,
    speed_priors: HashMap<String, f32>,
}

#[derive(Debug)]
struct InputRoles {
    ids: String,
    style: Option<String>,
    speed: Option<String>,
}

pub struct KittenEngine {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    phonemizer: EnglishG2p,
    voices: VoiceBank,
    speed_priors: HashMap<String, f32>,
    input_roles: InputRoles,
    output_name: String,
    model_name: String,
    threads: usize,
}

impl KittenEngine {
    /// Loads `config.json`, the ONNX model and `voices.npz` from `dir`.
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        if !dir.is_dir() {
            return Err(TtsError::ModelMissing(dir.display().to_string()));
        }
        let config = read_config(dir)?;
        let model_path = resolve_model_path(dir, config.model_file.as_deref())?;
        let voices_path = {
            let name = config.voices.clone().unwrap_or_else(|| "voices.npz".into());
            let path = dir.join(&name);
            if !path.is_file() {
                return Err(TtsError::Voices(format!(
                    "{} is missing (expected next to the ONNX model)",
                    path.display()
                )));
            }
            path
        };

        let mut voices = VoiceBank::load(&voices_path)?;
        if !config.voice_aliases.is_empty() {
            voices = voices.with_aliases(config.voice_aliases.clone());
        }

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4);
        let env = environment()?;
        let mut builder = Session::builder(&env).map_err(ort_err)?;
        builder = builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .unwrap_or_else(|e| e.recover());
        builder = builder
            .with_intra_threads(threads)
            .unwrap_or_else(|e| e.recover());
        let session = builder
            .commit_from_file(&model_path)
            .map_err(|e| TtsError::Ort(format!("{}: {e}", model_path.display())))?;

        let input_names: Vec<String> = session
            .inputs()
            .iter()
            .map(|input| input.name().to_string())
            .collect();
        let output_name = session
            .outputs()
            .first()
            .map(|output| output.name().to_string())
            .ok_or_else(|| TtsError::Config("model has no outputs".into()))?;
        let input_roles = classify_inputs(&input_names)?;

        let model_name = model_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "model.onnx".into());

        Ok(Self {
            session: Mutex::new(session),
            tokenizer: Tokenizer::new(),
            phonemizer: EnglishG2p::new(),
            voices,
            speed_priors: config.speed_priors,
            input_roles,
            output_name,
            model_name,
            threads,
        })
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }
}

impl TtsEngine for KittenEngine {
    fn status(&self) -> EngineStatus {
        EngineStatus {
            kind: EngineKind::Onnx,
            model: self.model_name.clone(),
            sample_rate: SAMPLE_RATE,
            voices: self.voices.available_voices(),
            threads: self.threads,
        }
    }

    fn synthesize(&self, request: &SynthRequest) -> Result<Vec<f32>> {
        let text = request.text.trim();
        if text.is_empty() {
            return Err(TtsError::BadText("empty text".into()));
        }

        let phonemes = self.phonemizer.phonemize(text)?;
        if phonemes.trim().is_empty() {
            return Err(TtsError::BadText(format!("`{text}` produced no phonemes")));
        }
        let tokens = self.tokenizer.encode_phonemes(&phonemes);
        let char_count = text.chars().count().min(MAX_LEN);

        let canonical = self
            .voices
            .canonical(&request.voice)
            .unwrap_or("expr-voice-2-f")
            .to_string();
        let prior = self.speed_priors.get(&canonical).copied().unwrap_or(1.0);
        let speed = (request.speed * prior).clamp(0.5, 2.0);
        let style = self.voices.style_row(&request.voice, char_count)?;

        let token_len = tokens.len() as i64;
        let ids = Tensor::from_array((vec![1_i64, token_len], tokens)).map_err(ort_err)?;
        let style_tensor =
            Tensor::from_array((vec![1_i64, style.len() as i64], style)).map_err(ort_err)?;
        let speed_tensor = Tensor::from_array((vec![1_i64], vec![speed])).map_err(ort_err)?;

        let mut feeds: Vec<(String, SessionInputValue)> = Vec::new();
        feeds.push((self.input_roles.ids.clone(), ids.into()));
        if let Some(name) = &self.input_roles.style {
            feeds.push((name.clone(), style_tensor.into()));
        }
        if let Some(name) = &self.input_roles.speed {
            feeds.push((name.clone(), speed_tensor.into()));
        }

        let mut session = self
            .session
            .lock()
            .map_err(|_| TtsError::Ort("inference session was poisoned".into()))?;
        let outputs = session.run(feeds).map_err(ort_err)?;
        if outputs.len() == 0 {
            return Err(TtsError::Ort("model returned no outputs".into()));
        }
        let output = outputs
            .get(self.output_name.as_str())
            .unwrap_or(&outputs[0]);
        let (_shape, data) = output.try_extract_tensor::<f32>().map_err(ort_err)?;

        // The reference implementation trims the trailing 5000 samples.
        let keep = data.len().saturating_sub(5000);
        if keep == 0 {
            return Err(TtsError::EmptyAudio);
        }
        Ok(data[..keep].to_vec())
    }
}

fn classify_inputs(names: &[String]) -> Result<InputRoles> {
    let mut ids = None;
    let mut style = None;
    let mut speed = None;
    for name in names {
        let lower = name.to_ascii_lowercase();
        if lower.contains("style") || lower.contains("voice") || lower.contains("ref") {
            style = Some(name.clone());
        } else if lower.contains("speed") {
            speed = Some(name.clone());
        } else if lower.contains("id") || lower.contains("token") || lower.contains("input") {
            ids = Some(name.clone());
        }
    }
    let ids = ids.ok_or_else(|| {
        TtsError::Config(format!("could not find the token input among {names:?}"))
    })?;
    Ok(InputRoles { ids, style, speed })
}

fn read_config(dir: &Path) -> Result<ModelConfig> {
    let path = dir.join("config.json");
    if !path.is_file() {
        return Ok(ModelConfig::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    serde_json::from_str(&raw).map_err(|e| TtsError::Config(format!("{}: {e}", path.display())))
}

fn resolve_model_path(dir: &Path, configured: Option<&str>) -> Result<PathBuf> {
    if let Some(name) = configured {
        let path = dir.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    for name in ["model.onnx", "kitten_tts.onnx", "kittentts.onnx"] {
        let path = dir.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    let mut onnx: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("onnx"))
                    .unwrap_or(false)
        })
        .collect();
    onnx.sort();
    onnx.into_iter().next().ok_or_else(|| {
        TtsError::ModelMissing(format!(
            "{} has no .onnx file (expected kitten_tts_mini_v0_8.onnx)",
            dir.display()
        ))
    })
}

fn ort_err(error: impl std::fmt::Display) -> TtsError {
    TtsError::Ort(error.to_string())
}

/// ONNX Runtime permits exactly one environment per process, so it is created lazily.
fn environment() -> Result<ort::environment::Environment> {
    static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();
    static INIT: Mutex<()> = Mutex::new(());

    if let Some(env) = ENV.get() {
        return Ok(env.clone());
    }
    let _guard = INIT
        .lock()
        .map_err(|_| TtsError::Ort("ONNX Runtime environment lock was poisoned".into()))?;
    if let Some(env) = ENV.get() {
        return Ok(env.clone());
    }
    let env = build_environment()?;
    let _ = ENV.set(env.clone());
    Ok(env)
}

fn build_environment() -> Result<ort::environment::Environment> {
    let builder = {
        #[cfg(feature = "onnx-dynamic")]
        {
            match dynamic_library_path() {
                Some(path) => ort::init_from(&path)
                    .map_err(|e| TtsError::Ort(format!("{}: {e}", path.display())))?,
                None => ort::init(),
            }
        }
        #[cfg(not(feature = "onnx-dynamic"))]
        {
            ort::init()
        }
    };
    builder
        .with_name("yapper")
        .build()
        .map_err(|e| TtsError::Ort(format!("could not create ONNX Runtime environment: {e}")))
}

#[cfg(feature = "onnx-dynamic")]
fn dynamic_library_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("YAPPER_ORT_DYLIB") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let names = [
        "onnxruntime.dll",
        "libonnxruntime.so",
        "libonnxruntime.dylib",
    ];
    for name in names {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Deterministic tone generator used when no model is installed. It does not
/// speak, but it exercises the full queue -> WAV -> playback pipeline.
pub struct MockEngine {
    status: EngineStatus,
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEngine {
    pub fn new() -> Self {
        Self {
            status: EngineStatus {
                kind: EngineKind::Mock,
                model: "tone".into(),
                sample_rate: SAMPLE_RATE,
                voices: VOICES.iter().map(|v| v.to_string()).collect(),
                threads: 1,
            },
        }
    }
}

impl TtsEngine for MockEngine {
    fn status(&self) -> EngineStatus {
        self.status.clone()
    }

    fn synthesize(&self, request: &SynthRequest) -> Result<Vec<f32>> {
        let text = request.text.trim();
        if text.is_empty() {
            return Err(TtsError::BadText("empty text".into()));
        }
        let sr = SAMPLE_RATE as f32;
        let base = pitch_for_voice(&request.voice) * request.speed.clamp(0.5, 2.0);
        let mut samples: Vec<f32> = Vec::new();
        let limit = (3.0 * sr) as usize;

        for (index, ch) in text.chars().take(MAX_LEN).enumerate() {
            if samples.len() >= limit {
                break;
            }
            let punctuation = ";:,.!?…—".contains(ch);
            let duration = if punctuation { 0.11 } else { 0.052 };
            let count = (duration * sr) as usize;
            if punctuation || ch.is_whitespace() {
                samples.extend(std::iter::repeat_n(0.0, count));
                continue;
            }
            let wobble = 1.0 + ((index as u32 % 7) as f32 - 3.0) * 0.028;
            let frequency = base * wobble;
            for i in 0..count {
                let progress = i as f32 / count as f32;
                let envelope = (std::f32::consts::PI * progress).sin().powi(2);
                let t = i as f32 / sr;
                let value = (2.0 * std::f32::consts::PI * frequency * t).sin()
                    + 0.35 * (4.0 * std::f32::consts::PI * frequency * t).sin()
                    + 0.15 * (6.0 * std::f32::consts::PI * frequency * t).sin();
                samples.push(value * 0.18 * envelope);
            }
        }
        if samples.is_empty() {
            return Err(TtsError::EmptyAudio);
        }
        Ok(samples)
    }
}

fn pitch_for_voice(voice: &str) -> f32 {
    let lower = voice.to_ascii_lowercase();
    match lower.as_str() {
        "bella" => 215.0,
        "luna" => 235.0,
        "rosie" => 200.0,
        "kiki" => 250.0,
        "jasper" => 120.0,
        "bruno" => 105.0,
        "hugo" => 115.0,
        "leo" => 130.0,
        _ => 165.0,
    }
}

/// Loads the ONNX engine if possible, otherwise falls back to [`MockEngine`].
/// The second value explains a fallback (shown in the UI as a warning).
pub fn create_engine(model_dir: Option<&Path>) -> (Arc<dyn TtsEngine>, Option<String>) {
    let Some(dir) = model_dir else {
        return (
            Arc::new(MockEngine::new()),
            Some("no model directory configured; using mock voice".into()),
        );
    };
    match KittenEngine::from_dir(dir) {
        Ok(engine) => (Arc::new(engine), None),
        Err(error) => (
            Arc::new(MockEngine::new()),
            Some(format!("{error}; using mock voice")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_engine_produces_audio() {
        let engine = MockEngine::new();
        let audio = engine
            .synthesize(&SynthRequest {
                text: "hello there".into(),
                voice: "Bella".into(),
                speed: 1.0,
            })
            .unwrap();
        assert!(audio.len() > 1000);
        assert!(audio.iter().all(|s| s.is_finite()));
        assert!(engine.is_mock());
    }

    #[test]
    fn mock_engine_rejects_empty_text() {
        let engine = MockEngine::new();
        assert!(engine
            .synthesize(&SynthRequest {
                text: "   ".into(),
                voice: "Bella".into(),
                speed: 1.0,
            })
            .is_err());
    }

    #[test]
    fn input_roles_are_detected() {
        let roles = classify_inputs(&["input_ids".into(), "style".into(), "speed".into()]).unwrap();
        assert_eq!(roles.ids, "input_ids");
        assert_eq!(roles.style.as_deref(), Some("style"));
        assert_eq!(roles.speed.as_deref(), Some("speed"));
    }
}
