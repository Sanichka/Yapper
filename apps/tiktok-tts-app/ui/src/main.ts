import "./style.css";
import { Player } from "./player";
import { Transport } from "./transport";
import type {
  ActivityEvent,
  AppConfig,
  AppStateDto,
  ConnStatus,
  ModelDto,
  ModelProgressEvent,
  ServerEvent,
  TtsSettings,
} from "./types";

const FALLBACK_VOICES = [
  "Bella",
  "Jasper",
  "Luna",
  "Bruno",
  "Rosie",
  "Hugo",
  "Kiki",
  "Leo",
];

const UKRAINIAN_VOICES = ["uk-UA-PolinaNeural", "uk-UA-OstapNeural"];
const LANGUAGE_AUTO = "auto";
const LANGUAGE_EN = "en";
const LANGUAGE_UK = "uk";

interface FeedEntry extends ActivityEvent {
  at: number;
}

const transport = new Transport();
const player = new Player();

let state: AppStateDto | null = null;
let feed: FeedEntry[] = [];
let feedFilter: "all" | "chat" | "gift" = "all";
let settingsDirty = false;
let settingsRevision = 0;
let saveTimer: number | undefined;
let saving = false;
let appConfigDirty = false;
let appConfigRevision = 0;
let appConfigTimer: number | undefined;
let savingAppConfig = false;
let playing: { id: number; durationMs: number } | null = null;
let currentSeq: number | null = null;
let lastStatus: ConnStatus | null = null;
let toastTimer: number | undefined;
const downloadingModels: Record<string, ModelProgressEvent> = {};
let modelsExpanded = false;
let switchingModelId: string | null = null;
let deletingModelId: string | null = null;

const root = document.querySelector<HTMLDivElement>("#app");
if (!root) throw new Error("missing #app");
root.innerHTML = shell();

const $ = <T extends HTMLElement>(selector: string): T =>
  document.querySelector(selector) as T;

bind();
void boot();

async function boot(): Promise<void> {
  try {
    await transport.start(onEvent);
  } catch (error) {
    toast(`Could not subscribe to events: ${message(error)}`, "error");
  }
  try {
    const initial = await transport.fetchState();
    applyState(initial);
    if (initial.nativeAudio) {
      void transport
        .call("request_notification_permission")
        .catch(() => undefined);
    }
  } catch (error) {
    toast(`Could not reach the Yapper backend: ${message(error)}`, "error");
  }
}

function onEvent(event: ServerEvent): void {
  switch (event.type) {
    case "state":
      applyState(event.state);
      break;
    case "utterance":
      if (state?.nativeAudio) {
        // Handled natively by Android Foreground Service / AudioTrack
        currentSeq = event.seq;
        playing = { id: event.item.id, durationMs: event.durationMs };
        renderNow();
      } else {
        void playUtterance(event.seq, event.wav, event.item.id, event.durationMs);
      }
      break;
    case "activity":
      feed.unshift({ ...event, at: Date.now() });
      if (feed.length > 150) feed.length = 150;
      if (matchesFilter(event)) renderActivity();
      break;
    case "log":
      if (event.level === "error") {
        toast(event.message, "error");
      } else {
        console.info("[yapper]", event.message);
      }
      break;
    case "modelProgress":
      downloadingModels[event.modelId] = { ...event };
      if (event.status === "complete") {
        toast(`Model ${event.modelId} ready!`);
        delete downloadingModels[event.modelId];
      } else if (event.status === "error") {
        toast(`Download failed: ${event.message || "Unknown error"}`, "error");
        delete downloadingModels[event.modelId];
      }
      renderModels();
      break;
  }
}

function applyState(next: AppStateDto): void {
  const previous = state;
  state = next;

  if (next.nativeAudio && !next.queue.active && playing) {
    playing = null;
    currentSeq = null;
  }

  if (
    previous &&
    previous.connection.status !== next.connection.status &&
    lastStatus !== next.connection.status
  ) {
    lastStatus = next.connection.status;
    if (next.connection.status === "live") {
      toast(`Connected to @${next.connection.username}`);
    } else if (next.connection.status === "waiting") {
      toast(
        next.connection.error ??
          `Waiting for @${next.connection.username} to go live…`,
      );
    } else if (next.connection.status === "error") {
      toast(next.connection.error ?? "Connection failed", "error");
    } else if (next.connection.status === "offline") {
      toast(next.connection.error ?? "The LIVE ended");
    }
  }
  render();
}

async function playUtterance(
  seq: number,
  wav: string,
  itemId: number,
  durationMs: number,
): Promise<void> {
  if (currentSeq !== null) {
    void transport.call("ack", { seq: currentSeq }).catch(() => undefined);
  }
  currentSeq = seq;
  playing = { id: itemId, durationMs };
  renderNow();

  try {
    await player.play(wav, () => {
      if (currentSeq === seq) {
        currentSeq = null;
        playing = null;
      }
      void transport.call("ack", { seq }).catch(() => undefined);
      renderNow();
    });
    if (player.suspended) showAudioBanner();
  } catch (error) {
    toast(`Audio playback failed: ${message(error)}`, "error");
    if (currentSeq === seq) {
      currentSeq = null;
      playing = null;
    }
    void transport.call("ack", { seq }).catch(() => undefined);
    renderNow();
  }
}

