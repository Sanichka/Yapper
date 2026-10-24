//! KittenTTS model catalog, download manager, and local storage resolution.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct ModelCatalogItem {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub repo: &'static str,
    pub onnx_file: &'static str,
    pub approx_size_mb: u32,
    pub files: &'static [&'static str],
}

pub const CATALOG: &[ModelCatalogItem] = &[
    ModelCatalogItem {
        id: "kitten-tts-nano-0.8",
        name: "KittenTTS Nano",
        description: "Lightweight & fast (~28 MB). Best for mobile.",
        repo: "KittenML/kitten-tts-nano-0.8-fp32",
        onnx_file: "kitten_tts_nano_v0_8.onnx",
        approx_size_mb: 28,
        files: &["config.json", "voices.npz", "kitten_tts_nano_v0_8.onnx"],
    },
    ModelCatalogItem {
        id: "kitten-tts-micro-0.8",
        name: "KittenTTS Micro",
        description: "Balanced speed & quality (~45 MB).",
        repo: "KittenML/kitten-tts-micro-0.8",
        onnx_file: "kitten_tts_micro_v0_8.onnx",
        approx_size_mb: 45,
        files: &["config.json", "voices.npz", "kitten_tts_micro_v0_8.onnx"],
    },
    ModelCatalogItem {
        id: "kitten-tts-mini-0.8",
        name: "KittenTTS Mini",
        description: "Highest quality (~82 MB). Default desktop model.",
        repo: "KittenML/kitten-tts-mini-0.8",
        onnx_file: "kitten_tts_mini_v0_8.onnx",
        approx_size_mb: 82,
        files: &["config.json", "voices.npz", "kitten_tts_mini_v0_8.onnx"],
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub approx_size_mb: u32,
    pub installed: bool,
    pub active: bool,
    pub disk_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgressEvent {
    pub model_id: String,
    pub status: String, // "downloading" | "verifying" | "complete" | "error"
    pub percent: f32,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub current_file: String,
    pub message: Option<String>,
}

pub fn is_model_installed(models_dir: &Path, id: &str) -> bool {
    let dir = models_dir.join(id);
    if !dir.is_dir() {
        return false;
    }
    let has_voices = dir.join("voices.npz").is_file();
    let has_onnx = fs::read_dir(&dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("onnx"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    has_voices && has_onnx
}

pub fn calc_dir_size(dir: &Path) -> u64 {
    if !dir.is_dir() {
        return 0;
    }
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

pub fn list_models(models_dir: &Path, active_model_dir: Option<&Path>) -> Vec<ModelDto> {
    let active_dir_name = active_model_dir
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());

    CATALOG
        .iter()
        .map(|cat| {
            let installed = is_model_installed(models_dir, cat.id);
            let active = installed && active_dir_name == Some(cat.id);
            let disk_size_bytes = if installed {
                Some(calc_dir_size(&models_dir.join(cat.id)))
            } else {
                None
            };
            ModelDto {
                id: cat.id.to_string(),
                name: cat.name.to_string(),
                description: cat.description.to_string(),
                approx_size_mb: cat.approx_size_mb,
                installed,
                active,
                disk_size_bytes,
            }
        })
        .collect()
}

pub async fn download_model<F>(
    models_dir: PathBuf,
    model_id: String,
    progress_cb: F,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<PathBuf, String>
where
    F: Fn(ModelProgressEvent) + Send + Sync + 'static,
{
    let item = CATALOG
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| format!("Unknown model: {model_id}"))?;

    fs::create_dir_all(&models_dir)
        .map_err(|e| format!("Could not create models directory {}: {e}", models_dir.display()))?;

    let temp_dir = models_dir.join(format!(".{model_id}.download"));
    if temp_dir.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
    }
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Could not create temp directory {}: {e}", temp_dir.display()))?;

    let client = reqwest::Client::builder()
        .user_agent("Yapper/0.0.1 (KittenTTS downloader)")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    // Pre-calculate / estimate total bytes
    let estimated_total_bytes = (item.approx_size_mb as u64) * 1024 * 1024;
    let mut total_downloaded: u64 = 0;
    let mut last_progress_emit = Instant::now();

    for &filename in item.files {
        if let Some(ref flag) = cancel_flag {
            if flag.load(Ordering::Relaxed) {
                let _ = fs::remove_dir_all(&temp_dir);
                return Err("Download cancelled by user".into());
            }
        }

        let url = format!(
            "https://huggingface.co/{}/resolve/main/{}",
            item.repo, filename
        );

        progress_cb(ModelProgressEvent {
            model_id: model_id.clone(),
            status: "downloading".into(),
            percent: ((total_downloaded as f64 / estimated_total_bytes as f64) * 100.0)
                .clamp(0.0, 99.0) as f32,
            downloaded_bytes: total_downloaded,
            total_bytes: estimated_total_bytes,
            current_file: filename.to_string(),
            message: Some(format!("Connecting to download {filename}…")),
        });

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to request {filename}: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(format!("Server returned HTTP {status} for {filename}"));
        }

        let file_path = temp_dir.join(filename);
        let mut file = tokio::fs::File::create(&file_path)
            .await
            .map_err(|e| format!("Failed to create {}: {e}", file_path.display()))?;

        use tokio::io::AsyncWriteExt;
        let mut stream = response.bytes_stream();

        while let Some(chunk_res) = stream.next().await {
            if let Some(ref flag) = cancel_flag {
                if flag.load(Ordering::Relaxed) {
                    let _ = fs::remove_dir_all(&temp_dir);
                    return Err("Download cancelled".into());
                }
            }

            let chunk = chunk_res.map_err(|e| format!("Error downloading {filename}: {e}"))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Error writing {filename}: {e}"))?;

            total_downloaded += chunk.len() as u64;

            if last_progress_emit.elapsed() >= Duration::from_millis(150) {
                last_progress_emit = Instant::now();
                let effective_total = estimated_total_bytes.max(total_downloaded);
                let percent = ((total_downloaded as f64 / effective_total as f64) * 100.0)
                    .clamp(0.0, 99.0) as f32;
                progress_cb(ModelProgressEvent {
                    model_id: model_id.clone(),
                    status: "downloading".into(),
                    percent,
                    downloaded_bytes: total_downloaded,
                    total_bytes: effective_total,
                    current_file: filename.to_string(),
                    message: Some(format!(
                        "Downloading {filename} ({:.1} MB / {:.1} MB)…",
                        total_downloaded as f64 / 1_048_576.0,
                        effective_total as f64 / 1_048_576.0
                    )),
                });
            }
        }

        file.flush()
            .await
            .map_err(|e| format!("Error flushing {filename}: {e}"))?;
    }

    // Verification step
    progress_cb(ModelProgressEvent {
        model_id: model_id.clone(),
        status: "verifying".into(),
        percent: 99.5,
        downloaded_bytes: total_downloaded,
        total_bytes: total_downloaded,
        current_file: "verification".into(),
        message: Some("Verifying ONNX model integrity…".into()),
    });

    // Test load with KittenEngine
    match kittentts_rs::KittenEngine::from_dir(&temp_dir) {
        Ok(_) => {
            // Success! Move to final destination
            let final_dir = models_dir.join(&model_id);
            if final_dir.exists() {
                let _ = fs::remove_dir_all(&final_dir);
            }
            fs::rename(&temp_dir, &final_dir).map_err(|e| {
                format!(
                    "Failed to move downloaded model from {} to {}: {e}",
                    temp_dir.display(),
                    final_dir.display()
                )
            })?;

            progress_cb(ModelProgressEvent {
                model_id: model_id.clone(),
                status: "complete".into(),
                percent: 100.0,
                downloaded_bytes: total_downloaded,
                total_bytes: total_downloaded,
                current_file: "complete".into(),
                message: Some(format!("Model {model_id} downloaded and verified successfully!")),
            });

            Ok(final_dir)
        }
        Err(err) => {
            let _ = fs::remove_dir_all(&temp_dir);
            let msg = format!("Downloaded model verification failed: {err}");
            progress_cb(ModelProgressEvent {
                model_id: model_id.clone(),
                status: "error".into(),
                percent: 0.0,
                downloaded_bytes: total_downloaded,
                total_bytes: total_downloaded,
                current_file: "failed".into(),
                message: Some(msg.clone()),
            });
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_standard_models() {
        assert_eq!(CATALOG.len(), 3);
        let ids: Vec<&str> = CATALOG.iter().map(|m| m.id).collect();
        assert!(ids.contains(&"kitten-tts-nano-0.8"));
        assert!(ids.contains(&"kitten-tts-micro-0.8"));
        assert!(ids.contains(&"kitten-tts-mini-0.8"));
    }

    #[test]
    fn list_models_reports_installed_and_active() {
        let temp = std::env::temp_dir().join(format!("yapper-models-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(temp.join("kitten-tts-micro-0.8")).unwrap();
        fs::write(temp.join("kitten-tts-micro-0.8/voices.npz"), b"fake").unwrap();
        fs::write(temp.join("kitten-tts-micro-0.8/test.onnx"), b"fake").unwrap();

        let active_dir = temp.join("kitten-tts-micro-0.8");
        let list = list_models(&temp, Some(&active_dir));
        let micro = list.iter().find(|m| m.id == "kitten-tts-micro-0.8").unwrap();
        assert!(micro.installed);
        assert!(micro.active);
        assert!(micro.disk_size_bytes.is_some());

        let nano = list.iter().find(|m| m.id == "kitten-tts-nano-0.8").unwrap();
        assert!(!nano.installed);
        assert!(!nano.active);

        let _ = fs::remove_dir_all(temp);
    }
}
