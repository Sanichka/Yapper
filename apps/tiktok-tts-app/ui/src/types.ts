export type ConnStatus =
  | "idle"
  | "connecting"
  | "live"
  | "reconnecting"
  | "offline"
  | "error";

export interface RoomOwner {
  userId: string;
  uniqueId: string;
  nickname: string;
  profilePictureUrl: string;
  followerCount: number;
}

export interface RoomStatus {
  roomId: string;
  isLive: boolean;
  title: string;
  viewerCount: number;
  owner: RoomOwner;
  streamStartedAt?: number;
  coverUrl?: string;
  hlsUrl?: string;
}

export interface ConnectionState {
  status: ConnStatus;
  username: string;
  room: RoomStatus | null;
  error: string | null;
}

export interface EngineStatus {
  kind: "onnx" | "mock";
  model: string;
  sampleRate: number;
  voices: string[];
  threads: number;
}

export interface TtsSettings {
  voice: string;
  speed: number;
  template: string;
  blacklist: string[];
  maxLen: number;
  cooldownSecs: number;
  transliterate: boolean;
  readGifts: boolean;
  giftTemplate: string;
  minGiftDiamonds: number;
}

export interface QueueItem {
  id: number;
  user: string;
  uniqueId: string;
  text: string;
  raw: string;
  priority: "high" | "normal";
  kind: string;
  voice: string;
  speed: number;
  createdAt: number;
}

export interface QueueSnapshot {
  active: QueueItem | null;
  items: QueueItem[];
  dropped: number;
  capacity: number;
}

export interface Stats {
  received: number;
  spoken: number;
  filtered: number;
  dropped: number;
}

export interface AppStateDto {
  connection: ConnectionState;
  engine: EngineStatus;
  settings: TtsSettings;
  queue: QueueSnapshot;
  paused: boolean;
  stats: Stats;
  serverMode: boolean;
  engineNote: string | null;
  dataDir: string;
}

export interface ActivityEvent {
  id: number;
  kind: string;
  user: string;
  text: string;
  status: "queued" | "filtered" | "dropped" | "seen";
  reason: string | null;
}

export type ServerEvent =
  | { type: "state"; state: AppStateDto }
  | {
      type: "utterance";
      seq: number;
      item: QueueItem;
      wav: string;
      durationMs: number;
    }
  | ({ type: "activity" } & ActivityEvent)
  | { type: "log"; level: string; message: string };