function render(): void {
  if (!state) return;
  renderStatus();
  renderConnection();
  renderVoice();
  renderSettings(false);
  renderAppSettings(false);
  renderNow();
  renderQueue();
  renderStats();
  renderActivity();
}

function renderStatus(): void {
  if (!state) return;
  const pill = $("#status-pill");
  const labels: Record<ConnStatus, string> = {
    idle: "Not connected",
    connecting: "Connecting…",
    waiting: "Waiting for live…",
    live: "LIVE",
    reconnecting: "Reconnecting…",
    offline: "Stream offline",
    error: "Connection error",
  };
  const status = state.connection.status;
  pill.className = `status-pill status-${status}`;
  pill.innerHTML = `<i></i><span>${labels[status]}</span>`;
  $("#server-badge").classList.toggle("hidden", !state.serverMode);
}

function renderConnection(): void {
  if (!state) return;
  const { connection } = state;
  const input = $<HTMLInputElement>("#username");
  const button = $<HTMLButtonElement>("#connect-btn");
  const busy = connection.status === "connecting";
  const connected =
    connection.status === "live" ||
    connection.status === "waiting" ||
    connection.status === "reconnecting" ||
    connection.status === "offline" ||
    connection.status === "error";

  if (document.activeElement !== input) {
    if (!input.value && connection.username) input.value = connection.username;
  }
  input.disabled =
    busy || connection.status === "live" || connection.status === "waiting";
  button.disabled = busy;
  button.textContent = busy ? "Cancel" : connected ? "Disconnect" : "Connect";
  button.classList.toggle("btn-primary", !connected);
  button.classList.toggle("btn-danger", connected);
  button.classList.toggle("btn-busy", busy);

  const meta = $("#connect-meta");
  if (connection.room) {
    const room = connection.room;
    const viewers = formatNumber(room.viewerCount);
    const owner = room.owner?.nickname || room.owner?.uniqueId || "";
    meta.innerHTML = `
      <div class="room">
        <div class="room-top">
          <span class="room-dot"></span>
          <span class="room-owner">${esc(owner)}</span>
          <span class="room-viewers">${viewers} watching</span>
        </div>
        <div class="room-title">${esc(room.title || "Untitled LIVE")}</div>
      </div>`;
  } else if (connection.status === "waiting") {
    meta.innerHTML = `
      <div class="waiting-status">
        <span class="waiting-dot"></span>
        <span>${esc(
          connection.error ??
            `Waiting for @${connection.username} to go live…`,
        )} <b>· checking every 5s</b></span>
      </div>`;
  } else if (connection.status === "error" || connection.status === "reconnecting") {
    meta.innerHTML = `<p class="connect-error">${esc(
      connection.error ?? "Connection lost",
    )}</p>`;
  } else if (connection.status === "offline") {
    meta.innerHTML = `<p class="connect-error">${esc(
      connection.error ?? "The streamer is offline",
    )}</p>`;
  } else if (connection.status === "connecting") {
    meta.innerHTML = `<p class="hint">Looking up @${esc(
      connection.username,
    )} on TikTok…</p>`;
  } else if (connection.status === "live") {
    meta.innerHTML = `<p class="hint">Listening to @${esc(
      connection.username,
    )}</p>`;
  } else {
    meta.innerHTML = `<p class="hint">No login needed. Enter the streamer’s handle to start reading chat.</p>`;
  }
}

function renderVoice(): void {
  if (!state) return;
  const language =
    settingsDirty
      ? ($<HTMLSelectElement>("#language").value as string)
      : state.settings.language;

  const engine = language === LANGUAGE_UK ? state.edgeEngine : state.engine;
  const badge = $("#engine-badge");
  const label = engine.kind === "onnx" ? "ONNX" : engine.kind === "edge" ? "EDGE" : "MOCK";
  const badgeClass =
    engine.kind === "onnx"
      ? "badge-mint"
      : engine.kind === "edge"
        ? "badge-blue"
        : "badge-amber";
  badge.textContent = label;
  badge.className = `badge ${badgeClass}`;
  badge.title = engine.model;

  $("#engine-line").textContent =
    engine.kind === "onnx"
      ? `${engine.model} · ${engine.sampleRate / 1000} kHz · ${engine.threads} threads`
      : engine.kind === "edge"
        ? `${engine.model} · ${engine.sampleRate / 1000} kHz`
        : "Tone test engine — model not loaded";

  const note = $("#engine-note");
  if (state.engineNote && language !== LANGUAGE_UK) {
    note.textContent = state.engineNote;
    note.classList.remove("hidden");
  } else {
    note.classList.add("hidden");
  }

  const langNote = $("#language-note");
  if (language === LANGUAGE_UK) {
    langNote.textContent =
      "Ukrainian is synthesized by Microsoft Edge online — an internet connection is required.";
    langNote.classList.remove("hidden");
  } else if (language === LANGUAGE_AUTO) {
    langNote.textContent =
      "English uses the local Kitten voice; Cyrillic messages are spoken by a Ukrainian Edge voice online.";
    langNote.classList.remove("hidden");
  } else {
    langNote.classList.add("hidden");
  }

  const select = $<HTMLSelectElement>("#voice");
  const voices =
    language === LANGUAGE_UK
      ? UKRAINIAN_VOICES.slice()
      : (state.engine.voices.length
          ? state.engine.voices
          : FALLBACK_VOICES
        ).slice();
  const signature = `${language}|${voices.join("|")}`;
  if (select.dataset.signature !== signature) {
    select.innerHTML = voices
      .map(
        (voice) =>
          `<option value="${esc(voice)}">${esc(displayVoice(voice))}</option>`,
      )
      .join("");
    select.dataset.signature = signature;
  }
  if (!voices.includes(select.value)) {
    select.value = voices[0];
  }
  if (!settingsDirty && document.activeElement !== select) {
    select.value = state.settings.voice;
  }

  const speed = $<HTMLInputElement>("#speed");
  if (document.activeElement !== speed && !settingsDirty) {
    speed.value = state.settings.speed.toFixed(2);
  }
  $("#speed-value").textContent = `${Number(
    settingsDirty ? speed.value : state.settings.speed,
  ).toFixed(2)}×`;

  const volume = $<HTMLInputElement>("#volume");
  const savedVolume = Number.isFinite(state.settings.volume)
    ? clamp(state.settings.volume, 0, 1)
    : 1;
  if (document.activeElement !== volume && !settingsDirty) {
    volume.value = savedVolume.toFixed(2);
  }
  const activeVolume = clamp(
    Number(settingsDirty ? volume.value : savedVolume),
    0,
    1,
  );
  $("#volume-value").textContent = `${Math.round(activeVolume * 100)}%`;
  player.setVolume(activeVolume);
  renderModels();
}

