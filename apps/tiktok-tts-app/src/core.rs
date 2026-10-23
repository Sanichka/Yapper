//! Application core: connection lifecycle, filtering, queue and synthesis.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use kittentts_rs::language;
use kittentts_rs::queue::QueueSnapshot;
use kittentts_rs::text::{FilterReason, TextPipeline, TtsSettings};
use kittentts_rs::{
    create_engine, wav, wav_bytes, BoundedQueue, EngineStatus, Priority, PushOutcome, QueueItem,
    SynthRequest, TtsEngine, SAMPLE_RATE,
};
use serde::{Deserialize, Serialize};
use tiklive_rs::{
    normalize_username, ConnectOptions, ConnectionEvent, DisconnectReason, LiveClient, LiveEvent,
    LiveSession, RoomStatus, SessionControl,
};
use tokio::sync::{broadcast, watch, Notify};

use crate::config::{self, PersistedConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnStatus {
    Idle,
    Connecting,
    Live,
    Reconnecting,
    Offline,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionState {
    pub status: ConnStatus,
    pub username: String,
    pub room: Option<RoomStatus>,
    pub error: Option<String>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            status: ConnStatus::Idle,
            username: String::new(),
            room: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub received: u64,
    pub spoken: u64,
    pub filtered: u64,
    /// Cumulative number of queue items dropped to keep up with chat.
    pub dropped: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub queue_capacity: usize,
    pub auto_connect: bool,
    pub server_port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigUpdate {
    pub queue_capacity: Option<usize>,
    pub auto_connect: Option<bool>,
    pub server_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateDto {
    pub connection: ConnectionState,
    pub engine: EngineStatus,
    pub edge_engine: EngineStatus,
    pub settings: TtsSettings,
    pub config: AppConfigDto,
    pub queue: QueueSnapshot,
    pub paused: bool,
    pub stats: StatsDto,
    pub server_mode: bool,
    pub engine_note: Option<String>,
    pub data_dir: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerEvent {
    State {
        state: AppStateDto,
    },
    Utterance {
        seq: u64,
        item: QueueItem,
        wav: String,
        duration_ms: u64,
    },
    Activity {
        id: u64,
        kind: String,
        user: String,
        text: String,
        /// `queued`, `filtered`, `dropped` or `seen`.
        status: String,
        reason: Option<String>,
    },
    Log {
        level: String,
        message: String,
    },
}

pub struct Core {
    config_path: PathBuf,
    config: Mutex<PersistedConfig>,
    engine: RwLock<Arc<dyn TtsEngine>>,
    edge_engine: Arc<dyn TtsEngine>,
    pub event_tx: broadcast::Sender<ServerEvent>,
    queue: Mutex<BoundedQueue>,
    pipeline: TextPipeline,
    connection: Mutex<ConnectionState>,
    session: Mutex<Option<SessionControl>>,
    generation: AtomicU64,
    paused: AtomicBool,
    wake: Notify,
    ack_tx: watch::Sender<u64>,
    ack_rx: watch::Receiver<u64>,
    stats_received: AtomicU64,
    stats_spoken: AtomicU64,
    stats_filtered: AtomicU64,
    activity_id: AtomicU64,
    utterance_seq: AtomicU64,
    active_seq: AtomicU64,
    skip_requested: AtomicBool,
    engine_note: Option<String>,
    server_mode: bool,
}

impl Core {
    pub fn new(
        config_path: PathBuf,
        model_override: Option<PathBuf>,
        server_mode: bool,
    ) -> Result<Arc<Self>, String> {
        crate::edge::ensure_default_crypto_provider();
        let persisted = PersistedConfig::load(&config_path);
        let queue_capacity = persisted.queue_capacity.clamp(1, 500);
        let configured_model = persisted.model_dir.clone();
        let model_dir = model_override
            .or_else(|| config::resolve_model_dir(&config_path, configured_model.as_deref()));
        let (engine, note) = create_engine(model_dir.as_deref());
        if let Some(note) = &note {
            tracing::warn!("{note}");
        }
        let (event_tx, _) = broadcast::channel(512);
        let (ack_tx, ack_rx) = watch::channel(0u64);

        Ok(Arc::new(Self {
            config_path,
            config: Mutex::new(persisted),
            engine: RwLock::new(engine),
            edge_engine: Arc::new(crate::edge::EdgeEngine),
            event_tx,
            queue: Mutex::new(BoundedQueue::new(queue_capacity)),
            pipeline: TextPipeline::new(),
            connection: Mutex::new(ConnectionState::default()),
            session: Mutex::new(None),
            generation: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            wake: Notify::new(),
            ack_tx,
            ack_rx,
            stats_received: AtomicU64::new(0),
            stats_spoken: AtomicU64::new(0),
            stats_filtered: AtomicU64::new(0),
            activity_id: AtomicU64::new(0),
            utterance_seq: AtomicU64::new(0),
            active_seq: AtomicU64::new(0),
            skip_requested: AtomicBool::new(false),
            engine_note: note,
            server_mode,
        }))
    }

    pub fn state(&self) -> AppStateDto {
        let settings = self.settings();
        let connection = self
            .connection
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        let queue = self
            .queue
            .lock()
            .map(|q| q.snapshot())
            .unwrap_or(QueueSnapshot {
                active: None,
                items: Vec::new(),
                dropped: 0,
                capacity: kittentts_rs::MAX_QUEUE_SIZE,
            });
        let engine = self
            .engine
            .read()
            .map(|e| e.status())
            .unwrap_or(EngineStatus {
                kind: kittentts_rs::EngineKind::Mock,
                model: "unavailable".into(),
                sample_rate: SAMPLE_RATE,
                voices: Vec::new(),
                threads: 0,
            });
        AppStateDto {
            connection,
            engine,
            edge_engine: self.edge_engine.status(),
            settings,
            config: self.app_config(),
            paused: self.paused.load(Ordering::Relaxed),
            stats: StatsDto {
                received: self.stats_received.load(Ordering::Relaxed),
                spoken: self.stats_spoken.load(Ordering::Relaxed),
                filtered: self.stats_filtered.load(Ordering::Relaxed),
                dropped: queue.dropped,
            },
            server_mode: self.server_mode,
            engine_note: self.engine_note.clone(),
            data_dir: self
                .config_path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            queue,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.event_tx.subscribe()
    }

    pub fn emit_state(&self) {
        let _ = self.event_tx.send(ServerEvent::State {
            state: self.state(),
        });
    }

    pub fn log(&self, level: &str, message: impl Into<String>) {
        let _ = self.event_tx.send(ServerEvent::Log {
            level: level.into(),
            message: message.into(),
        });
    }

    fn emit_activity(
        &self,
        kind: &str,
        user: &str,
        text: &str,
        status: &str,
        reason: Option<String>,
    ) {
        let id = self.activity_id.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.event_tx.send(ServerEvent::Activity {
            id,
            kind: kind.into(),
            user: user.into(),
            text: text.into(),
            status: status.into(),
            reason,
        });
    }

    pub fn settings(&self) -> TtsSettings {
        self.config
            .lock()
            .map(|c| c.settings.clone())
            .unwrap_or_default()
    }

    pub fn app_config(&self) -> AppConfigDto {
        self.config
            .lock()
            .map(|c| AppConfigDto {
                queue_capacity: c.queue_capacity,
                auto_connect: c.auto_connect,
                server_port: c.server_port,
            })
            .unwrap_or(AppConfigDto {
                queue_capacity: kittentts_rs::MAX_QUEUE_SIZE,
                auto_connect: false,
                server_port: config::DEFAULT_PORT,
            })
    }

    pub fn username(&self) -> String {
        self.config
            .lock()
            .map(|c| c.username.clone())
            .unwrap_or_default()
    }

    pub fn auto_connect(&self) -> bool {
        self.config.lock().map(|c| c.auto_connect).unwrap_or(false)
    }

    pub fn server_port(&self) -> u16 {
        self.config
            .lock()
            .map(|c| c.server_port)
            .unwrap_or(config::DEFAULT_PORT)
    }

    pub fn update_settings(&self, settings: TtsSettings) -> Result<(), String> {
        let mut next = settings;
        next.language = language::normalize_language(&next.language).to_string();
        next.voice = language::voice_for_language(&next.language, &next.voice);
        if next.voice.is_empty() {
            next.voice = "Bella".into();
        }
        next.speed = next.speed.clamp(0.5, 2.0);
        next.max_len = next.max_len.clamp(10, 500);
        next.cooldown_secs = next.cooldown_secs.min(60);
        next.min_gift_diamonds = next.min_gift_diamonds.min(1_000_000);
        next.template = truncate(&next.template, 300);
        next.gift_template = truncate(&next.gift_template, 300);
        if next.gift_template.trim().is_empty() {
            next.gift_template = TtsSettings::default().gift_template;
        }
        next.blacklist = next
            .blacklist
            .into_iter()
            .map(|w| truncate(w.trim(), 60))
            .filter(|w| !w.is_empty())
            .take(200)
            .collect();

        {
            let mut config = self
                .config
                .lock()
                .map_err(|_| "config lock was poisoned".to_string())?;
            config.settings = next;
        }
        self.pipeline.reset_cooldowns();
        self.save_config();
        self.emit_state();
        Ok(())
    }

    pub fn update_app_config(&self, update: AppConfigUpdate) -> Result<(), String> {
        if let Some(capacity) = update.queue_capacity {
            if !(1..=500).contains(&capacity) {
                return Err("Queue capacity must be between 1 and 500".into());
            }
        }
        if update.server_port == Some(0) {
            return Err("Server port must be between 1 and 65535".into());
        }

        {
            let mut config = self
                .config
                .lock()
                .map_err(|_| "config lock was poisoned".to_string())?;
            if let Some(capacity) = update.queue_capacity {
                config.queue_capacity = capacity;
            }
            if let Some(auto_connect) = update.auto_connect {
                config.auto_connect = auto_connect;
            }
            if let Some(port) = update.server_port {
                config.server_port = port;
            }
        }

        if let Some(capacity) = update.queue_capacity {
            if let Ok(mut queue) = self.queue.lock() {
                queue.set_capacity(capacity);
            }
        }
        self.save_config();
        self.emit_state();
        Ok(())
    }

    fn save_config(&self) {
        let snapshot = self.config.lock().map(|c| c.clone());
        if let Ok(config) = snapshot {
            if let Err(error) = config.save(&self.config_path) {
                self.log("error", format!("could not save settings: {error}"));
            }
        }
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    fn set_connection(&self, generation: u64, next: ConnectionState) {
        if !self.is_current(generation) {
            return;
        }
        if let Ok(mut state) = self.connection.lock() {
            *state = next;
        }
        self.emit_state();
    }

    pub async fn connect(self: &Arc<Self>, input: &str) -> Result<(), String> {
        let username = normalize_username(input);
        if username.len() < 2 {
            return Err("Enter a TikTok username (without the @)".into());
        }

        self.disconnect();

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.set_connection(
            generation,
            ConnectionState {
                status: ConnStatus::Connecting,
                username: username.clone(),
                room: None,
                error: None,
            },
        );
        {
            let mut config = self
                .config
                .lock()
                .map_err(|_| "config lock was poisoned".to_string())?;
            config.username = username.clone();
        }
        self.save_config();

        let client = LiveClient::new(None)
            .map_err(|error| format!("could not create the TikTok client: {error}"))?;
        let session = client.connect(&username, ConnectOptions::default());
        let control = session.control();
        if let Ok(mut slot) = self.session.lock() {
            *slot = Some(control);
        }

        let core = self.clone();
        tokio::spawn(async move {
            core.pump(generation, username, session).await;
        });
        Ok(())
    }

    pub fn disconnect(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(control) = self.session.lock().ok().and_then(|mut s| s.take()) {
            control.disconnect();
        }
        if let Ok(mut state) = self.connection.lock() {
            let username = state.username.clone();
            *state = ConnectionState {
                status: ConnStatus::Idle,
                username,
                room: None,
                error: None,
            };
        }
        self.emit_state();
    }

    async fn pump(self: Arc<Self>, generation: u64, username: String, mut session: LiveSession) {
        let mut last_error: Option<String> = None;
        while let Some(event) = session.recv().await {
            if !self.is_current(generation) {
                return;
            }
            match event {
                ConnectionEvent::Connected { room_id, room, .. } => {
                    let mut room = room;
                    if room.is_none() {
                        room = Some(RoomStatus {
                            room_id,
                            is_live: true,
                            ..Default::default()
                        });
                    }
                    self.set_connection(
                        generation,
                        ConnectionState {
                            status: ConnStatus::Live,
                            username: username.clone(),
                            room,
                            error: None,
                        },
                    );
                    self.log("info", format!("connected to @{username}'s LIVE"));
                }
                ConnectionEvent::Event(event) => match event {
                    LiveEvent::StreamEnd { reason, .. } => {
                        self.set_connection(
                            generation,
                            ConnectionState {
                                status: ConnStatus::Offline,
                                username: username.clone(),
                                room: None,
                                error: Some(if reason.is_empty() {
                                    "the LIVE ended".into()
                                } else {
                                    format!("LIVE ended: {reason}")
                                }),
                            },
                        );
                        break;
                    }
                    other => self.handle_live_event(other),
                },
                ConnectionEvent::Disconnected {
                    reason,
                    will_reconnect,
                    detail,
                } => {
                    if will_reconnect {
                        last_error = detail.clone();
                        self.set_connection(
                            generation,
                            ConnectionState {
                                status: ConnStatus::Reconnecting,
                                username: username.clone(),
                                room: None,
                                error: detail,
                            },
                        );
                    } else {
                        let message = detail.unwrap_or_else(|| match reason {
                            DisconnectReason::StreamEnd => "the LIVE ended".into(),
                            DisconnectReason::Offline => "the streamer is offline".into(),
                            DisconnectReason::Network => "connection lost".into(),
                        });
                        let status = if reason == DisconnectReason::Offline {
                            ConnStatus::Offline
                        } else {
                            ConnStatus::Error
                        };
                        last_error = Some(message.clone());
                        self.set_connection(
                            generation,
                            ConnectionState {
                                status,
                                username: username.clone(),
                                room: None,
                                error: Some(message),
                            },
                        );
                    }
                }
                ConnectionEvent::Error { message, .. } => {
                    last_error = Some(message.clone());
                    self.set_connection(
                        generation,
                        ConnectionState {
                            status: ConnStatus::Error,
                            username: username.clone(),
                            room: None,
                            error: Some(message),
                        },
                    );
                    break;
                }
            }
        }

        if !self.is_current(generation) {
            return;
        }
        if let Ok(mut slot) = self.session.lock() {
            *slot = None;
        }
        if let Ok(mut state) = self.connection.lock() {
            if matches!(
                state.status,
                ConnStatus::Live | ConnStatus::Reconnecting | ConnStatus::Connecting
            ) {
                state.status = if last_error.is_some() {
                    ConnStatus::Error
                } else {
                    ConnStatus::Idle
                };
                state.room = None;
                state.error = last_error;
            }
        }
        self.emit_state();
    }

    fn handle_live_event(&self, event: LiveEvent) {
        let settings = self.settings();
        match event {
            LiveEvent::Chat { user, comment, .. } => {
                self.stats_received.fetch_add(1, Ordering::Relaxed);
                let display = user.display_name().to_string();
                match self
                    .pipeline
                    .prepare_chat(&settings, &display, &user.unique_id, &comment)
                {
                    Ok(prepared) => {
                        self.push_item(
                            "chat",
                            Priority::Normal,
                            display.clone(),
                            user.unique_id.clone(),
                            prepared.spoken,
                            prepared.raw,
                            &settings,
                            &prepared.language,
                        );
                    }
                    Err(reason) => {
                        self.stats_filtered.fetch_add(1, Ordering::Relaxed);
                        self.emit_activity(
                            "chat",
                            &display,
                            &comment,
                            "filtered",
                            Some(filter_message(&reason)),
                        );
                    }
                }
            }
            LiveEvent::Gift {
                user,
                gift_name,
                diamond_count,
                repeat_count,
                streaking,
                total_diamonds,
                ..
            } => {
                if streaking {
                    return;
                }
                self.stats_received.fetch_add(1, Ordering::Relaxed);
                let display = user.display_name().to_string();
                let count = repeat_count.max(1);
                let diamonds = total_diamonds.max(diamond_count.saturating_mul(count));
                match self.pipeline.prepare_gift(
                    &settings,
                    &display,
                    &user.unique_id,
                    &gift_name,
                    count,
                    diamonds,
                ) {
                    Ok(prepared) => {
                        self.push_item(
                            "gift",
                            Priority::High,
                            display.clone(),
                            user.unique_id.clone(),
                            prepared.spoken,
                            prepared.raw,
                            &settings,
                            &prepared.language,
                        );
                    }
                    Err(reason) => {
                        if settings.read_gifts {
                            self.stats_filtered.fetch_add(1, Ordering::Relaxed);
                        }
                        self.emit_activity(
                            "gift",
                            &display,
                            &format!("{gift_name} x{count}"),
                            "filtered",
                            Some(filter_message(&reason)),
                        );
                    }
                }
            }
            LiveEvent::Follow { user, .. } => {
                self.emit_activity("follow", user.display_name(), "followed", "seen", None);
            }
            LiveEvent::Share { user, .. } => {
                self.emit_activity(
                    "share",
                    user.display_name(),
                    "shared the LIVE",
                    "seen",
                    None,
                );
            }
            LiveEvent::Subscribe {
                user, sub_month, ..
            } => {
                self.emit_activity(
                    "subscribe",
                    user.display_name(),
                    &if sub_month > 1 {
                        format!("resubscribed for {sub_month} months")
                    } else {
                        "subscribed".to_string()
                    },
                    "seen",
                    None,
                );
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_item(
        &self,
        kind: &str,
        priority: Priority,
        user: String,
        unique_id: String,
        text: String,
        raw: String,
        settings: &TtsSettings,
        language: &str,
    ) {
        let voice = language::voice_for_language(language, &settings.voice);
        let item = {
            let mut queue = match self.queue.lock() {
                Ok(queue) => queue,
                Err(_) => return,
            };
            QueueItem {
                id: queue.next_id(),
                user: user.clone(),
                unique_id,
                text,
                raw: raw.clone(),
                language: language.into(),
                priority,
                kind: kind.into(),
                voice,
                speed: settings.speed,
                created_at: now_ms(),
            }
        };
        let id = item.id;
        let outcome = self
            .queue
            .lock()
            .map(|mut queue| queue.enqueue(item))
            .unwrap_or(PushOutcome::Rejected(QueueItem {
                id,
                user: user.clone(),
                unique_id: String::new(),
                text: String::new(),
                raw: raw.clone(),
                language: language.into(),
                priority,
                kind: kind.into(),
                voice: language::voice_for_language(language, &settings.voice),
                speed: settings.speed,
                created_at: now_ms(),
            }));

        let (status, reason) = match outcome {
            PushOutcome::Enqueued { .. } => ("queued", None),
            PushOutcome::Evicted(evicted) => (
                "queued",
                Some(format!("dropped older message from {}", evicted.user)),
            ),
            PushOutcome::Rejected(_) => ("dropped", Some("queue is full".into())),
        };
        self.emit_activity(kind, &user, &raw, status, reason);
        self.emit_state();
        self.wake.notify_one();
    }

    pub fn ack(&self, seq: u64) {
        if seq == 0 {
            return;
        }
        let current = *self.ack_tx.borrow();
        if seq > current {
            self.ack_tx.send_replace(seq);
        }
    }

    pub fn skip(&self) {
        let has_active = self
            .queue
            .lock()
            .map(|queue| queue.active().is_some())
            .unwrap_or(false);
        if !has_active {
            return;
        }
        let active = self.active_seq.load(Ordering::SeqCst);
        if active > 0 {
            // Playing: acknowledge the utterance so the loop moves on.
            self.ack(active);
        } else {
            // Still synthesizing: drop the audio when it is ready.
            self.skip_requested.store(true, Ordering::SeqCst);
        }
        self.emit_state();
    }

    pub fn clear_queue(&self) -> usize {
        let removed = self
            .queue
            .lock()
            .map(|mut queue| queue.clear_pending())
            .unwrap_or(0);
        if removed > 0 {
            self.log("info", format!("cleared {removed} queued messages"));
        }
        self.emit_state();
        removed
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        if !paused {
            self.wake.notify_one();
        }
        self.emit_state();
    }

    pub fn speak(self: &Arc<Self>, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Nothing to speak".into());
        }
        let settings = self.settings();
        let language = language::resolve_language(&settings.language, text);
        self.push_item(
            "test",
            Priority::High,
            "Test".into(),
            "test".into(),
            text.to_string(),
            text.to_string(),
            &settings,
            language,
        );
        Ok(())
    }

    pub async fn run_synth_loop(self: Arc<Self>) {
        loop {
            if self.paused.load(Ordering::SeqCst) {
                self.wake.notified().await;
                continue;
            }

            let next = self
                .queue
                .lock()
                .ok()
                .and_then(|mut queue| queue.take_next());
            let Some(item) = next else {
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
                continue;
            };
            self.emit_state();

            let engine = if item.language == language::UKRAINIAN {
                self.edge_engine.clone()
            } else {
                self.engine
                    .read()
                    .map(|engine| engine.clone())
                    .unwrap_or_else(|_| Arc::new(kittentts_rs::MockEngine::new()))
            };
            let sample_rate = engine.sample_rate();
            let request = SynthRequest {
                text: item.text.clone(),
                voice: item.voice.clone(),
                speed: item.speed,
            };
            let label = format!("{}: {}", item.user, item.raw);
            let result = tokio::task::spawn_blocking(move || engine.synthesize(&request)).await;
            let skip = self.skip_requested.swap(false, Ordering::SeqCst);

            match result {
                Ok(Ok(samples)) if !skip => {
                    let duration_ms = wav::duration_ms(samples.len(), sample_rate);
                    let bytes = wav_bytes(&samples, sample_rate);
                    let audio = base64::engine::general_purpose::STANDARD.encode(bytes);
                    let seq = self.utterance_seq.fetch_add(1, Ordering::SeqCst) + 1;
                    self.active_seq.store(seq, Ordering::SeqCst);
                    self.stats_spoken.fetch_add(1, Ordering::Relaxed);
                    let _ = self.event_tx.send(ServerEvent::Utterance {
                        seq,
                        item: item.clone(),
                        wav: audio,
                        duration_ms,
                    });
                    self.wait_for_ack(seq, duration_ms).await;
                    self.active_seq.store(0, Ordering::SeqCst);
                    self.skip_requested.store(false, Ordering::SeqCst);
                }
                Ok(Ok(_)) => {
                    self.log("info", format!("skipped {label}"));
                }
                Ok(Err(error)) => {
                    self.stats_filtered.fetch_add(1, Ordering::Relaxed);
                    self.log("error", format!("TTS failed for {label}: {error}"));
                }
                Err(error) => {
                    self.log("error", format!("TTS worker failed: {error}"));
                }
            }

            if let Ok(mut queue) = self.queue.lock() {
                queue.finish_active();
            }
            self.emit_state();
        }
    }

    async fn wait_for_ack(&self, seq: u64, duration_ms: u64) {
        let mut rx = self.ack_rx.clone();
        let wait = async {
            while *rx.borrow() < seq {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        };
        let timeout = tokio::time::sleep(Duration::from_millis(duration_ms.saturating_add(5_000)));
        tokio::select! {
            _ = wait => {}
            _ = timeout => {}
        }
    }
}

fn filter_message(reason: &FilterReason) -> String {
    reason.message()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("yapper-core-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.join("config.json"), dir)
    }

    #[test]
    fn updates_app_config_and_queue_capacity() {
        let (path, dir) = temp_config("update");
        let core = Core::new(path.clone(), Some(dir.join("missing-model")), false).unwrap();

        core.update_app_config(AppConfigUpdate {
            queue_capacity: Some(7),
            auto_connect: Some(true),
            server_port: Some(9099),
        })
        .unwrap();

        let state = core.state();
        assert_eq!(state.config.queue_capacity, 7);
        assert!(state.config.auto_connect);
        assert_eq!(state.config.server_port, 9099);
        assert_eq!(state.queue.capacity, 7);

        let persisted = PersistedConfig::load(&path);
        assert_eq!(persisted.queue_capacity, 7);
        assert!(persisted.auto_connect);
        assert_eq!(persisted.server_port, 9099);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_invalid_app_config() {
        let (path, dir) = temp_config("invalid");
        let core = Core::new(path, Some(dir.join("missing-model")), false).unwrap();

        assert!(core
            .update_app_config(AppConfigUpdate {
                queue_capacity: Some(0),
                auto_connect: None,
                server_port: None,
            })
            .is_err());
        assert!(core
            .update_app_config(AppConfigUpdate {
                queue_capacity: Some(501),
                auto_connect: None,
                server_port: None,
            })
            .is_err());
        assert!(core
            .update_app_config(AppConfigUpdate {
                queue_capacity: None,
                auto_connect: None,
                server_port: Some(0),
            })
            .is_err());

        let _ = std::fs::remove_dir_all(dir);
    }
}
