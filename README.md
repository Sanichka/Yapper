# Yapper

TikTok LIVE chat and gifts, spoken out loud in a local neural voice.

Yapper connects anonymously to a TikTok LIVE stream (no login, no API keys), filters and
cleans chat messages, and runs **KittenTTS** locally through ONNX Runtime. The same Rust
core powers a desktop app (Tauri), a headless LAN server, and a browser UI — no Python,
Node.js or cloud service is needed at runtime.

## Features

- **Anonymous TikTok LIVE client** — resolves the room, fetches chat, decodes protobuf
  messages (`WebcastChatMessage`, gifts, likes, members, follows, shares).
- **Local neural TTS** — KittenTTS mini 0.8 ONNX, 24 kHz, eight expressive voices,
  adjustable speed, CPU inference.
- **Readable chat** — emote/mention stripping, URL and repeat collapsing, blacklist,
  transliteration, per-user cooldown, length limit, `{user}` / `{text}` templates.
- **Gift announcements** — reads gift streaks with `{user}`, `{gift}`, `{count}`,
  minimum diamond filter.
- **Bounded priority queue** — chat and gifts in two lanes, newest kept when full,
  pause / skip / clear, one utterance at a time.
- **Two shells, one core** — native Tauri window or `--server` mode with REST + WebSocket
  control and the same UI served over the LAN.
- **Plays everywhere** — Web Audio playback in the browser and Tauri webview; an explicit
  acknowledgement drives the queue so audio and state stay in sync.

## Workspace layout

```text
crates/tiklive-rs/        TikTok LIVE connector (HTTP, protobuf, WebSocket)
crates/kittentts-rs/      KittenTTS tokenizer, phonemizer, ONNX engine, queue, WAV
apps/tiktok-tts-app/      Tauri app + LAN server + config
apps/tiktok-tts-app/ui/   TypeScript/Vite dashboard
models/                   Local model files (gitignored)
```

## Requirements

- Rust 1.89+ (2024 edition)
- Node.js 20+ and npm (only to build the UI)
- CMake not required; ONNX Runtime binaries are downloaded by the `ort` crate
- Optional: `cargo install tauri-cli --locked` for `cargo tauri ...`

## Get the model