function renderModels(): void {
  if (!state) return;
  const list = $("#models-list");
  const section = $("#models-section");
  const summaryBadge = $("#models-summary-badge");
  const warningBanner = $("#models-warning-banner");
  const toggleBtn = $<HTMLButtonElement>("#models-toggle-btn");
  const toggleText = $("#models-toggle-text");

  const language =
    settingsDirty
      ? ($<HTMLSelectElement>("#language").value as string)
      : state.settings.language;

  const isLocalSupported = language !== LANGUAGE_UK;
  section.classList.toggle("hidden", !isLocalSupported);
  if (!isLocalSupported) return;

  const models = state.models || [];
  const installedCount = models.filter((m) => m.installed).length;
  const isMock = state.engine.kind === "mock";
  const activeModel = models.find((m) => m.active);

  if (isMock && !modelsExpanded && toggleBtn.dataset.manuallyToggled !== "true") {
    modelsExpanded = true;
  }

  summaryBadge.textContent = activeModel
    ? activeModel.name
    : `${installedCount}/${models.length} installed`;
  summaryBadge.className = `badge ${activeModel ? "badge-mint" : isMock ? "badge-amber" : "badge-muted"}`;

  warningBanner.classList.toggle("hidden", !isMock);
  list.classList.toggle("hidden", !modelsExpanded);
  toggleBtn.classList.toggle("active", modelsExpanded);
  toggleText.textContent = modelsExpanded ? "Hide" : "Manage";

  if (!modelsExpanded) return;

  list.innerHTML = models
    .map((m) => {
      const dl = downloadingModels[m.id];
      const isDownloading = !!dl;
      const isSwitching = switchingModelId === m.id;
      const isDeleting = deletingModelId === m.id;

      let statusBadge = "";
      if (m.active) {
        statusBadge = `<span class="badge badge-mint">Active</span>`;
      } else if (m.installed) {
        const sizeStr = m.diskSizeBytes
          ? `${(m.diskSizeBytes / (1024 * 1024)).toFixed(0)} MB`
          : `${m.approxSizeMb} MB`;
        statusBadge = `<span class="badge badge-muted">Installed · ${sizeStr}</span>`;
      } else {
        statusBadge = `<span class="badge badge-muted">${m.approxSizeMb} MB</span>`;
      }

      let actionsHtml = "";
      if (isDownloading) {
        actionsHtml = `
          <div class="model-progress">
            <div class="model-progress-header">
              <span class="model-progress-file">${esc(dl?.currentFile || "Downloading…")}</span>
              <span class="model-progress-percent">${(dl?.percent ?? 0).toFixed(0)}%</span>
            </div>
            <div class="progress-bar-track">
              <div class="progress-bar-fill" style="width: ${dl?.percent ?? 0}%"></div>
            </div>
            <span class="model-progress-msg">${esc(dl?.message || "Downloading model files…")}</span>
          </div>`;
      } else if (m.active) {
        actionsHtml = `<span class="model-status-text">Active voice engine</span>`;
      } else if (m.installed) {
        actionsHtml = `
          <div class="model-actions">
            <button class="btn btn-sm btn-primary model-activate-btn" data-id="${esc(m.id)}" ${isSwitching ? "disabled" : ""}>
              ${isSwitching ? "Switching…" : "Use"}
            </button>
            <button class="btn btn-sm btn-ghost btn-danger-ghost model-delete-btn" data-id="${esc(m.id)}" ${isDeleting ? "disabled" : ""}>
              ${isDeleting ? "…" : "Delete"}
            </button>
          </div>`;
      } else {
        actionsHtml = `
          <div class="model-actions">
            <button class="btn btn-sm btn-primary model-download-btn" data-id="${esc(m.id)}">
              Download (${m.approxSizeMb} MB)
            </button>
          </div>`;
      }

      return `
        <div class="model-card ${m.active ? "model-card-active" : ""}">
          <div class="model-card-head">
            <div class="model-title-row">
              <span class="model-name">${esc(m.name)}</span>
              ${statusBadge}
            </div>
            <p class="model-desc">${esc(m.description)}</p>
          </div>
          <div class="model-card-body">
            ${actionsHtml}
          </div>
        </div>`;
    })
    .join("");

  list.querySelectorAll<HTMLButtonElement>(".model-download-btn").forEach((btn) => {
    btn.onclick = (e) => {
      e.stopPropagation();
      const id = btn.dataset.id;
      if (id) void downloadModel(id);
    };
  });

  list.querySelectorAll<HTMLButtonElement>(".model-activate-btn").forEach((btn) => {
    btn.onclick = (e) => {
      e.stopPropagation();
      const id = btn.dataset.id;
      if (id) void switchModel(id);
    };
  });

  list.querySelectorAll<HTMLButtonElement>(".model-delete-btn").forEach((btn) => {
    btn.onclick = (e) => {
      e.stopPropagation();
      const id = btn.dataset.id;
      if (id && confirm(`Delete model "${id}" from this device?`)) {
        void deleteModel(id);
      }
    };
  });
}

