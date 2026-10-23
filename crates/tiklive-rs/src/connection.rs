//! A live connection to one TikTok LIVE room.
//!
//! TikTok's webcast WebSocket requires a signed URL. The signed route data is
//! fetched from EulerStream, then the initial event batch and signed cookie are
//! used for the native WebSocket connection.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{
    HeaderValue, ACCEPT_ENCODING, ACCEPT_LANGUAGE, CACHE_CONTROL, COOKIE, ORIGIN, REFERER,
    USER_AGENT,
};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};
use url::Url;

use crate::error::LiveError;
use crate::events::{LiveEvent, RoomStatus};
use crate::proto::{
    decode_message, decode_socket_frame, encode_ack, encode_enter_room, encode_heartbeat,
    FetchResult, WS_PATH,
};
use crate::web::{normalize_username, TikTokWeb};

pub const DEFAULT_WS_HOSTS: [&str; 2] = ["webcast-ws.tiktok.com", "webcast-ws.us.tiktok.com"];

const DEFAULT_MAX_ATTEMPTS: u32 = 10;

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Skip the first room lookup and join this room directly.
    pub room_id: Option<String>,
    /// Hosts for the `TIKLIVE_DIRECT` unsigned fallback. Signed connections use
    /// the push server returned by the signer instead.
    pub ws_hosts: Vec<String>,
    /// Emit the few recent comments TikTok replays on join.
    pub include_history: bool,
    /// Reconnect after unexpected drops.
    pub reconnect: bool,
    pub max_attempts: u32,
    pub max_delay: Duration,
    pub heartbeat_interval: Duration,
    /// Treat the socket as dropped when nothing arrives for this long.
    pub idle_timeout: Duration,
    /// Timeout for the room lookup, each cookie request and each socket open.
    pub connect_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            room_id: None,
            ws_hosts: DEFAULT_WS_HOSTS.iter().map(|h| (*h).to_string()).collect(),
            include_history: false,
            reconnect: true,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            max_delay: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The socket dropped unexpectedly. Followed by a reconnect when `will_reconnect`.
    Network,
    /// TikTok sent a stream-ended control message.
    StreamEnd,
    /// On reconnect, the room lookup said the user is no longer live.
    Offline,
}

#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    Connected {
        room_id: String,
        /// `None` only when connecting with an explicit `room_id`.
        room: Option<RoomStatus>,
        ws_host: String,
    },
    Event(LiveEvent),
    Disconnected {
        reason: DisconnectReason,
        will_reconnect: bool,
        detail: Option<String>,
    },
    /// A fatal failure. No further events follow.
    Error {
        message: String,
        kind: ErrorKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Offline,
    RateLimited,
    Network,
    Protocol,
}

/// Owns the background connection task and the event stream.
pub struct LiveSession {
    pub room_id: Option<String>,
    rx: mpsc::Receiver<ConnectionEvent>,
    shutdown: Arc<Notify>,
    handle: JoinHandle<()>,
}

/// A cloneable way to stop a [`LiveSession`] whose receiver lives inside a
/// reader task.
#[derive(Clone)]
pub struct SessionControl {
    shutdown: Arc<Notify>,
    handle: tokio::task::AbortHandle,
}

impl SessionControl {
    pub fn disconnect(&self) {
        self.shutdown.notify_waiters();
        self.handle.abort();
    }
}

impl LiveSession {
    /// Next event, or `None` once the connection task has finished.
    pub async fn recv(&mut self) -> Option<ConnectionEvent> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<ConnectionEvent> {
        self.rx.try_recv().ok()
    }

    /// Stops the connection immediately.
    pub fn disconnect(&self) {
        self.shutdown.notify_waiters();
        self.handle.abort();
    }

