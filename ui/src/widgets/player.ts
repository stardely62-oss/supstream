// Виджет-плеер для OBS (browser source).
//
// Ядро решает, что играть; плеер только исполняет: грузит звук, ставит паузу,
// меняет громкость и глушит отмеченные места через GainNode с плавной сменой
// уровня (без щелчков). Раз в секунду сообщает позицию, в конце — «закончился».
//
// Параметры адреса: ?token=… (обязательно), &ui=0 — без карточки «сейчас играет».

import "./player.css";

interface Mute {
  start: number;
  end: number;
  enabled: boolean;
}

interface Track {
  id: string;
  query: string;
  requested_by: { name: string; via: string };
  meta?: { title: string; artist: string; track_title: string; thumbnail?: string; duration?: number };
  mutes: Mute[];
}

interface Player {
  current: string | null;
  paused: boolean;
  panic: boolean;
  volume: number;
}

interface MusicView {
  player: Player;
  now: Track | null;
}

const params = new URLSearchParams(location.search);
const token = params.get("token") ?? "";
const showUi = params.get("ui") !== "0";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const card = $("card");
const cover = $<HTMLImageElement>("cover");
const titleEl = $("title");
const whoEl = $("who");
const fill = $("fill");
const unlock = $<HTMLButtonElement>("unlock");

const audio = new Audio();
audio.preload = "auto";

let ctx: AudioContext | null = null;
let gain: GainNode | null = null;

let currentId: string | null = null;
let mutes: Mute[] = [];
let wantPlay = false;
let panic = false;
let mutedNow = false;

function ensureAudioGraph() {
  if (ctx) return;
  try {
    ctx = new AudioContext();
    const src = ctx.createMediaElementSource(audio);
    gain = ctx.createGain();
    src.connect(gain).connect(ctx.destination);
  } catch {
    ctx = null; // без Web Audio — глушим через audio.muted (грубее, но работает)
  }
}

async function post(path: string, body: unknown) {
  try {
    await fetch(`/api${path}`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    /* ядро недоступно — переподключимся через WebSocket */
  }
}

async function tryPlay() {
  ensureAudioGraph();
  try {
    if (ctx?.state === "suspended") await ctx.resume();
    await audio.play();
    unlock.hidden = true;
  } catch {
    // Обычный браузер не даёт играть без клика. В OBS этого ограничения нет.
    unlock.hidden = false;
  }
}

unlock.addEventListener("click", () => {
  void tryPlay();
});

function apply(view: MusicView) {
  const { player, now } = view;
  const nextId = player.current && now && now.id === player.current ? now.id : null;

  if (nextId !== currentId) {
    currentId = nextId;
    if (currentId) {
      audio.src = `/api/media/${encodeURIComponent(currentId)}?token=${encodeURIComponent(token)}`;
      audio.currentTime = 0;
    } else {
      audio.pause();
      audio.removeAttribute("src");
      audio.load();
    }
  }

  mutes = (now?.mutes ?? []).filter((m) => m.enabled);
  panic = player.panic;
  audio.volume = Math.max(0, Math.min(1, player.volume));
  wantPlay = !!currentId && !player.paused && !player.panic;

  if (wantPlay && audio.paused) void tryPlay();
  if (!wantPlay && !audio.paused) audio.pause();
  updateMute(true);

  if (showUi && now && currentId) {
    card.hidden = false;
    titleEl.textContent = now.meta ? (now.meta.artist ? `${now.meta.artist} — ${now.meta.track_title}` : now.meta.title) : now.query;
    whoEl.textContent = `заказал(а) ${now.requested_by.name}`;
    if (now.meta?.thumbnail) {
      cover.src = now.meta.thumbnail;
      cover.hidden = false;
    } else {
      cover.hidden = true;
    }
  } else {
    card.hidden = true;
  }
}

/** Глушение: проверяем позицию каждые 20 мс и заранее (+40 мс) начинаем приглушать. */
function updateMute(force = false) {
  const t = audio.currentTime;
  const shouldMute = panic || mutes.some((m) => t + 0.04 >= m.start && t < m.end);
  if (shouldMute === mutedNow && !force) return;
  mutedNow = shouldMute;
  if (ctx && gain) {
    // Паника — мгновенно; обычное глушение — плавно (~10 мс), чтобы не было щелчка.
    if (panic) gain.gain.setValueAtTime(0, ctx.currentTime);
    else gain.gain.setTargetAtTime(shouldMute ? 0 : 1, ctx.currentTime, 0.01);
  } else {
    audio.muted = shouldMute;
  }
}

setInterval(() => updateMute(), 20);

setInterval(() => {
  if (!currentId) return;
  const d = audio.duration;
  if (Number.isFinite(d) && d > 0) fill.style.width = `${(audio.currentTime / d) * 100}%`;
  if (!audio.paused) {
    void post("/music/player/report", {
      track_id: currentId,
      position: audio.currentTime,
      duration: Number.isFinite(d) ? d : null,
    });
  }
}, 1000);

audio.addEventListener("ended", () => {
  if (currentId) void post("/music/player/report", { track_id: currentId, ended: true });
});

audio.addEventListener("error", () => {
  if (!currentId || !audio.src) return;
  const code = audio.error?.code ?? 0;
  void post("/music/player/report", { track_id: currentId, error: `не удалось воспроизвести (код ${code})` });
});

let retry = 0;
function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/api/ws?client=player&token=${encodeURIComponent(token)}`);
  ws.onopen = () => (retry = 0);
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.kind === "hello" || m.kind === "music") apply(m.music);
  };
  ws.onclose = () => {
    setTimeout(connect, Math.min(1000 * 2 ** retry++, 10_000));
  };
}

if (!token) {
  document.body.innerHTML = '<p class="err">Нет ключа: скопируйте адрес плеера из панели Supstream (Музыка → Плеер для OBS).</p>';
} else {
  connect();
}

// Для отладки и автотестов.
(window as any).__player = {
  audio,
  get gain() {
    return gain?.gain.value ?? (audio.muted ? 0 : 1);
  },
  get muted() {
    return mutedNow;
  },
  get current() {
    return currentId;
  },
};
