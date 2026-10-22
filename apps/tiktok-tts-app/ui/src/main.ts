import "./style.css";
import { Player } from "./player";
import { Transport } from "./transport";
import type {
  ActivityEvent,
  AppStateDto,
  ConnStatus,
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
let playing: { id: number; durationMs: number } | null = null;
let currentSeq: number | null = null;
let lastStatus: ConnStatus | null = null;
let toastTimer: number | undefined;

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
    applyState(await transport.fetchState());
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
      void playUtterance(event.seq, event.wav, event.item.id, event.durationMs);
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
  }
}

function applyState(next: AppStateDto): void {
  const previous = state;
  state = next;
  if (
    previous &&
    previous.connection.status !== next.connection.status &&
    lastStatus !== next.connection.status
  ) {
    lastStatus = next.connection.status;
    if (next.connection.status === "live") {
      toast(`Connected to @${next.connection.username}`);
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
    connection.status === "reconnecting" ||
    connection.status === "offline" ||
    connection.status === "error";

  if (document.activeElement !== input) {
    if (!input.value && connection.username) input.value = connection.username;
  }
  input.disabled = busy || connection.status === "live";
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
  const engine = state.engine;
  const badge = $("#engine-badge");
  badge.textContent = engine.kind === "onnx" ? "ONNX" : "MOCK";
  badge.className = `badge ${engine.kind === "onnx" ? "badge-mint" : "badge-amber"}`;
  badge.title = engine.model;

  $("#engine-line").textContent =
    engine.kind === "onnx"
      ? `${engine.model} · ${engine.sampleRate / 1000} kHz · ${engine.threads} threads`
      : "Tone test engine — model not loaded";

  const note = $("#engine-note");
  if (state.engineNote) {
    note.textContent = state.engineNote;
    note.classList.remove("hidden");
  } else {
    note.classList.add("hidden");
  }

  const select = $<HTMLSelectElement>("#voice");
  const voices = (engine.voices.length ? engine.voices : FALLBACK_VOICES).slice();
  const current = settingsDirty ? select.value : state.settings.voice;
  if (current && !voices.includes(current)) voices.unshift(current);
  if (document.activeElement !== select && !settingsDirty) {
    const signature = voices.join("|");
    if (select.dataset.signature !== signature) {
      select.innerHTML = voices
        .map(
          (voice) =>
            `<option value="${esc(voice)}">${esc(displayVoice(voice))}</option>`,
        )
        .join("");
      select.dataset.signature = signature;
    }
    select.value = state.settings.voice;
  }

  const speed = $<HTMLInputElement>("#speed");
  if (document.activeElement !== speed && !settingsDirty) {
    speed.value = state.settings.speed.toFixed(2);
  }
  $("#speed-value").textContent = `${Number(
    settingsDirty ? speed.value : state.settings.speed,
  ).toFixed(2)}×`;
}

function renderSettings(force: boolean): void {
  if (!state) return;
  if (settingsDirty && !force) return;
  const settings = state.settings;
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

  $("#pause-btn").addEventListener("click", () => {
    if (!state) return;
    void command("set_paused", { paused: !state.paused });
  });

  $("#skip-btn").addEventListener("click", () => {
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
    "#voice, #speed, #template, #gift-template, #max-len, #cooldown, #min-gift, #blacklist, #read-gifts, #transliterate";
  document.querySelectorAll(settingsSelector).forEach((element) => {
    element.addEventListener("input", () => {
      if (element.id === "speed") updateSpeedLabel();
      markSettingsDirty();
    });
    element.addEventListener("change", () => markSettingsDirty());
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

function collectSettings(): TtsSettings {
  const current = state?.settings;
  const voice = $<HTMLSelectElement>("#voice").value;
  const speed = Number($<HTMLInputElement>("#speed").value);
  return {
    voice: voice || current?.voice || "Bella",
    speed: clamp(speed, 0.5, 2),
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
          <label class="field">
            <span class="field-label">Voice</span>
            <select id="voice" class="select"></select>
          </label>
          <label class="field">
            <span class="field-label">Speed <b id="speed-value">1.00×</b></span>
            <input id="speed" class="range" type="range" min="0.5" max="2" step="0.05" value="1">
          </label>
        </div>
        <p id="engine-note" class="note hidden"></p>
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