    /// Handle for stopping the connection from elsewhere.
    pub fn control(&self) -> SessionControl {
        SessionControl {
            shutdown: self.shutdown.clone(),
            handle: self.handle.abort_handle(),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

/// A signed-out TikTok web client that can open LIVE event streams.
#[derive(Clone)]
pub struct LiveClient {
    web: TikTokWeb,
}

impl LiveClient {
    pub fn new(user_agent: Option<String>) -> Result<Self, LiveError> {
        Ok(Self {
            web: TikTokWeb::new(user_agent)?,
        })
    }

    pub fn web(&self) -> &TikTokWeb {
        &self.web
    }

    /// Connects to `username`'s LIVE and streams lifecycle + events.
    pub fn connect(&self, username: &str, options: ConnectOptions) -> LiveSession {
        let web = self.web.clone();
        let username = normalize_username(username);
        let (tx, rx) = mpsc::channel(1024);
        let shutdown = Arc::new(Notify::new());
        let cancel = shutdown.clone();

        let handle = tokio::spawn(async move {
            run(web, username, options, tx, cancel).await;
        });

        LiveSession {
            room_id: None,
            rx,
            shutdown,
            handle,
        }
    }
}

#[derive(Debug)]
enum AttemptError {
    Offline,
    Dropped(String),
    Open(String),
    Request(LiveError),
}

enum SocketEnd {
    StreamEnd,
    Cancelled,
}

struct SocketTarget {
    label: String,
    url: Url,
    cookie: String,
}

struct SocketPlan {
    targets: Vec<SocketTarget>,
    initial: Option<FetchResult>,
}

async fn run(
    web: TikTokWeb,
    username: String,
    options: ConnectOptions,
    tx: mpsc::Sender<ConnectionEvent>,
    shutdown: Arc<Notify>,
) {
    let host_list = if options.ws_hosts.is_empty() {
        DEFAULT_WS_HOSTS
            .iter()
            .map(|h| (*h).to_string())
            .collect::<Vec<_>>()
    } else {
        options.ws_hosts.clone()
    };

    let mut explicit_room_id = options.room_id.clone();
    let mut seen = RecentIds::new(5000);
    let mut ever_connected = false;
    let mut failures: u32 = 0;
    let mut refresh_ttwid = false;

    loop {
        let result = attempt(
            &web,
            &username,
            &options,
            &host_list,
            &mut explicit_room_id,
            &mut seen,
            &mut ever_connected,
            refresh_ttwid,
            &tx,
            &shutdown,
        )
        .await;

        match result {
            Ok(SocketEnd::StreamEnd) => {
                emit(
                    &tx,
                    ConnectionEvent::Disconnected {
                        reason: DisconnectReason::StreamEnd,
                        will_reconnect: false,
                        detail: None,
                    },
                )
                .await;
                return;
            }
            Ok(SocketEnd::Cancelled) => return,
            Err(AttemptError::Offline) => {
                if ever_connected {
                    emit(
                        &tx,
                        ConnectionEvent::Disconnected {
                            reason: DisconnectReason::Offline,
                            will_reconnect: false,
                            detail: None,
                        },
                    )
                    .await;
                } else {
                    emit(
                        &tx,
                        ConnectionEvent::Error {
                            message: format!("@{username} is not live right now"),
                            kind: ErrorKind::Offline,
                        },
                    )
                    .await;
                }
                return;
            }
            Err(AttemptError::Request(err)) => {
                if !ever_connected {
                    emit(
                        &tx,
                        ConnectionEvent::Error {
                            kind: classify(&err),
                            message: err.to_string(),
                        },
                    )
                    .await;
                    return;
                }
                warn!("room/cookie request failed after connect: {err}");
                refresh_ttwid = true;
            }
            Err(AttemptError::Open(detail)) => {
                if !ever_connected {
                    emit(
                        &tx,
                        ConnectionEvent::Error {
                            kind: ErrorKind::Network,
                            message: format!("could not open the TikTok socket: {detail}"),
                        },
                    )
                    .await;
                    return;
                }
                warn!("socket open failed: {detail}");
                refresh_ttwid = true;
            }
            Err(AttemptError::Dropped(detail)) => {
                let will_reconnect = options.reconnect && failures < options.max_attempts;
                emit(
                    &tx,
                    ConnectionEvent::Disconnected {
                        reason: DisconnectReason::Network,
                        will_reconnect,
                        detail: Some(detail.clone()),
                    },
                )
                .await;
                if !will_reconnect {
                    emit(
                        &tx,
                        ConnectionEvent::Error {
                            kind: ErrorKind::Network,
                            message: format!(
                                "reconnect failed after {failures} attempts: {detail}"
                            ),
                        },
                    )
                    .await;
                    return;
                }
            }
        }

        if !(options.reconnect && failures < options.max_attempts) {
            return;
        }

        let base = 1000u64.saturating_mul(1u64 << failures.min(16));
        let capped = base.min(options.max_delay.as_millis() as u64);
        let jitter = 0.8 + rand::random::<f64>() * 0.4;
        let delay = Duration::from_millis((capped as f64 * jitter) as u64);
        failures += 1;
        debug!("reconnecting in {delay:?} (attempt {failures})");

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.notified() => return,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn attempt(
    web: &TikTokWeb,
    username: &str,
    options: &ConnectOptions,
    hosts: &[String],
    explicit_room_id: &mut Option<String>,
    seen: &mut RecentIds,
    ever_connected: &mut bool,
    refresh_ttwid: bool,
    tx: &mpsc::Sender<ConnectionEvent>,
    shutdown: &Arc<Notify>,
) -> Result<SocketEnd, AttemptError> {
    let timeout = options.connect_timeout;

    let (room_id, room) = if let Some(room_id) = explicit_room_id.take() {
        (room_id, None)
    } else {
        let room = match tokio::time::timeout(timeout, web.room_status(username)).await {
            Err(_) => {
                return Err(AttemptError::Request(LiveError::RoomLookup(
                    "room lookup timed out".into(),
                )))
            }
            Ok(Err(err)) => return Err(AttemptError::Request(err)),
            Ok(Ok(room)) => room,
        };
        if !room.is_live || room.room_id.is_empty() {
            return Err(AttemptError::Offline);
        }
        (room.room_id.clone(), Some(room))
    };

    let direct = std::env::var_os("TIKLIVE_DIRECT").is_some();
    let plan = if direct {
        let ttwid = match tokio::time::timeout(timeout, web.ttwid(refresh_ttwid)).await {
            Err(_) => {
                return Err(AttemptError::Request(LiveError::RoomLookup(
                    "device cookie request timed out".into(),
                )))
            }
            Ok(result) => result.map_err(AttemptError::Request)?,
        };
        let cookie = format!("ttwid={}", ttwid.replace("%7C", "|"));
        SocketPlan {
            targets: hosts
                .iter()
                .map(|host| SocketTarget {
                    label: host.clone(),
                    url: build_ws_url(host, &room_id, &options.heartbeat_interval),
                    cookie: cookie.clone(),
                })
                .collect(),
            initial: None,
        }
    } else {
        let signed = match tokio::time::timeout(timeout, web.signed_socket(&room_id)).await {
            Err(_) => {
                return Err(AttemptError::Request(LiveError::Signing(
                    "sign request timed out".into(),
                )))
            }
            Ok(result) => result.map_err(AttemptError::Request)?,
        };
        let url = build_signed_ws_url(
            &signed.push_server,
            &room_id,
            &signed.initial,
            web.user_agent(),
            &options.heartbeat_interval,
        )?;
        let label = url.host_str().unwrap_or("signed").to_string();
        SocketPlan {
            targets: vec![SocketTarget {
                label,
                url,
                cookie: signed.cookie,
            }],
            initial: Some(signed.initial),
        }
    };

    run_socket(
        &room_id,
        room.as_ref(),
        web.user_agent(),
        plan,
        options,
        seen,
        ever_connected,
        tx,
        shutdown,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_socket(
    room_id: &str,
    room: Option<&RoomStatus>,
    user_agent: &str,
    plan: SocketPlan,
    options: &ConnectOptions,
    seen: &mut RecentIds,
    ever_connected: &mut bool,
    tx: &mpsc::Sender<ConnectionEvent>,
    shutdown: &Arc<Notify>,
) -> Result<SocketEnd, AttemptError> {
    let room_num: u64 = room_id
        .parse()
        .map_err(|_| AttemptError::Open(format!("invalid room id `{room_id}`")))?;

    let mut errors = Vec::new();
    let mut opened = None;

    for target in &plan.targets {
        let mut request = target
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| AttemptError::Open(format!("invalid websocket URL: {e}")))?;
        let headers = request.headers_mut();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(user_agent).unwrap_or(HeaderValue::from_static("Mozilla/5.0")),
        );
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&target.cookie)
                .map_err(|_| AttemptError::Open("invalid cookie".into()))?,
        );
        headers.insert(ORIGIN, HeaderValue::from_static("https://www.tiktok.com"));
        headers.insert(REFERER, HeaderValue::from_static("https://www.tiktok.com/"));
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));

        match tokio::time::timeout(
            options.connect_timeout,
            tokio_tungstenite::connect_async(request),
        )
        .await
        {
            Err(_) => errors.push(format!("{}: connect timed out", target.label)),
            Ok(Err(err)) => errors.push(format!("{}: {err}", target.label)),
            Ok(Ok((socket, _response))) => {
                opened = Some((socket, target.label.clone()));
                break;
            }
        }
    }

    let Some((socket, host)) = opened else {
        return Err(AttemptError::Open(errors.join("; ")));
    };

    let (mut sink, mut stream) = socket.split();

    let mut seq_id = 1u64;
    let enter = encode_enter_room(room_num, "", rand::random::<u64>());
    sink.send(Message::Binary(enter.into()))
        .await
        .map_err(|e| AttemptError::Dropped(format!("enter room failed: {e}")))?;

    *ever_connected = true;
    emit(
        tx,
        ConnectionEvent::Connected {
            room_id: room_id.to_string(),
            room: room.cloned(),
            ws_host: host,
        },
    )
    .await;

    if let Some(initial) = plan.initial.as_ref() {
        if emit_messages(initial, options, seen, tx).await {
            return Ok(SocketEnd::StreamEnd);
        }
    }

    let mut ticker = tokio::time::interval_at(
        Instant::now() + options.heartbeat_interval,
        options.heartbeat_interval,
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                let _ = sink.close().await;
                return Ok(SocketEnd::Cancelled);
            }
            _ = ticker.tick() => {
                seq_id = seq_id.wrapping_add(1);
                let heartbeat = encode_heartbeat(room_num, seq_id);
                if let Err(err) = sink.send(Message::Binary(heartbeat.into())).await {
                    return Err(AttemptError::Dropped(format!("heartbeat failed: {err}")));
                }
            }
            incoming = tokio::time::timeout(options.idle_timeout, stream.next()) => {
                let message = match incoming {
                    Err(_) => {
                        return Err(AttemptError::Dropped(format!(
                            "no data from TikTok for {} seconds",
                            options.idle_timeout.as_secs()
                        )));
                    }
                    Ok(None) => return Err(AttemptError::Dropped("socket closed".into())),
                    Ok(Some(Err(err))) => {
                        return Err(AttemptError::Dropped(format!("socket error: {err}")))
                    }
                    Ok(Some(Ok(message))) => message,
                };

                let data = match message {
                    Message::Binary(data) => data,
                    Message::Close(frame) => {
                        let detail = match frame {
                            Some(frame) => {
                                format!("closed with code {}: {}", frame.code, frame.reason)
                            }
                            None => "closed by TikTok".into(),
                        };
                        return Err(AttemptError::Dropped(detail));
                    }
                    // Ping/Pong are answered by tungstenite; text frames carry no events.
                    _ => continue,
                };

                let decoded = match decode_socket_frame(&data) {
                    Ok(decoded) => decoded,
                    Err(err) => {
                        debug!("skipping undecodable frame: {err}");
                        continue;
                    }
                };
                let Some(result) = decoded.result else {
                    continue;
                };

                if decoded.frame.log_id != 0 && !result.internal_ext.is_empty() {
                    let ack = encode_ack(decoded.frame.log_id, &result.internal_ext);
                    let _ = sink.send(Message::Binary(ack.into())).await;
                }

                if emit_messages(&result, options, seen, tx).await {
                    return Ok(SocketEnd::StreamEnd);
                }
            }
        }
    }
}

