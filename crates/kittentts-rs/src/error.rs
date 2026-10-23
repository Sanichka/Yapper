use thiserror::Error;

pub type Result<T, E = TtsError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("model directory `{0}` does not contain a usable KittenTTS model")]
    ModelMissing(String),
    #[error("model config is invalid: {0}")]
    Config(String),
    #[error("voices.npz could not be read: {0}")]
    Voices(String),
    #[error(
        "voice `{0}` is not available (try Bella, Jasper, Luna, Bruno, Rosie, Hugo, Kiki, Leo)"
    )]
    UnknownVoice(String),
    #[error("text cannot be synthesized: {0}")]
    BadText(String),
    #[error("ONNX Runtime: {0}")]
    Ort(String),
    #[error("inference produced an empty waveform")]
    EmptyAudio,
    #[error("speech service failed: {0}")]
    Service(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
