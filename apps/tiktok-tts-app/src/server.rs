//! Headless LAN server: REST actions, WebSocket events and the built UI.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kittentts_rs::text::TtsSettings;
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

use crate::config;
use crate::core::{AppConfigUpdate, AppStateDto, Core, ServerEvent};

#[derive(Debug)]
struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T = Json<AppStateDto>> = Result<T, ApiError>;

pub fn router(core: Arc<Core>) -> Router {
    let api = Router::new()
        .route("/state", get(get_state))
        .route("/connect", post(connect))
        .route("/disconnect", post(disconnect))
        .route("/settings", post(update_settings))
        .route("/config", post(update_app_config))
        .route("/speak", post(speak))
        .route("/queue/skip", post(skip))
        .route("/queue/clear", post(clear))
        .route("/queue/pause", post(pause))
        .route("/ack", post(ack))
        .route("/models", get(get_models))
        .route("/models/download", post(download_model))
        .route("/models/switch", post(switch_model))
        .route("/models/delete", post(delete_model));

    let mut router = Router::new()
        .nest("/api", api)
        .route("/ws", get(ws_upgrade))
        .with_state(core);

    if let Some(dir) = config::default_ui_dir() {
        tracing::info!(ui = %dir.display(), "serving UI");
        router = router.fallback_service(
            ServeDir::new(&dir).not_found_service(ServeFile::new(dir.join("index.html"))),
        );
    } else {
        tracing::warn!("no UI build found; run `npm --prefix apps/tiktok-tts-app/ui install && npm --prefix apps/tiktok-tts-app/ui run build`");
        router = router.fallback(missing_ui);
    }

    router.layer(CorsLayer::permissive())
}

async fn get_state(State(core): State<Arc<Core>>) -> Json<AppStateDto> {
    Json(core.state())
}

#[derive(Debug, Deserialize)]
struct ConnectRequest {
    username: String,
}

async fn connect(State(core): State<Arc<Core>>, Json(request): Json<ConnectRequest>) -> ApiResult {
    core.connect(&request.username)
        .await
        .map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

async fn disconnect(State(core): State<Arc<Core>>) -> ApiResult {
    core.disconnect();
    Ok(Json(core.state()))
}

async fn update_settings(
    State(core): State<Arc<Core>>,
    Json(settings): Json<TtsSettings>,
) -> ApiResult {
    core.update_settings(settings).map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

async fn update_app_config(
    State(core): State<Arc<Core>>,
    Json(config): Json<AppConfigUpdate>,
) -> ApiResult {
    core.update_app_config(config).map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

#[derive(Debug, Deserialize)]
struct SpeakRequest {
    text: String,
}

async fn speak(State(core): State<Arc<Core>>, Json(request): Json<SpeakRequest>) -> ApiResult {
    core.speak(&request.text).map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

async fn skip(State(core): State<Arc<Core>>) -> Json<AppStateDto> {
    core.skip();
    Json(core.state())
}

async fn clear(State(core): State<Arc<Core>>) -> Json<AppStateDto> {
    core.clear_queue();
    Json(core.state())
}

#[derive(Debug, Deserialize)]
struct PauseRequest {
    paused: bool,
}

async fn pause(
    State(core): State<Arc<Core>>,
    Json(request): Json<PauseRequest>,
) -> Json<AppStateDto> {
    core.set_paused(request.paused);
    Json(core.state())
}

#[derive(Debug, Deserialize)]
struct AckRequest {
    seq: u64,
}

async fn ack(State(core): State<Arc<Core>>, Json(request): Json<AckRequest>) -> StatusCode {
    core.ack(request.seq);
    StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelActionRequest {
    model_id: String,
}

async fn get_models(State(core): State<Arc<Core>>) -> Json<Vec<crate::models::ModelDto>> {
    Json(core.get_models())
}

async fn download_model(
    State(core): State<Arc<Core>>,
    Json(request): Json<ModelActionRequest>,
) -> ApiResult {
    core.download_model(request.model_id)
        .await
        .map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

async fn switch_model(
    State(core): State<Arc<Core>>,
    Json(request): Json<ModelActionRequest>,
) -> ApiResult {
    core.switch_model(&request.model_id)
        .map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

async fn delete_model(
    State(core): State<Arc<Core>>,
    Json(request): Json<ModelActionRequest>,
) -> ApiResult {
    core.delete_model(&request.model_id)
        .map_err(ApiError::bad)?;
    Ok(Json(core.state()))
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(core): State<Arc<Core>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket_loop(socket, core))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMessage {
    Ack { seq: u64 },
}

async fn websocket_loop(mut socket: WebSocket, core: Arc<Core>) {
    let mut events = core.subscribe();
    let initial = ServerEvent::State {
        state: core.state(),
    };
    if send_event(&mut socket, &initial).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(ClientMessage::Ack { seq }) =
                            serde_json::from_str::<ClientMessage>(&text)
                        {
                            core.ack(seq);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if send_event(&mut socket, &event).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), ()> {
    let text = serde_json::to_string(event).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

async fn missing_ui() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Yapper</title>
<style>body{background:#0b0c10;color:#e6e6e6;font-family:system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0}
main{max-width:34rem;padding:2rem;border:1px solid #2a2d35;border-radius:16px;background:#14161c}
code{background:#0b0c10;padding:.15rem .4rem;border-radius:6px;color:#9ae6b4}</style></head>
<body><main><h1>Yapper server is running</h1>
<p>The web UI has not been built yet.</p>
<p><code>npm --prefix apps/tiktok-tts-app/ui install</code><br>
<code>npm --prefix apps/tiktok-tts-app/ui run build</code></p>
<p>Then restart the server or set <code>YAPPER_UI_DIR</code> to the built folder.</p>
</main></body></html>"#,
    )
}

pub fn main() {
    let config_path = config::default_config_path();
    let core = match Core::new(config_path, None, true) {
        Ok(core) => core,
        Err(error) => {
            eprintln!("Yapper could not start: {error}");
            std::process::exit(1);
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Yapper could not start its runtime: {error}");
            std::process::exit(1);
        }
    };

    runtime.block_on(async move {
        let synth = core.clone();
        tokio::spawn(async move { synth.run_synth_loop().await });

        if core.auto_connect() {
            let username = core.username();
            if !username.is_empty() {
                let core = core.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if let Err(error) = core.connect(&username).await {
                        core.log("error", error);
                    }
                });
            }
        }

        let port = cli_port().unwrap_or_else(|| core.server_port());
        let bind = std::env::var("YAPPER_BIND").unwrap_or_else(|_| format!("0.0.0.0:{port}"));
        let addr: SocketAddr = bind.parse().unwrap_or_else(|_| {
            eprintln!("invalid YAPPER_BIND `{bind}`; using 127.0.0.1:{port}");
            SocketAddr::from(([127, 0, 0, 1], port))
        });

        let listener = match TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("could not bind {addr}: {error}");
                std::process::exit(1);
            }
        };

        println!("Yapper is serving the UI and API on http://localhost:{port}");
        println!("On your LAN, open http://<this-computer-ip>:{port}");
        println!("Model: {}", core.state().engine.model);

        if let Err(error) = axum::serve(listener, router(core)).await {
            eprintln!("server stopped: {error}");
        }
    });
}

fn cli_port() -> Option<u16> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--port" {
            return args.next().and_then(|value| value.parse().ok());
        }
        if let Some(value) = arg.strip_prefix("--port=") {
            return value.parse().ok();
        }
    }
    None
}