async fn emit_messages(
    result: &FetchResult,
    options: &ConnectOptions,
    seen: &mut RecentIds,
    tx: &mpsc::Sender<ConnectionEvent>,
) -> bool {
    for message in &result.messages {
        if message.is_history && !options.include_history {
            continue;
        }
        let msg_id = message.msg_id.to_string();
        if !seen.check_and_insert(&msg_id) {
            continue;
        }
        let Some(event) = decode_message(message) else {
            continue;
        };
        let is_stream_end = matches!(event, LiveEvent::StreamEnd { .. });
        emit(tx, ConnectionEvent::Event(event)).await;
        if is_stream_end {
            return true;
        }
    }
    false
}

async fn emit(tx: &mpsc::Sender<ConnectionEvent>, event: ConnectionEvent) {
    let _ = tx.send(event).await;
}

fn classify(err: &LiveError) -> ErrorKind {
    match err {
        LiveError::UserNotFound(_) => ErrorKind::NotFound,
        LiveError::UserOffline(_) => ErrorKind::Offline,
        LiveError::RateLimited { .. } => ErrorKind::RateLimited,
        LiveError::Protocol(_) => ErrorKind::Protocol,
        _ => ErrorKind::Network,
    }
}

fn build_signed_ws_url(
    push_server: &str,
    room_id: &str,
    initial: &FetchResult,
    user_agent: &str,
    heartbeat: &Duration,
) -> Result<Url, AttemptError> {
    let mut url = Url::parse(push_server)
        .map_err(|e| AttemptError::Open(format!("invalid signed push server: {e}")))?;
    let heartbeat_ms = heartbeat.as_millis().to_string();
    let last_rtt = format!("{:.3}", 100.0 + rand::random::<f64>() * 100.0);
    let browser_version = user_agent.trim_start_matches("Mozilla/").to_string();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("version_code", "180800")
            .append_pair("device_platform", "web")
            .append_pair("cookie_enabled", "true")
            .append_pair("screen_width", "1920")
            .append_pair("screen_height", "1080")
            .append_pair("browser_language", "en-US")
            .append_pair("browser_platform", "MacIntel")
            .append_pair("browser_name", "Mozilla")
            .append_pair("browser_version", &browser_version)
            .append_pair("browser_online", "true")
            .append_pair("tz_name", "UTC")
            .append_pair("app_name", "tiktok_web")
            .append_pair("sup_ws_ds_opt", "1")
            .append_pair("update_version_code", "2.0.0")
            .append_pair("compress", "gzip")
            .append_pair("webcast_language", "en")
            .append_pair("ws_direct", "0")
            .append_pair("aid", "1988")
            .append_pair("live_id", "12")
            .append_pair("app_language", "en")
            .append_pair("client_enter", "1")
            .append_pair("room_id", room_id)
            .append_pair("identity", "audience")
            .append_pair("history_comment_count", "6")
            .append_pair("last_rtt", &last_rtt)
            .append_pair("heartbeat_duration", &heartbeat_ms)
            .append_pair("resp_content_type", "protobuf")
            .append_pair("did_rule", "3")
            .append_pair("internal_ext", &initial.internal_ext)
            .append_pair("cursor", &initial.cursor);
        for param in &initial.route_params {
            if !param.key.is_empty() {
                query.append_pair(&param.key, &param.value);
            }
        }
        query
            .append_pair("User-Agent", user_agent)
            .append_pair("version_code", "270000");
    }
    Ok(url)
}