async function downloadModel(modelId: string): Promise<void> {
  downloadingModels[modelId] = {
    modelId,
    status: "downloading",
    percent: 0,
    downloadedBytes: 0,
    totalBytes: 0,
    currentFile: "Connecting…",
    message: "Starting download…",
  };
  renderModels();
  try {
    toast(`Downloading ${modelId}…`);
    await transport.call("download_model", { modelId });
  } catch (err) {
    delete downloadingModels[modelId];
    renderModels();
    toast(`Failed to download ${modelId}: ${message(err)}`, "error");
  }
}

async function switchModel(modelId: string): Promise<void> {
  switchingModelId = modelId;
  renderModels();
  try {
    const updated = await transport.call<AppStateDto>("switch_model", { modelId });
    if (updated) applyState(updated);
    toast(`Activated ${modelId}`);
  } catch (err) {
    toast(`Failed to switch to ${modelId}: ${message(err)}`, "error");
  } finally {
    switchingModelId = null;
    renderModels();
  }
}

async function deleteModel(modelId: string): Promise<void> {
  deletingModelId = modelId;
  renderModels();
  try {
    const updated = await transport.call<AppStateDto>("delete_model", { modelId });
    if (updated) applyState(updated);
    toast(`Deleted ${modelId}`);
  } catch (err) {
    toast(`Failed to delete ${modelId}: ${message(err)}`, "error");
  } finally {
    deletingModelId = null;
    renderModels();
  }
}

function renderSettings(force: boolean): void {
  if (!state) return;
  if (settingsDirty && !force) return;
  const settings = state.settings;
  setValue("#language", settings.language);
  setValue("#template", settings.template);
  setValue("#gift-template", settings.giftTemplate);
  setValue("#max-len", String(settings.maxLen));
  setValue("#cooldown", String(settings.cooldownSecs));
  setValue("#min-gift", String(settings.minGiftDiamonds));
  setValue("#blacklist", settings.blacklist.join("\n"));
  $<HTMLInputElement>("#read-gifts").checked = settings.readGifts;
  $<HTMLInputElement>("#transliterate").checked = settings.transliterate;
  if (force) {
    $("#save-hint").textContent = "Saved";
  }
}

function renderAppSettings(force: boolean): void {
  if (!state) return;
  if (appConfigDirty && !force) return;
  setValue("#queue-capacity", String(state.config.queueCapacity));
  setValue("#server-port", String(state.config.serverPort));
  const autoConnect = $<HTMLInputElement>("#auto-connect");
  if (document.activeElement !== autoConnect) {
    autoConnect.checked = state.config.autoConnect;
  }
  $("#server-port-field").classList.toggle("hidden", !state.serverMode);
  $("#app-settings-note").textContent = state.serverMode
    ? "Auto-connect uses the last saved username. Server port changes apply after restarting server mode."
    : "Auto-connect uses the last saved username. Queue capacity applies immediately.";
  if (force) {
    $("#app-save-hint").textContent = "Saved";
  }
}

function renderNow(): void {
  if (!state) return;
  const active = state.queue.active;
  const eq = $("#eq");
  eq.classList.toggle("is-running", Boolean(active && !state.paused));
  eq.classList.toggle("is-idle", !active || state.paused);
  $("#pause-btn").textContent = state.paused ? "Resume" : "Pause";
  $("#pause-btn").classList.toggle("is-active", state.paused);
  $<HTMLButtonElement>("#skip-btn").disabled = !active;

  const body = $("#now-body");
  if (!active) {
    playing = null;
    body.innerHTML = `
      <div class="now-empty">
        <div class="now-empty-mark"></div>
        <p>Queue is quiet.</p>
        <span>Chat comments and finished gift streaks will appear here.</span>
      </div>`;
    return;
  }

  const isPlaying = playing?.id === active.id;
  const progress = isPlaying
    ? `<div class="progress" aria-hidden="true"><span style="animation-duration:${Math.max(
        300,
        playing?.durationMs ?? 0,
      )}ms"></span></div>`
    : "";
  body.innerHTML = `
    <div class="now-meta">
      <span class="badge ${kindClass(active.kind)}">${kindLabel(active.kind)}</span>
      <span class="now-user">${esc(active.user)}</span>
      <span class="now-voice">${esc(displayVoice(active.voice))} · ${active.speed.toFixed(
        2,
      )}×</span>
    </div>
    <p class="now-text">${esc(active.text)}</p>
    ${progress}`;
}

