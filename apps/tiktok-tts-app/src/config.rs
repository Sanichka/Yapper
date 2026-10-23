//! On-disk configuration and path resolution.

use std::fs;
use std::path::{Path, PathBuf};

use kittentts_rs::text::TtsSettings;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 8787;
pub const MODEL_DIR_NAME: &str = "kitten-tts-mini-0.8";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersistedConfig {
    pub username: String,
    pub auto_connect: bool,
    pub settings: TtsSettings,
    /// Optional directory containing the KittenTTS ONNX model.
    pub model_dir: Option<String>,
    pub server_port: u16,
    pub queue_capacity: usize,
}

impl Default for PersistedConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            auto_connect: false,
            settings: TtsSettings::default(),
            model_dir: None,
            server_port: DEFAULT_PORT,
            queue_capacity: kittentts_rs::MAX_QUEUE_SIZE,
        }
    }
}

impl PersistedConfig {
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(config) => config,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "config is invalid; using defaults"
                    );
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read config");
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
        if fs::rename(&tmp, path).is_err() {
            // Windows refuses to rename over an existing file.
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
}

/// `%APPDATA%\Yapper`, `~/Library/Application Support/Yapper`, or
/// `~/.local/share/yapper`.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("YAPPER_DATA_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("Yapper");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Yapper");
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(data).join("yapper");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("yapper");
        }
    }
    PathBuf::from(".").join("yapper-data")
}

pub fn default_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("YAPPER_CONFIG") {
        return PathBuf::from(path);
    }
    default_data_dir().join("config.json")
}

pub fn default_ui_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("YAPPER_UI_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ui"));
            candidates.push(dir.join("ui").join("dist"));
            candidates.push(dir.join("..").join("ui").join("dist"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(
            cwd.join("apps")
                .join("tiktok-tts-app")
                .join("ui")
                .join("dist"),
        );
        candidates.push(cwd.join("ui").join("dist"));
        candidates.push(cwd.join("ui"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("ui")
            .join("dist"),
    );
    candidates
        .into_iter()
        .find(|dir| dir.join("index.html").is_file())
}

/// Finds the model directory: explicit override, env var, saved config, then
/// the conventional `models/` folders.
pub fn resolve_model_dir(config_path: &Path, configured: Option<&str>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("YAPPER_MODEL_DIR") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = configured.filter(|p| !p.trim().is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    if let Some(data) = config_path.parent() {
        candidates.push(data.join("models").join(MODEL_DIR_NAME));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("models").join(MODEL_DIR_NAME));
        candidates.push(cwd.join("models"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("models").join(MODEL_DIR_NAME));
        }
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("models")
            .join(MODEL_DIR_NAME),
    );

    for dir in &candidates {
        if dir.is_dir() && contains_onnx(dir) {
            return Some(dir.clone());
        }
    }
    candidates.into_iter().find(|dir| dir.is_dir())
}

fn contains_onnx(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("onnx"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrips() {
        let dir = std::env::temp_dir().join(format!("yapper-config-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = PersistedConfig::default();
        config.username = "creator".into();
        config.settings.voice = "Luna".into();
        config.save(&path).unwrap();
        let loaded = PersistedConfig::load(&path);
        assert_eq!(loaded.username, "creator");
        assert_eq!(loaded.settings.voice, "Luna");
        assert_eq!(loaded.settings.language, "auto");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn old_config_without_language_loads_with_defaults() {
        let raw = r#"{
          "username": "creator",
          "autoConnect": false,
          "serverPort": 8787,
          "queueCapacity": 15,
          "modelDir": null,
          "settings": {
            "voice": "Bella",
            "speed": 1.0,
            "template": "{user} says: {text}",
            "giftTemplate": "{user} sent {count} {gift}",
            "readGifts": true,
            "minGiftDiamonds": 1,
            "maxLen": 150,
            "cooldownSecs": 3,
            "transliterate": true,
            "blacklist": []
          }
        }"#;
        let config: PersistedConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(config.settings.language, "auto");
    }
}