/// Builds the webcast socket URL, mirroring TikTok's web client query params.
pub fn build_ws_url(host: &str, room_id: &str, heartbeat: &Duration) -> Url {
    let base = if host.starts_with("ws://") || host.starts_with("wss://") {
        host.trim_end_matches('/').to_string()
    } else {
        format!("wss://{host}{WS_PATH}")
    };
    let mut url = Url::parse(&base).expect("valid base url");
    let heartbeat_ms = heartbeat.as_millis().to_string();
    let last_rtt = format!("{:.3}", 100.0 + rand::random::<f64>() * 100.0);
    let browser_version = crate::web::DEFAULT_USER_AGENT
        .trim_start_matches("Mozilla/")
        .to_string();
    url.query_pairs_mut()
        .append_pair("version_code", "180800")
        .append_pair("device_platform", "web")
        .append_pair("cookie_enabled", "true")
        .append_pair("screen_width", "1920")
        .append_pair("screen_height", "1080")
        .append_pair("browser_language", "en-US")
        .append_pair("browser_platform", "MacIntel")
        .append_pair("browser_name", "Mozilla")
        .append_pair("browser_version", &browser_version)
        .append_pair("browser_online", "true")
        .append_pair("tz_name", "UTC")
        .append_pair("app_name", "tiktok_web")
        .append_pair("sup_ws_ds_opt", "1")
        .append_pair("update_version_code", "2.0.0")
        .append_pair("compress", "gzip")
        .append_pair("webcast_language", "en")
        .append_pair("ws_direct", "1")
        .append_pair("aid", "1988")
        .append_pair("live_id", "12")
        .append_pair("app_language", "en")
        .append_pair("client_enter", "1")
        .append_pair("room_id", room_id)
        .append_pair("identity", "audience")
        .append_pair("history_comment_count", "6")
        .append_pair("last_rtt", &last_rtt)
        .append_pair("heartbeat_duration", &heartbeat_ms)
        .append_pair("resp_content_type", "protobuf")
        .append_pair("did_rule", "3");
    url
}

