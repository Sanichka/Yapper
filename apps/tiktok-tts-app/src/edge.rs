//! Microsoft Edge online voice engine for languages KittenTTS does not cover.
//!
//! KittenTTS 0.8 only speaks English, so Ukrainian chat is synthesized with
//! Microsoft Edge neural voices over the internet (no API key, no login). The
//! returned MP3 is decoded to 24 kHz mono PCM, the same format the rest of the
//! pipeline uses.
//!
//! Uses `tokio-tungstenite` with `rustls-tls-webpki-roots` so it works across
//! desktop and mobile (Android/iOS) without requiring OS platform verifiers.

use std::sync::OnceLock;

use futures_util::{SinkExt, StreamExt};
use kittentts_rs::{
    EngineKind, EngineStatus, SynthRequest, TtsEngine, TtsError, SAMPLE_RATE,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

#[path = "edge_drm.rs"]
mod drm;

const EDGE_AUDIO_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// rustls ships both ring and aws-lc crypto in this workspace, so the default
/// provider must be installed explicitly before building a TLS client.
pub fn ensure_default_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
    });
}

#[derive(Default)]
pub struct EdgeEngine;

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
        let rate =
            (((request.speed.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32).clamp(-50, 100);

        let audio_bytes = match tokio::runtime::Handle::try_current() {
            Ok(handle) => match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::MultiThread => {
                    tokio::task::block_in_place(|| {
                        handle.block_on(synthesize_via_edge(text, &request.voice, rate))
                    })?
                }
                _ => std::thread::scope(|s| {
                    s.spawn(|| {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| TtsError::Service(e.to_string()))?;
                        rt.block_on(synthesize_via_edge(text, &request.voice, rate))
                    })
                    .join()
                    .map_err(|_| TtsError::Service("Edge TTS worker thread panicked".into()))?
                })?,
            },
            Err(_) => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| TtsError::Service(e.to_string()))?;
                rt.block_on(synthesize_via_edge(text, &request.voice, rate))?
            }
        };

        decode_mp3(&audio_bytes)
    }
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

pub async fn synthesize_via_edge(text: &str, voice: &str, rate: i32) -> kittentts_rs::Result<Vec<u8>> {
    ensure_default_crypto_provider();

    let connection_id = uuid::Uuid::new_v4().simple().to_string();
    let sec_ms_gec = drm::generate_sec_ms_gec();
    let url_str = format!(
        "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1?TrustedClientToken=6A5AA1D4EAFF4E9FB37E23D68491D6F4&ConnectionId={connection_id}&Sec-MS-GEC={sec_ms_gec}&Sec-MS-GEC-Version=2-155.0.4283.45"
    );

    let mut request = url_str
        .into_client_request()
        .map_err(|e| TtsError::Service(e.to_string()))?;
    let headers = request.headers_mut();
    headers.insert("Pragma", "no-cache".parse().unwrap());
    headers.insert("Cache-Control", "no-cache".parse().unwrap());
    headers.insert(
        "User-Agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36 Edg/155.0.0.0".parse().unwrap(),
    );
    headers.insert(
        "Origin",
        "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".parse().unwrap(),
    );

    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| TtsError::Service(e.to_string()))?;

    let timestamp = chrono::Utc::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string();
    let config_msg = format!(
        "X-Timestamp:{timestamp}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"true\"}},\"outputFormat\":\"{EDGE_AUDIO_FORMAT}\"}}}}}}}}"
    );
    ws.send(Message::Text(config_msg.into()))
        .await
        .map_err(|e| TtsError::Service(e.to_string()))?;

    let req_id = uuid::Uuid::new_v4().simple().to_string();
    let escaped_text = escape_xml(text);
    let ssml = format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'><voice name='{voice}'><prosody pitch='{:+0}Hz' rate='{:+}%' volume='{:+0}%'>{escaped_text}</prosody></voice></speak>",
        0, rate, 0
    );
    let ssml_msg = format!(
        "X-RequestId:{req_id}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{timestamp}Z\r\nPath:ssml\r\n\r\n{ssml}"
    );
    ws.send(Message::Text(ssml_msg.into()))
        .await
        .map_err(|e| TtsError::Service(e.to_string()))?;

    let mut audio_bytes = Vec::new();
    while let Some(msg) = ws.next().await {
        let msg = msg.map_err(|e| TtsError::Service(e.to_string()))?;
        match msg {
            Message::Binary(bytes) => {
                if bytes.len() > 2 {
                    let header_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
                    if bytes.len() >= header_len + 2 {
                        audio_bytes.extend_from_slice(&bytes[header_len + 2..]);
                    }
                }
            }
            Message::Text(text) => {
                if text.contains("turn.end") {
                    break;
                }
            }
            Message::Close(frame) => {
                eprintln!("Edge TTS closed: {:?}", frame);
                break;
            }
            _ => {}
        }
    }

    if audio_bytes.is_empty() {
        return Err(TtsError::EmptyAudio);
    }
    Ok(audio_bytes)
}

fn decode_mp3(bytes: &[u8]) -> kittentts_rs::Result<Vec<f32>> {
    if bytes.is_empty() {
        return Err(TtsError::EmptyAudio);
    }
    let decoded = nanomp3::decode_all::<f32>(bytes);
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

/// Simple linear resampler; Edge usually returns 24 kHz already.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_edge_synthesize_live() {
        let engine = EdgeEngine;
        let req = SynthRequest {
            text: "Привіт".into(),
            voice: "uk-UA-PolinaNeural".into(),
            speed: 1.0,
        };
        let res = engine.synthesize(&req);
        assert!(res.is_ok(), "synthesis failed: {:?}", res.err());
        let samples = res.unwrap();
        assert!(!samples.is_empty());
    }
}