Download the files from
[`KittenML/kitten-tts-mini-0.8`](https://huggingface.co/KittenML/kitten-tts-mini-0.8)
into `models/kitten-tts-mini-0.8/`:

```powershell
$dir = "models/kitten-tts-mini-0.8"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$base = "https://huggingface.co/KittenML/kitten-tts-mini-0.8/resolve/main"
Invoke-WebRequest "$base/kitten_tts_mini_v0_8.onnx" -OutFile "$dir/kitten_tts_mini_v0_8.onnx"
Invoke-WebRequest "$base/voices.npz" -OutFile "$dir/voices.npz"
Invoke-WebRequest "$base/config.json" -OutFile "$dir/config.json"
```

```bash
mkdir -p models/kitten-tts-mini-0.8
cd models/kitten-tts-mini-0.8
curl -LO https://huggingface.co/KittenML/kitten-tts-mini-0.8/resolve/main/kitten_tts_mini_v0_8.onnx
curl -LO https://huggingface.co/KittenML/kitten-tts-mini-0.8/resolve/main/voices.npz
curl -LO https://huggingface.co/KittenML/kitten-tts-mini-0.8/resolve/main/config.json
```

Yapper searches, in order: `YAPPER_MODEL_DIR`, the `modelDir` value in the config,
`<data dir>/models/kitten-tts-mini-0.8`, `./models/kitten-tts-mini-0.8`, `./models`, and
next to the executable. Without a model it falls back to a quiet test tone so the UI and
chat flow can still be exercised.

## Build the UI

```bash
npm --prefix apps/tiktok-tts-app/ui install
npm --prefix apps/tiktok-tts-app/ui run build
```

## Desktop app

```bash
cargo install tauri-cli --locked   # once
cd apps/tiktok-tts-app
cargo tauri dev                    # development window
cargo tauri build                  # platform bundles
```

Without the Tauri CLI, `cargo run -p tiktok-tts-app` from the repository root launches the
same development window. The desktop build is the default feature set (`desktop` + `onnx`).

## Headless LAN server

A server-only binary (no Tauri) is useful for a streaming PC or a small box on the LAN:

```bash
cargo build -p tiktok-tts-app --release --no-default-features --features onnx
./target/release/tiktok-tts-app --server --port 8787
```

The same binary built with default features also accepts `--server`:

```bash
cargo build -p tiktok-tts-app --release
./target/release/tiktok-tts-app --server
```

Open `http://<host>:8787`. The UI is served from `ui/dist` (searched relative to the
executable and the working directory); set `YAPPER_UI_DIR` to override it. Connect,
change the voice, and watch the queue from any browser on the network.

Server flags:

| Flag | Meaning |
| --- | --- |
| `--server` | Run the headless server instead of the Tauri window |
| `--port <PORT>` | Override `serverPort` from the config |
| `--model-dir <PATH>` | Override model discovery |

Environment variables:

| Variable | Purpose |
| --- | --- |
| `YAPPER_CONFIG` | Config file path (default: OS data dir `config.json`) |
| `YAPPER_DATA_DIR` | Data directory (default: `%APPDATA%\Yapper`, `~/Library/Application Support/Yapper`, `~/.local/share/yapper`) |
| `YAPPER_MODEL_DIR` | Model directory |
| `YAPPER_UI_DIR` | Directory with the built UI |
| `YAPPER_BIND` | Bind address (default `0.0.0.0`) |
| `YAPPER_ORT_DYLIB` | `libonnxruntime` path for the `onnx-dynamic` build |
| `RUST_LOG` | Log filter (default `info,tower_http=warn,ort=warn`) |

The server binds `0.0.0.0` by default and enables permissive CORS, so only expose it on a
trusted network.

## REST and WebSocket API

All responses are the same application state object the UI renders.

| Method | Path | Body |
| --- | --- | --- |
| `GET` | `/api/state` | — |
| `POST` | `/api/connect` | `{"username":"creator"}` |
| `POST` | `/api/disconnect` | — |
| `POST` | `/api/settings` | `TtsSettings` JSON |
| `POST` | `/api/speak` | `{"text":"Hello"}` |
| `POST` | `/api/queue/skip` | — |
| `POST` | `/api/queue/clear` | — |
| `POST` | `/api/queue/pause` | — |
| `POST` | `/api/ack` | — |
| `GET` | `/ws` | WebSocket events |

The WebSocket streams `state`, `activity`, `utterance` and `log` events (tagged with
`type`). Each `utterance` carries a sequence number, duration and base64 WAV; the client
sends `{"type":"ack","seq":N}` when playback ends, which releases the queue. A missing ack
times out after the audio duration plus five seconds.

Settings JSON (all fields optional when omitted by clients):

```json
{
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
```

## Configuration

Settings, the last username and server port live in `config.json` in the data directory.
The file is written atomically and can be edited while Yapper is stopped:

```json
{
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
}
```

Voice names are case-insensitive and include `Bella`, `Jasper`, `Luna`, `Bruno`, `Rosie`,
`Hugo`, `Kiki`, `Leo`, or the raw model names such as `expr-voice-2-f`.

## Android (experimental)

The intended mobile path is Tauri's Android target with a runtime-loaded ONNX Runtime:

1. Build with `--no-default-features --features onnx-dynamic` (or add it to the feature
   list) and ship `libonnxruntime.so` for each ABI.
2. Point `YAPPER_ORT_DYLIB` at the library, or place it next to the app library.
3. Add `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MEDIA_PLAYBACK`, `INTERNET` and
   `WAKE_LOCK` permissions to the generated manifest.

This path is not built or tested in CI yet. The CI workflow instead builds the default
feature set for `aarch64`, which downloads the Android ONNX Runtime binary through `ort`
and produces a debug-signed APK.

## Releases and CI

[`.github/workflows/build.yml`](.github/workflows/build.yml) builds the desktop bundles
(Windows NSIS, macOS DMG, Linux DEB/AppImage) and an Android arm64 APK.

The workflow only builds when the pushed commit message contains the version from
`Cargo.toml` (for example `Release v0.0.1`) or when a matching `v<version>` tag is pushed.
`workflow_dispatch` always builds. Tags additionally publish a GitHub release with every
artifact. The version gate fails early if `Cargo.toml`, `tauri.conf.json`, or the UI
package versions disagree.

To cut a release:

```bash
# bump the version in Cargo.toml, tauri.conf.json, ui/package.json and ui/package-lock.json
git commit -am "Release v0.0.1"
git push origin main
git tag -a v0.0.1 -m "Yapper v0.0.1"
git push origin v0.0.1
```

The Android artifact is a debug-signed APK for testing; configure a keystore to sign a
Play Store release. Desktop artifacts are unsigned unless signing secrets are configured.

## Development

```bash
cargo test --workspace
cargo check -p tiktok-tts-app
cargo build -p tiktok-tts-app --no-default-features --features onnx
cargo run -p kittentts-rs --example say -- "Hello from Yapper"
```

The UI dev server runs on port 1420 and is wired to `cargo tauri dev`; for the LAN server
run `npm run build` instead, since the Rust server serves static files.

## Notes and limitations

- TikTok's live protocol is unofficial and can change without notice.
- Gift streaks are read when they complete; very high-traffic rooms may drop messages by
  design (bounded queue, newest wins).
- KittenTTS is a small model; quality is best at moderate speed with clean English text.
  Check the model card for its license before redistribution.
- The model files are not part of this repository.

## Credits

- [KittenTTS](https://github.com/KittenML/KittenTTS) by KittenML for the model and
  tokenizer/phonemizer design.
- [TikTokLive](https://github.com/isaackogan/TikTokLive) for documenting the live
  protocol.
- [ort](https://github.com/pykeio/ort), [tauri](https://tauri.app),
  [axum](https://github.com/tokio-rs/axum) and the Rust ecosystem.