/// Bounded set of recently seen message IDs; TikTok redelivers messages after
/// reconnects.
pub struct RecentIds {
    ids: HashSet<String>,
    order: VecDeque<String>,
    max: usize,
}

impl RecentIds {
    pub fn new(max: usize) -> Self {
        Self {
            ids: HashSet::with_capacity(max),
            order: VecDeque::with_capacity(max),
            max,
        }
    }

    /// Returns `false` when the id was already seen (and records it otherwise).
    pub fn check_and_insert(&mut self, id: &str) -> bool {
        if id.is_empty() || id == "0" {
            return true;
        }
        if self.ids.contains(id) {
            return false;
        }
        self.ids.insert(id.to_string());
        self.order.push_back(id.to_string());
        while self.order.len() > self.max {
            if let Some(oldest) = self.order.pop_front() {
                self.ids.remove(&oldest);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_contains_required_params() {
        let url = build_ws_url("webcast-ws.tiktok.com", "12345", &Duration::from_secs(10));
        assert_eq!(url.path(), "/webcast/im/ws_proxy/ws_reuse_supplement/");
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["room_id"], "12345");
        assert_eq!(params["identity"], "audience");
        assert_eq!(params["resp_content_type"], "protobuf");
        assert_eq!(params["heartbeat_duration"], "10000");
    }

    #[test]
    fn full_ws_host_is_used_verbatim() {
        let url = build_ws_url("wss://example.test/custom", "1", &Duration::from_secs(10));
        assert_eq!(url.host_str(), Some("example.test"));
        assert_eq!(url.path(), "/custom");
    }

    #[test]
    fn signed_url_carries_route_params_and_batch_state() {
        let initial = FetchResult {
            cursor: "cursor-token".into(),
            internal_ext: "internal-token".into(),
            route_params: vec![
                crate::proto::RouteParam {
                    key: "wrss".into(),
                    value: "router-value".into(),
                },
                crate::proto::RouteParam {
                    key: "X-Bogus".into(),
                    value: "bogus-value".into(),
                },
            ],
            ..Default::default()
        };
        let url = build_signed_ws_url(
            "wss://webcast-ws.tiktok.com/webcast/im/ws_proxy/ws_reuse_supplement/",
            "12345",
            &initial,
            "Mozilla/5.0",
            &Duration::from_secs(10),
        )
        .unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["room_id"], "12345");
        assert_eq!(params["cursor"], "cursor-token");
        assert_eq!(params["internal_ext"], "internal-token");
        assert_eq!(params["wrss"], "router-value");
        assert_eq!(params["X-Bogus"], "bogus-value");
        assert_eq!(params["ws_direct"], "0");
    }

    #[test]
    fn recent_ids_dedupes_and_bounds() {
        let mut ids = RecentIds::new(2);
        assert!(ids.check_and_insert("1"));
        assert!(!ids.check_and_insert("1"));
        assert!(ids.check_and_insert("2"));
        assert!(ids.check_and_insert("3"));
        // "1" was evicted, so it is accepted again.
        assert!(ids.check_and_insert("1"));
        // Empty ids are never deduped away.
        assert!(ids.check_and_insert("0"));
    }
}