function renderQueue(): void {
  if (!state) return;
  const queue = state.queue;
  $("#queue-count").textContent = `${queue.items.length}/${queue.capacity}`;
  $<HTMLButtonElement>("#clear-btn").disabled = queue.items.length === 0;

  const list = $("#queue-list");
  if (queue.items.length === 0) {
    list.innerHTML = `<div class="empty">Nothing waiting. Yapper speaks one message at a time.</div>`;
    return;
  }
  list.innerHTML = queue.items
    .map(
      (item, index) => `
      <div class="queue-row">
        <span class="queue-index">${String(index + 1).padStart(2, "0")}</span>
        <div class="queue-main">
          <div class="queue-top">
            <span class="queue-user">${esc(item.user)}</span>
            <span class="badge ${kindClass(item.kind)}">${kindLabel(item.kind)}</span>
          </div>
          <p class="queue-text">${esc(item.text)}</p>
        </div>
      </div>`,
    )
    .join("");
}

function renderActivity(): void {
  const list = $("#activity-list");
  const entries = feed.filter(matchesFilter);
  if (entries.length === 0) {
    list.innerHTML = `<div class="empty">${
      feed.length === 0
        ? "Waiting for the LIVE chat to start moving."
        : "No entries for this filter yet."
    }</div>`;
    return;
  }
  list.innerHTML = entries
    .slice(0, 80)
    .map((entry) => {
      const reason =
        entry.reason && (entry.status === "filtered" || entry.status === "dropped")
          ? `<div class="feed-reason">${esc(entry.reason)}</div>`
          : "";
      return `
      <div class="feed-row feed-${entry.status}">
        <div class="feed-top">
          <span class="badge ${kindClass(entry.kind)}">${kindLabel(entry.kind)}</span>
          <span class="feed-user">${esc(entry.user)}</span>
          <span class="feed-status">${esc(entry.status)}</span>
          <span class="feed-time">${timeAgo(entry.at)}</span>
        </div>
        <p class="feed-text">${esc(entry.text)}</p>
        ${reason}
      </div>`;
    })
    .join("");
}

function renderStats(): void {
  if (!state) return;
  $("#stat-received").textContent = formatNumber(state.stats.received);
  $("#stat-spoken").textContent = formatNumber(state.stats.spoken);
  $("#stat-filtered").textContent = formatNumber(state.stats.filtered);
  $("#stat-dropped").textContent = formatNumber(state.stats.dropped);
  const dir = $("#data-dir");
  dir.textContent = state.dataDir;
  dir.title = state.dataDir;
}

function bind(): void {
  $("#connect-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void toggleConnection();
  });

  $("#test-form").addEventListener("submit", (event) => {
    event.preventDefault();
    void speakTest();
  });

  $("#models-toggle-btn").addEventListener("click", () => {
    modelsExpanded = !modelsExpanded;
    $<HTMLButtonElement>("#models-toggle-btn").dataset.manuallyToggled = "true";
    renderModels();
  });

  $("#pause-btn").addEventListener("click", () => {
    if (!state) return;
    player.stop();
    void command("set_paused", { paused: !state.paused });
  });

  $("#skip-btn").addEventListener("click", () => {
    player.stop();
    void command("skip");
  });

  $("#clear-btn").addEventListener("click", () => {
    void command("clear_queue");
  });

  $("#feed-filters").addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      "button[data-filter]",
    );
    if (!button) return;
    feedFilter = (button.dataset.filter as typeof feedFilter) ?? "all";
    document
      .querySelectorAll<HTMLButtonElement>("#feed-filters .chip")
      .forEach((chip) =>
        chip.classList.toggle("is-active", chip === button),
      );
    renderActivity();
  });

  const settingsSelector =
    "#language, #voice, #speed, #volume, #template, #gift-template, #max-len, #cooldown, #min-gift, #blacklist, #read-gifts, #transliterate";
  document.querySelectorAll(settingsSelector).forEach((element) => {
    element.addEventListener("input", () => {
      if (element.id === "speed") updateSpeedLabel();
      if (element.id === "volume") {
        updateVolumeLabel();
        player.setVolume(Number($<HTMLInputElement>("#volume").value));
      }
      markSettingsDirty();
    });
    element.addEventListener("change", () => {
      if (element.id === "language") {
        markSettingsDirty();
        renderVoice();
      } else {
        if (element.id === "volume") {
          updateVolumeLabel();
          player.setVolume(Number($<HTMLInputElement>("#volume").value));
        }
        markSettingsDirty();
      }
    });
  });

  const appConfigSelector = "#queue-capacity, #server-port, #auto-connect";
  document.querySelectorAll(appConfigSelector).forEach((element) => {
    element.addEventListener("input", markAppConfigDirty);
    element.addEventListener("change", markAppConfigDirty);
  });

  document.addEventListener(
    "pointerdown",
    () => {
      void unlockAudio();
    },
    { passive: true },
  );
  $("#audio-banner").addEventListener("click", () => void unlockAudio());
}

