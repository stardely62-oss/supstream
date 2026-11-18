// Связь панели с ядром: HTTP API + WebSocket-поток событий и статусов.

export type Source = "twitch" | "donationalerts" | "supstream";

export type EventType =
  | "donation"
  | "redemption"
  | "chat_message"
  | "follow"
  | "subscription"
  | "gift_subs"
  | "cheer"
  | "raid";

export interface StreamEvent {
  id: string;
  at: string;
  source: Source;
  test: boolean;
  type: EventType;
  data: Record<string, any>;
}

export type ConnState =
  | { state: "not_configured"; reason: string }
  | { state: "connecting" }
  | { state: "connected"; account: string }
  | { state: "error"; message: string; retry_in: number };

export interface DeviceLogin {
  user_code: string;
  verification_uri: string;
  expires_at: string;
}

export interface Connections {
  twitch: ConnState;
  donationalerts: ConnState;
  twitch_device_login?: DeviceLogin;
}

export interface Settings {
  twitch: {
    client_id: string;
    has_builtin_client_id: boolean;
    configured: boolean;
    login: string | null;
    missing_scopes: string[];
  };
  donationalerts: {
    client_id: string;
    client_secret_set: boolean;
    redirect_uri: string;
    logged_in: boolean;
  };
  server: { port: number; allow_lan: boolean };
  data_dir: string;
}

const TOKEN_KEY = "supstream.token";
const MAX_EVENTS = 300;

function readToken(): string | null {
  const params = new URLSearchParams(location.search);
  const fromUrl = params.get("token");
  if (fromUrl) {
    try {
      localStorage.setItem(TOKEN_KEY, fromUrl);
    } catch {
      /* хранилище недоступно — токен останется только в памяти */
    }
    params.delete("token");
    const rest = params.toString();
    history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : "") + location.hash);
    return fromUrl;
  }
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

class Store {
  token = $state<string | null>(readToken());
  online = $state(false);
  unauthorized = $state(false);
  version = $state("");
  connections = $state<Connections | null>(null);
  events = $state<StreamEvent[]>([]);

  #ws: WebSocket | null = null;
  #retry = 0;

  setToken(token: string) {
    this.token = token.trim();
    this.unauthorized = false;
    try {
      localStorage.setItem(TOKEN_KEY, this.token);
    } catch {
      /* без сохранения */
    }
    this.connect();
  }

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await fetch(`/api${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${this.token ?? ""}`,
        ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (res.status === 401) this.unauthorized = true;
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, data?.error ?? `Ошибка ${res.status}`);
    return data as T;
  }

  get = <T>(path: string) => this.request<T>("GET", path);
  post = <T>(path: string, body: unknown = {}) => this.request<T>("POST", path, body);
  put = <T>(path: string, body: unknown) => this.request<T>("PUT", path, body);

  connect() {
    this.#ws?.close();
    if (!this.token) return;
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/api/ws?token=${encodeURIComponent(this.token)}`);
    this.#ws = ws;

    ws.onopen = () => {
      this.online = true;
      this.#retry = 0;
    };
    ws.onmessage = (msg) => {
      const m = JSON.parse(msg.data);
      if (m.kind === "hello") {
        this.version = m.version;
        this.connections = m.status;
        this.events = (m.events as StreamEvent[]).slice().reverse();
      } else if (m.kind === "status") {
        this.connections = m.status;
      } else if (m.kind === "event") {
        const next = [m.event as StreamEvent, ...this.events];
        this.events = next.length > MAX_EVENTS ? next.slice(0, MAX_EVENTS) : next;
      }
    };
    ws.onclose = () => {
      if (this.#ws !== ws) return;
      this.online = false;
      // Браузер не сообщает код 401 при апгрейде — проверяем ключ отдельно.
      this.get("/status").catch(() => {});
      const delay = Math.min(1000 * 2 ** this.#retry++, 10_000);
      setTimeout(() => this.#ws === ws && !this.unauthorized && this.connect(), delay);
    };
  }
}

export const store = new Store();
