# Yapper

TikTok LIVE chat and gifts, spoken out loud in a local neural voice.

Yapper connects to a TikTok LIVE stream without a TikTok login, filters and cleans chat
messages, and runs **KittenTTS** locally through ONNX Runtime. The same Rust core powers a
desktop app (Tauri), a headless LAN server, and a browser UI. Live connections use
EulerStream's public endpoint for the required WebSocket signature; no Python or Node.js is
needed at runtime.

## Features

- **Login-free TikTok LIVE client** — resolves the room, signs the WebSocket via
  EulerStream, fetches chat, and decodes protobuf messages (`WebcastChatMessage`, gifts,
  likes, members, follows, shares).
- **HTTP 429 & connection resilience** — automatic fallback to HTML room scraping if
  TikTok rate-limits the room API, plus exponential backoff with jitter on reconnects.
- **Local neural TTS** — KittenTTS mini 0.8 ONNX, 24 kHz, eight expressive voices,
  adjustable speed, CPU inference.
- **In-app model manager** — download and activate neural models directly inside the
  desktop or mobile UI without needing command-line tools.
- **Spoken language & mixed-language synthesis** — choose `auto` (English / Ukrainian), English (local
  KittenTTS), or Ukrainian (Microsoft Edge online). In `auto` mode, mixed-script sentences
  (e.g. English + Ukrainian words) are seamlessly split, synthesized with their matching voices,
  and chained into a unified audio utterance.
- **Readable chat** — emote/mention stripping, URL and repeat collapsing, blacklist,
  transliteration, per-user cooldown, length limit, `{user}` / `{text}` templates.
- **Gift announcements** — reads gift streaks with `{user}`, `{gift}`, `{count}`,
  minimum diamond filter.
- **Bounded priority queue** — chat and gifts in two lanes, newest kept when full,
  pause / skip / clear, one utterance at a time.
- **Cross-platform** — native desktop Tauri app (Windows, macOS, Linux), Android mobile app
  (~32 MB optimized APK), or `--server` headless LAN mode with browser dashboard.
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

### Option 1: In-App Download (Recommended)

In the desktop or mobile app dashboard, open the **Models** dialog from the status bar or settings. Click **Download** next to **KittenTTS mini 0.8** to fetch the ONNX weights, voice embeddings, and config automatically into your local data directory.

### Option 2: Manual Download

You can also download the files manually from
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

### Spoken language

KittenTTS 0.8 natively speaks English. The **Spoken language** setting chooses how chat is
voiced:

| Setting | Behavior |
| --- | --- |
| `auto` (default) | Intelligently routes languages: Latin messages use the local Kitten voice; Cyrillic messages use a Ukrainian Edge voice. Mixed sentences (e.g. *"Hello friend це гарний стрім"*) are split into language chunks and voiced with their respective engines in a single continuous utterance. |
| `en` | Everything is spoken by the local Kitten voice (non-Latin scripts are transliterated for speech). |
| `uk` | Everything is spoken by a Microsoft Edge Ukrainian neural voice (`Polina` / `Ostap`) over the internet — no API key, no login. |

Chat is always displayed in its original script; transliteration only ever affects the
spoken text when English is active. Ukrainian voices require a working internet
connection (Edge Read Aloud service).


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
| `POST` | `/api/config` | `{"queueCapacity":15,"autoConnect":false,"serverPort":8787}` |
| `POST` | `/api/speak` | `{"text":"Hello"}` |
| `POST` | `/api/queue/skip` | — |
| `POST` | `/api/queue/clear` | — |
| `POST` | `/api/queue/pause` | — |
| `POST` | `/api/ack` | — |
| `GET` | `/api/models` | — (returns catalog, installed status, active model) |
| `POST` | `/api/models/download` | `{"modelId":"kitten-tts-mini-0.8"}` |
| `POST` | `/api/models/switch` | `{"modelId":"kitten-tts-mini-0.8"}` |
| `POST` | `/api/models/delete` | `{"modelId":"kitten-tts-mini-0.8"}` |
| `GET` | `/ws` | WebSocket events |

The WebSocket streams `state`, `activity`, `utterance` and `log` events (tagged with
`type`). Each `utterance` carries a sequence number, duration and base64 WAV; the client
sends `{"type":"ack","seq":N}` when playback ends, which releases the queue. A missing ack
times out after the audio duration plus five seconds.

Settings JSON (all fields optional when omitted by clients):