async function toggleConnection(): Promise<void> {
  if (!state) return;
  const status = state.connection.status;
  if (
    status === "live" ||
    status === "waiting" ||
    status === "reconnecting" ||
    status === "offline" ||
    status === "error"
  ) {
    await command("disconnect");
    return;
  }
  if (status === "connecting") {
    await command("disconnect");
    return;
  }

  const input = $<HTMLInputElement>("#username");
  const username = input.value.trim().replace(/^@/, "");
  if (!username) {
    toast("Enter the TikTok username to listen to", "error");
    input.focus();
    return;
  }
  const button = $<HTMLButtonElement>("#connect-btn");
  button.disabled = true;
  try {
    applyState(
      await transport.call<AppStateDto>("connect", { username }),
    );
  } catch (error) {
    toast(message(error), "error");
  } finally {
    button.disabled = false;
  }
}

async function speakTest(): Promise<void> {
  const input = $<HTMLInputElement>("#test-text");
  const text = input.value.trim();
  if (!text) {
    input.focus();
    return;
  }
  try {
    applyState(await transport.call<AppStateDto>("speak", { text }));
    toast("Queued for speech");
  } catch (error) {
    toast(message(error), "error");
  }
}

async function command(
  name: string,
  args: Record<string, unknown> = {},
): Promise<void> {
  try {
    const next = await transport.call<AppStateDto>(name, args);
    if (next) applyState(next);
  } catch (error) {
    toast(message(error), "error");
  }
}

function markSettingsDirty(): void {
  settingsDirty = true;
  settingsRevision += 1;
  $("#save-hint").textContent = "Saving…";
  if (saveTimer) window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => void saveSettings(), 650);
}

async function saveSettings(): Promise<void> {
  if (!state) return;
  if (saving) return;
  saving = true;
  const revision = settingsRevision;
  try {
    const settings = collectSettings();
    const next = await transport.call<AppStateDto>("update_settings", {
      settings,
    });
    state = next;
    if (revision === settingsRevision) {
      settingsDirty = false;
      $("#save-hint").textContent = "Saved";
    } else {
      window.setTimeout(() => void saveSettings(), 200);
    }
    renderVoice();
    renderSettings(false);
    renderNow();
    renderQueue();
    renderStats();
  } catch (error) {
    toast(`Could not save settings: ${message(error)}`, "error");
    $("#save-hint").textContent = "Not saved";
  } finally {
    saving = false;
  }
}

function markAppConfigDirty(): void {
  appConfigDirty = true;
  appConfigRevision += 1;
  $("#app-save-hint").textContent = "Saving…";
  if (appConfigTimer) window.clearTimeout(appConfigTimer);
  appConfigTimer = window.setTimeout(() => void saveAppConfig(), 650);
}

async function saveAppConfig(): Promise<void> {
  if (!state) return;
  if (savingAppConfig) return;
  savingAppConfig = true;
  const revision = appConfigRevision;
  try {
    const config = collectAppConfig();
    const next = await transport.call<AppStateDto>("update_app_config", {
      config,
    });
    state = next;
    if (revision === appConfigRevision) {
      appConfigDirty = false;
      $("#app-save-hint").textContent = "Saved";
    } else {
      window.setTimeout(() => void saveAppConfig(), 200);
    }
    renderAppSettings(false);
    renderQueue();
  } catch (error) {
    toast(`Could not save app settings: ${message(error)}`, "error");
    $("#app-save-hint").textContent = "Not saved";
  } finally {
    savingAppConfig = false;
  }
}

function collectAppConfig(): AppConfig {
  const current = state?.config;
  const queueCapacity = Number($<HTMLInputElement>("#queue-capacity").value);
  const serverPort = Number($<HTMLInputElement>("#server-port").value);
  return {
    queueCapacity: clamp(
      Math.round(queueCapacity) || current?.queueCapacity || 15,
      1,
      500,
    ),
    autoConnect: $<HTMLInputElement>("#auto-connect").checked,
    serverPort: clamp(
      Math.round(serverPort) || current?.serverPort || 8787,
      1,
      65535,
    ),
  };
}

function collectSettings(): TtsSettings {
  const current = state?.settings;
  const language = $<HTMLSelectElement>("#language").value || "auto";
  const voice = $<HTMLSelectElement>("#voice").value;
  const speed = Number($<HTMLInputElement>("#speed").value);
  const rawVolume = Number($<HTMLInputElement>("#volume").value);
  const volume = Number.isFinite(rawVolume)
    ? clamp(rawVolume, 0, 1)
    : (current?.volume ?? 1);
  return {
    language,
    voice: voice || current?.voice || "Bella",
    speed: clamp(speed, 0.5, 2),
    volume,
    template: $<HTMLInputElement>("#template").value,
    giftTemplate: $<HTMLInputElement>("#gift-template").value,
    maxLen: clamp(Number($<HTMLInputElement>("#max-len").value) || 150, 10, 500),
    cooldownSecs: clamp(
      Number($<HTMLInputElement>("#cooldown").value) || 0,
      0,
      60,
    ),
    minGiftDiamonds: Math.max(
      0,
      Number($<HTMLInputElement>("#min-gift").value) || 0,
    ),
    blacklist: $<HTMLTextAreaElement>("#blacklist")
      .value.split(/[\n,]+/)
      .map((word) => word.trim())
      .filter(Boolean),
    readGifts: $<HTMLInputElement>("#read-gifts").checked,
    transliterate: $<HTMLInputElement>("#transliterate").checked,
  };
}

