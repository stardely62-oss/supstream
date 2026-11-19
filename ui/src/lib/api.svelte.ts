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

// ───── Музыка ─────

export type TrackStatus =
  | "resolving"
  | "checking"
  | "ready"
  | "needs_review"
  | "rejected"
  | "failed"
  | "playing"
  | "played"
  | "skipped";

export type Policy = "allow" | "mute" | "review" | "reject";
export type Category = "hate" | "profanity";

export interface Hit {
  start: number;
  end: number;
  word: string;
  category: Category;
}

export interface Mute {
  id: number;
  start: number;
  end: number;
  label: string;
  category?: Category;
  auto: boolean;
  enabled: boolean;
}

export interface Track {
  id: string;
  query: string;
  requested_by: { name: string; via: "panel" | "chat" | "reward" | "donation"; amount?: string };
  requested_at: string;
  status: TrackStatus;
  error?: string;
  meta?: {
    video_id: string;
    url: string;
    title: string;
    artist: string;
    track_title: string;
    duration?: number;
    thumbnail?: string;
    extractor: string;
    channel_id?: string;
    channel: string;
    local: boolean;
  };
  check?: {
    lyrics?: { source: string; synced: boolean; instrumental: boolean; matched_artist: string; matched_title: string };
    lines: { time?: number; end?: number; text: string; hits?: Hit[] }[];
    hate: number;
    profanity: number;
    reasons: string[];
    precision: string;
  };
  mutes: Mute[];
  approved: boolean;
}

export interface MusicView {
  player: { current: string | null; paused: boolean; position: number; duration?: number; panic: boolean; volume: number };
  players_connected: number;
  now: Track | null;
  queue: Track[];
  history: Track[];
}

export interface Progress {
  track_id: string | null;
  position: number;
  duration?: number;
}

export interface MusicSettings {
  enabled: boolean;
  max_duration_secs: number;
  per_user_limit: number;
  hate: Policy;
  profanity: Policy;
  no_lyrics: Policy;
  mute_padding: number;
  chat_enabled: boolean;
  chat_command: string;
  chat_min_role: "everyone" | "subscriber" | "vip" | "moderator";
  chat_replies: boolean;
  reward_title: string;
  donation_min_amount: number;
  volume: number;
  custom_rules: { pattern: string; category: Category; kind: "exact" | "prefix" | "contains" | "root" }[];
  allow_words: string[];
  source_mode: "any" | "safe";
  safe_channels: { channel_id: string; name: string; url: string }[];
  local_dirs: string[];
  reward_id: string;
  reward_cost: number;
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
  music = $state<MusicView | null>(null);
  progress = $state<Progress | null>(null);

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
        this.music = m.music;
      } else if (m.kind === "music") {
        this.music = m.music;
      } else if (m.kind === "music_progress") {
        this.progress = m.progress;
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