```json
{
  "language": "auto",
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

Settings, the last username, queue capacity, auto-connect and server port live in
`config.json` in the data directory. Queue capacity and auto-connect can be changed live
from **App settings** in the UI; the server port applies the next time server mode starts.
The file is written atomically and can be edited while Yapper is stopped:

```json
{
  "username": "creator",
  "autoConnect": false,
  "serverPort": 8787,
  "queueCapacity": 15,
  "modelDir": null,
  "settings": {
    "language": "auto",
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
`Hugo`, `Kiki`, `Leo`, or the raw model names such as `expr-voice-2-f`. When the spoken
language is Ukrainian, choose `uk-UA-PolinaNeural` or `uk-UA-OstapNeural`.

## Android

Yapper runs natively on Android (`arm64-v8a`) powered by Tauri v2 with native background streaming:

- **Background Execution (Foreground Service)**: Keeps CPU and Wi-Fi alive (`WakeLock` + `WifiLock`) while streaming so Yapper continues listening to TikTok LIVE chat and synthesizing speech even when minimized or when you switch over to the TikTok app.
- **Notification Shade Media Controller**: Persistent interactive Android notification shade controller displaying real-time stream state (`Yapper • Connected to @creator`), queue depth, and interactive Pause / Resume / Skip buttons.
- **Hardware-Direct AudioTrack Playback**: Bypasses WebView audio downmixers with native low-latency stereo 16-bit PCM streaming directly to device speakers, with immediate mid-sentence interruption on Skip.
- **Compact Release Size**: Release builds use LTO, symbol stripping, and ProGuard optimization to keep the APK around **~32 MB** while embedding full ONNX inference and local neural synthesis.
- **In-App Model Download**: No manual file transfers needed. Tap the **Models** badge in the mobile UI to download KittenTTS directly to the device's internal storage over Wi-Fi.
- **Bundled Dependencies**: The APK bundles `libc++_shared.so` alongside the Rust binary to ensure smooth ONNX Runtime compatibility across Android 7.0+ (API 24+).

### Building the APK locally

Prerequisites:
- Android SDK with platform tools & NDK (r27+ recommended)
- Java 17+ (or JDK 23)
- `rustup target add aarch64-linux-android`

#### One-Click Build Script (Windows)

```powershell
powershell -ExecutionPolicy Bypass -File .\.agents\skills\android-apk-build\scripts\build-apk.ps1
```

The script automatically builds the frontend, cross-compiles the Rust release `cdylib`, bundles `libc++_shared.so`, runs Gradle assembly, and copies the signed APK to `release/yapper-v0.0.2-arm64.apk`.

#### Manual Step-by-Step Build

```powershell
$env:JAVA_HOME = "C:\Program Files\Java\jdk-23"
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
$env:NDK_HOME = "$env:ANDROID_HOME\ndk\27.0.12077973"
$env:ANDROID_NDK_HOME = $env:NDK_HOME

# 1. Build frontend
cd apps\tiktok-tts-app\ui
npm run build
cd ..\..\..

# 2. Cross-compile Rust cdylib
cargo build --package tiktok-tts-app --target aarch64-linux-android --release

# 3. Copy binary and libc++ to jniLibs (bypasses Windows symlink restrictions)
$jniDir = "apps\tiktok-tts-app\gen\android\app\src\main\jniLibs\arm64-v8a"
New-Item -ItemType Directory -Force -Path $jniDir | Out-Null
Copy-Item "target\aarch64-linux-android\release\libtiktok_tts_app_lib.so" -Destination "$jniDir\" -Force
$libcxx = Get-ChildItem -Path $env:NDK_HOME -Recurse -Filter "libc++_shared.so" | Where-Object { $_.FullName -like "*aarch64*" } | Select-Object -First 1
Copy-Item $libcxx.FullName -Destination "$jniDir\" -Force

# 4. Assemble release APK with Gradle
cd apps\tiktok-tts-app\gen\android
.\gradlew.bat assembleArm64Release -x rustBuildArm64Release -x rustBuildUniversalRelease
cd ..\..\..
```

The resulting signed APK is saved to `apps/tiktok-tts-app/gen/android/app/build/outputs/apk/arm64/release/app-arm64-release.apk` and copied to `release/yapper-v0.0.2-arm64.apk`.


## Releases and CI

[`.github/workflows/build.yml`](.github/workflows/build.yml) builds the desktop bundles
(Windows NSIS, macOS DMG, Linux DEB) and an Android arm64 APK. The Linux DEB is built on
Ubuntu 24.04 because the prebuilt ONNX Runtime requires glibc 2.38 or newer.

The workflow only builds when the pushed commit message contains the version from
`Cargo.toml` (for example `Release v0.0.2`) or when a matching `v<version>` tag is pushed.
`workflow_dispatch` always builds. Tags additionally publish a GitHub release with every
artifact. The version gate fails early if `Cargo.toml`, `tauri.conf.json`, or the UI
package versions disagree.

To cut a release:

```bash
# bump the version in Cargo.toml, tauri.conf.json, ui/package.json and ui/package-lock.json
git commit -am "Release v0.0.2"
git push origin main
git tag -a v0.0.2 -m "Yapper v0.0.2"
git push origin v0.0.2
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
- WebSocket signing uses [EulerStream](https://eulerstream.com/)'s public endpoint. It is
  rate-limited and depends on a third-party service; set `SIGN_API_URL`/`SIGN_API_KEY`
  when using a compatible alternative or paid tier.
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
- [EulerStream](https://eulerstream.com/) for the public webcast signing endpoint.
- [ort](https://github.com/pykeio/ort), [tauri](https://tauri.app),
  [axum](https://github.com/tokio-rs/axum) and the Rust ecosystem.