function updateSpeedLabel(): void {
  const speed = Number($<HTMLInputElement>("#speed").value);
  $("#speed-value").textContent = `${speed.toFixed(2)}×`;
}

function updateVolumeLabel(): void {
  const volume = clamp(Number($<HTMLInputElement>("#volume").value), 0, 1);
  $("#volume-value").textContent = `${Math.round(volume * 100)}%`;
}

function matchesFilter(entry: ActivityEvent): boolean {
  if (feedFilter === "all") return true;
  if (feedFilter === "gift") return entry.kind === "gift";
  return entry.kind === "chat" || entry.kind === "test";
}

async function unlockAudio(): Promise<void> {
  await player.unlock();
  if (!player.suspended) hideAudioBanner();
}

function showAudioBanner(): void {
  $("#audio-banner").classList.remove("hidden");
}

function hideAudioBanner(): void {
  $("#audio-banner").classList.add("hidden");
}

function toast(messageText: string, kind: "info" | "error" = "info"): void {
  const element = $("#toast");
  element.textContent = messageText;
  element.className = `toast toast-${kind}`;
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    element.classList.add("hidden");
  }, 4800);
}

function setValue(selector: string, value: string): void {
  const input = document.querySelector<HTMLInputElement>(selector);
  if (input && document.activeElement !== input) input.value = value;
}

function displayVoice(voice: string): string {
  if (voice.startsWith("uk-UA-")) {
    return voice
      .replace(/^uk-UA-(\w+)Neural$/, "Polina/Ostap ($1)")
      .replace(/^uk-UA-/, "Ukrainian ");
  }
  return voice
    .replace(/^expr-voice-(\d)-([fm])$/i, "Voice $1$2")
    .replace(/^kitten-/, "");
}

function kindLabel(kind: string): string {
  switch (kind) {
    case "chat":
      return "CHAT";
    case "gift":
      return "GIFT";
    case "test":
      return "TEST";
    case "follow":
      return "FOLLOW";
    case "share":
      return "SHARE";
    case "subscribe":
      return "SUB";
    default:
      return kind.toUpperCase() || "EVENT";
  }
}

function kindClass(kind: string): string {
  switch (kind) {
    case "gift":
      return "badge-amber";
    case "follow":
      return "badge-blue";
    case "share":
      return "badge-blue";
    case "subscribe":
      return "badge-violet";
    case "test":
      return "badge-muted";
    default:
      return "badge-mint";
  }
}

