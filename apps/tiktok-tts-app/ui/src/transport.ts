import type { AppStateDto, ServerEvent } from "./types";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

interface TauriBridge {
  core: { invoke: Invoke };
  event: {
    listen<T>(
      name: string,
      handler: (event: { payload: T }) => void,
    ): Promise<() => void>;
  };
}

declare global {
  interface Window {
    __TAURI__?: TauriBridge;
  }
}

function bridge(): TauriBridge | null {
  const candidate = window.__TAURI__;
  return candidate?.core?.invoke ? candidate : null;
}

export class Transport {
  readonly tauri = bridge();
  private socket: WebSocket | null = null;
  private retry = 0;
  private handler: ((event: ServerEvent) => void) | null = null;
  private disposed = false;

  async start(handler: (event: ServerEvent) => void): Promise<void> {
    this.handler = handler;
    if (this.tauri) {
      await this.tauri.event.listen<ServerEvent>("yapper://event", (event) =>
        handler(event.payload),
      );
      return;
    }
    this.connectSocket();
  }

  private connectSocket(): void {
    if (this.disposed) return;
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const socket = new WebSocket(`${scheme}://${location.host}/ws`);
    this.socket = socket;

    socket.onopen = () => {
      this.retry = 0;
    };
    socket.onmessage = (message) => {
      try {
        this.handler?.(JSON.parse(message.data as string) as ServerEvent);
      } catch (error) {
        console.warn("bad server event", error);
      }
    };
    socket.onclose = () => {
      if (this.disposed) return;
      const delay = Math.min(10_000, 750 * 2 ** this.retry++);
      window.setTimeout(() => this.connectSocket(), delay);
    };
    socket.onerror = () => socket.close();
  }

  async call<T>(
    command: string,
    args: Record<string, unknown> = {},
  ): Promise<T> {
    if (this.tauri) {
      return this.tauri.core.invoke<T>(command, args);
    }
    return this.rest<T>(command, args);
  }

  private async rest<T>(
    command: string,
    args: Record<string, unknown>,
  ): Promise<T> {
    let response: Response;
    switch (command) {
      case "get_state":
        response = await fetch("/api/state");
        break;
      case "connect":
        response = await post("/api/connect", { username: args.username });
        break;
      case "update_settings":
        response = await post("/api/settings", args.settings);
        break;
      case "update_app_config":
        response = await post("/api/config", args.config);
        break;
      case "speak":
        response = await post("/api/speak", { text: args.text });
        break;
      case "set_paused":
        response = await post("/api/queue/pause", { paused: args.paused });
        break;
      case "ack":
        response = await post("/api/ack", { seq: args.seq });
        break;
      case "disconnect":
        response = await post("/api/disconnect", {});
        break;
      case "skip":
        response = await post("/api/queue/skip", {});
        break;
      case "clear_queue":
        response = await post("/api/queue/clear", {});
        break;
      case "get_models":
        response = await fetch("/api/models");
        break;
      case "download_model":
        response = await post("/api/models/download", { modelId: args.modelId });
        break;
      case "switch_model":
        response = await post("/api/models/switch", { modelId: args.modelId });
        break;
      case "delete_model":
        response = await post("/api/models/delete", { modelId: args.modelId });
        break;
      default:
        throw new Error(`unknown command ${command}`);
    }

    if (!response.ok) {
      throw new Error(await readError(response));
    }
    if (response.status === 204) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }

  async fetchState(): Promise<AppStateDto> {
    return this.call<AppStateDto>("get_state");
  }
}

async function post(path: string, body: unknown): Promise<Response> {
  return fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function readError(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: string };
    if (body.error) return body.error;
  } catch {
    // not JSON
  }
  return `${response.status} ${response.statusText}`;
}