function timeAgo(at: number): string {
  const seconds = Math.max(0, Math.round((Date.now() - at) / 1000));
  if (seconds < 5) return "now";
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h`;
}

function formatNumber(value: number): string {
  return new Intl.NumberFormat(undefined, { notation: "compact" }).format(
    value,
  );
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function message(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function esc(value: string): string {
  return value.replace(/[&<>"']/g, (character) => {
    switch (character) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return "&#39;";
    }
  });
}

function shell(): string {
  return `
<div class="app-shell">
  <header class="topbar">
    <div class="brand">
      <div class="brand-mark" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></div>
      <div>
        <div class="brand-name">Yapper</div>
        <div class="brand-sub">TikTok LIVE → local voice</div>
      </div>
    </div>
    <div class="topbar-right">
      <span id="server-badge" class="badge badge-muted hidden">LAN server</span>
      <span id="status-pill" class="status-pill status-idle"><i></i><span>Not connected</span></span>
    </div>
  </header>

  <main class="layout">
    <aside class="col-left">
      <section class="card">
        <div class="card-head">
          <h2>Connect</h2>
          <span class="hint">anonymous · no login</span>
        </div>
        <form id="connect-form" class="connect-form">
          <div class="input-affix">
            <span>@</span>
            <input id="username" placeholder="tiktok_username" autocomplete="off" spellcheck="false" maxlength="32" aria-label="TikTok username">
          </div>
          <button id="connect-btn" class="btn btn-primary" type="submit">Connect</button>
        </form>
        <div id="connect-meta" class="connect-meta">
          <p class="hint">No login needed. Enter the streamer’s handle to start reading chat.</p>
        </div>
      </section>

      <section class="card">
        <div class="card-head">
          <h2>Voice</h2>
          <span id="engine-badge" class="badge badge-muted">engine</span>
        </div>
        <p id="engine-line" class="hint engine-line"></p>
        <div class="voice-grid">
          <label class="field field-wide">
            <span class="field-label">Spoken language</span>
            <select id="language" class="select">
              <option value="auto">Auto (English / Ukrainian)</option>
              <option value="en">English — local</option>
              <option value="uk">Ukrainian — online</option>
            </select>
          </label>
          <label class="field">
            <span class="field-label">Voice</span>
            <select id="voice" class="select"></select>
          </label>
          <label class="field">
            <span class="field-label">Speed <b id="speed-value">1.00×</b></span>
            <input id="speed" class="range" type="range" min="0.5" max="2" step="0.05" value="1">
          </label>
          <label class="field field-wide">
            <span class="field-label">Volume <b id="volume-value">100%</b></span>
            <input id="volume" class="range" type="range" min="0" max="1" step="0.01" value="1">
          </label>
        </div>
        <p id="language-note" class="note hidden"></p>
        <p id="engine-note" class="note hidden"></p>
        <div id="models-section" class="models-section">
          <div class="models-header">
            <div class="models-header-left">
              <span class="models-title">Local speech models</span>
              <span id="models-summary-badge" class="badge badge-muted">models</span>
            </div>
            <button id="models-toggle-btn" class="btn btn-ghost btn-sm" type="button">
              <span id="models-toggle-text">Manage</span>
              <span class="chevron">▾</span>
            </button>
          </div>
          <div id="models-warning-banner" class="models-warning-banner hidden">
            <span class="warning-icon">⚠</span>
            <div class="warning-text">
              <strong>Mock voice active</strong>
              <p>Download a KittenTTS model below to enable real neural voice synthesis on this device.</p>
            </div>
          </div>
          <div id="models-list" class="models-list"></div>
        </div>
        <form id="test-form" class="test-form">
          <input id="test-text" class="input" placeholder="Test the voice…" maxlength="150">
          <button class="btn" type="submit">Speak</button>
        </form>
      </section>

      <details class="card filters" id="filters">
        <summary class="card-head filters-summary">
          <h2>Filters &amp; gifts</h2>
          <span id="save-hint" class="hint">Saved</span>
        </summary>
        <div class="filters-body">
          <label class="field">
            <span class="field-label">Chat template</span>
            <input id="template" class="input" maxlength="300">
          </label>
          <p class="hint">Placeholders: <code>{user}</code> <code>{text}</code></p>
          <label class="field">
            <span class="field-label">Gift template</span>
            <input id="gift-template" class="input" maxlength="300">
          </label>
          <p class="hint">Placeholders: <code>{user}</code> <code>{gift}</code> <code>{count}</code></p>
          <div class="grid-2">
            <label class="field">
              <span class="field-label">Max characters</span>
              <input id="max-len" type="number" min="10" max="500" class="input">
            </label>
            <label class="field">
              <span class="field-label">Cooldown (s)</span>
              <input id="cooldown" type="number" min="0" max="60" class="input">
            </label>
          </div>
          <label class="check"><input id="read-gifts" type="checkbox"> Read gifts aloud</label>
          <label class="check"><input id="transliterate" type="checkbox"> Transliterate other scripts</label>
          <label class="field">
            <span class="field-label">Minimum gift diamonds</span>
            <input id="min-gift" type="number" min="0" class="input">
          </label>
          <label class="field">
            <span class="field-label">Blacklist words</span>
            <textarea id="blacklist" class="input textarea" rows="2" placeholder="one per line"></textarea>
          </label>
        </div>
      </details>

      <details class="card filters app-settings" id="app-settings">
        <summary class="card-head filters-summary">
          <h2>App settings</h2>
          <span id="app-save-hint" class="hint">Saved</span>
        </summary>
        <div class="filters-body">
          <div class="grid-2">
            <label class="field">
              <span class="field-label">Queue capacity</span>
              <input id="queue-capacity" type="number" min="1" max="500" class="input">
            </label>
            <label class="field" id="server-port-field">
              <span class="field-label">Server port</span>
              <input id="server-port" type="number" min="1" max="65535" class="input">
            </label>
          </div>
          <label class="check"><input id="auto-connect" type="checkbox"> Connect automatically on launch</label>
          <p class="hint" id="app-settings-note">Auto-connect uses the last saved username. Queue capacity applies immediately.</p>
        </div>
      </details>
    </aside>

    <section class="col-right">
      <section class="card now">
        <div class="card-head">
          <div class="now-heading">
            <span class="eq is-idle" id="eq" aria-hidden="true"><i></i><i></i><i></i><i></i></span>
            <h2>Now speaking</h2>
          </div>
          <div class="row-actions">
            <button id="pause-btn" class="btn btn-ghost" type="button">Pause</button>
            <button id="skip-btn" class="btn btn-ghost" type="button">Skip</button>
          </div>
        </div>
        <div id="now-body"></div>
      </section>

      <div class="columns">
        <section class="card queue">
          <div class="card-head">
            <h2>Queue <span id="queue-count" class="count">0/15</span></h2>
            <button id="clear-btn" class="btn btn-ghost" type="button">Clear</button>
          </div>
          <div id="queue-list" class="list scroll"></div>
        </section>

        <section class="card activity">
          <div class="card-head">
            <h2>Live activity</h2>
            <div class="chips" id="feed-filters">
              <button data-filter="all" class="chip is-active" type="button">All</button>
              <button data-filter="chat" class="chip" type="button">Chat</button>
              <button data-filter="gift" class="chip" type="button">Gifts</button>
            </div>
          </div>
          <div id="activity-list" class="list scroll"></div>
        </section>
      </div>
    </section>
  </main>

  <footer class="statusbar">
    <span><b id="stat-received">0</b> received</span>
    <span><b id="stat-spoken">0</b> spoken</span>
    <span><b id="stat-filtered">0</b> filtered</span>
    <span><b id="stat-dropped">0</b> dropped</span>
    <span id="data-dir" class="statusbar-dir"></span>
  </footer>
</div>
<button id="audio-banner" class="audio-banner hidden" type="button">Click anywhere to enable audio</button>
<div id="toast" class="toast hidden"></div>`;
}
