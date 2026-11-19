<script lang="ts">
  import { store, type Track, type TrackStatus } from "../lib/api.svelte";
  import { clock, copy } from "../lib/format";
  import TrackReview from "../components/TrackReview.svelte";
  import MusicSettings from "../components/MusicSettings.svelte";

  const STATUS: Record<TrackStatus, { text: string; tone: string }> = {
    resolving: { text: "ищу трек…", tone: "idle" },
    checking: { text: "проверяю…", tone: "idle" },
    ready: { text: "готов", tone: "ok" },
    needs_review: { text: "нужна проверка", tone: "warn" },
    rejected: { text: "отклонён", tone: "bad" },
    failed: { text: "ошибка", tone: "bad" },
    playing: { text: "играет", tone: "ok" },
    played: { text: "сыгран", tone: "idle" },
    skipped: { text: "пропущен", tone: "idle" },
  };
  const VIA = { panel: "🎛️", chat: "💬", reward: "🎁", donation: "💸" } as const;

  let query = $state("");
  let adding = $state(false);
  let error = $state("");
  let open = $state<string | null>(null);
  let tab = $state<"queue" | "history" | "settings">("queue");
  let copied = $state(false);
  let obsHelp = $state(false);

  const m = $derived(store.music);
  const now = $derived(m?.now ?? null);
  const progress = $derived(
    store.progress && now && store.progress.track_id === now.id
      ? store.progress
      : { position: m?.player.position ?? 0, duration: m?.player.duration },
  );
  const reviewCount = $derived(m?.queue.filter((t) => t.status === "needs_review").length ?? 0);
  const playerUrl = $derived(`${location.origin}/widgets/player.html?token=${store.token ?? ""}`);

  function title(t: Track) {
    if (t.meta) return t.meta.artist ? `${t.meta.artist} — ${t.meta.track_title}` : t.meta.title;
    return t.query;
  }

  async function run(fn: () => Promise<unknown>) {
    error = "";
    try {
      await fn();
    } catch (e) {
      error = (e as Error).message;
    }
  }

  const player = (body: Record<string, unknown>) => run(() => store.post("/music/player", body));
  const trackCmd = (id: string, body: Record<string, unknown>) => run(() => store.post(`/music/tracks/${id}`, body));

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!query.trim()) return;
    adding = true;
    await run(async () => {
      await store.post("/music/request", { query });
      query = "";
    });
    adding = false;
  }

  let volTimer: ReturnType<typeof setTimeout>;
  function setVolume(v: number) {
    clearTimeout(volTimer);
    volTimer = setTimeout(() => player({ action: "volume", value: v }), 120);
  }

  async function copyPlayer() {
    await copy(playerUrl);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<header class="head">
  <div>
    <h1>Музыка</h1>
    <p class="muted">Заказы из чата, наград и донатов. Каждый трек проверяется до эфира.</p>
  </div>
  <button
    class="btn panic"
    class:on={m?.player.panic}
    title="Мгновенно выключить музыку в эфире"
    onclick={() => player({ action: "panic", on: !m?.player.panic })}
  >
    {m?.player.panic ? "🔈 Вернуть звук" : "🚨 Тревога: выключить звук"}
  </button>
</header>

{#if m}
  <!-- Плеер в OBS -->
  {#if m.players_connected === 0}
    <div class="card notice">
      <div>
        <strong>Плеер не подключён.</strong>
        <span class="muted">Музыка играет через источник «Браузер» в OBS — добавьте его по ссылке:</span>
      </div>
      <div class="row">
        <button class="btn primary" onclick={copyPlayer}>{copied ? "Скопировано" : "Скопировать адрес плеера"}</button>
        <button class="btn ghost" onclick={() => (obsHelp = !obsHelp)}>Как настроить OBS</button>
      </div>
      {#if obsHelp}
        <ol class="steps">
          <li>OBS → Источники → <b>+</b> → <b>Браузер</b>. Вставьте адрес плеера, размер 460×100.</li>
          <li>Включите <b>«Управлять звуком через OBS»</b> — музыка станет отдельным источником звука.</li>
          <li>
            Чтобы музыки не было в записи: Настройки → Вывод → Потоковое вещание → включите <b>«Дорожка VOD Twitch»</b>
            (например, дорожка 2), затем в «Расширенных свойствах звука» снимите у плеера галочку этой дорожки.
          </li>
          <li>Адрес содержит ключ доступа — не показывайте его на стриме.</li>
        </ol>
      {/if}
    </div>
  {:else}
    <p class="faint small player-ok">
      <span class="dot ok"></span> Плеер подключён{m.players_connected > 1 ? ` (${m.players_connected})` : ""}
      · <button class="link" onclick={copyPlayer}>{copied ? "скопировано" : "адрес плеера"}</button>
    </p>
  {/if}

  <!-- Сейчас играет -->
  <section class="card now">
    {#if now}
      {#if now.meta?.thumbnail}<img class="cover" src={now.meta.thumbnail} alt="" />{:else}<div class="cover empty">🎵</div>{/if}
      <div class="now-info">
        <div class="now-title">{title(now)}</div>
        <div class="faint small">
          {VIA[now.requested_by.via]} {now.requested_by.name}{now.requested_by.amount ? ` · ${now.requested_by.amount}` : ""}
          {#if now.mutes.some((x) => x.enabled)} · 🔇 {now.mutes.filter((x) => x.enabled).length}{/if}
        </div>
        <div class="progress">
          <span class="mono small">{clock(progress.position)}</span>
          <div class="bar">
            <div class="fill" style:width={progress.duration ? `${(progress.position / progress.duration) * 100}%` : "0%"}></div>
            {#if progress.duration}
              {#each now.mutes.filter((x) => x.enabled) as mt}
                <div
                  class="mute-mark"
                  style:left={`${(mt.start / progress.duration) * 100}%`}
                  style:width={`${((mt.end - mt.start) / progress.duration) * 100}%`}
                ></div>
              {/each}
            {/if}
          </div>
          <span class="mono small">{clock(progress.duration)}</span>
        </div>
      </div>
    {:else}
      <div class="cover empty">🎵</div>
      <div class="now-info">
        <div class="now-title faint">{m.player.paused ? "Пауза" : "Ничего не играет"}</div>
        <div class="faint small">{m.queue.some((t) => t.status === "ready") ? "" : "Очередь пуста — закажите трек ниже или через чат"}</div>
      </div>
    {/if}
    <div class="controls">
      {#if m.player.paused}
        <button class="btn" title="Играть" onclick={() => player({ action: "play" })}>▶</button>
      {:else}
        <button class="btn" title="Пауза" onclick={() => player({ action: "pause" })}>⏸</button>
      {/if}
      <button class="btn" title="Следующий" disabled={!now} onclick={() => player({ action: "skip" })}>⏭</button>
      <input
        class="volume"
        type="range"
        min="0"
        max="1"
        step="0.01"
        value={m.player.volume}
        title="Громкость"
        oninput={(e) => setVolume(Number(e.currentTarget.value))}
      />
    </div>
  </section>

  <form class="add" onsubmit={add}>
    <input class="input" placeholder="Ссылка YouTube/SoundCloud или «исполнитель — название»" bind:value={query} />
    <button class="btn primary" type="submit" disabled={adding || !query.trim()}>{adding ? "…" : "Добавить"}</button>
  </form>
  {#if error}<p class="error-text">{error}</p>{/if}

  <div class="tabs">
    <button class="chip" class:active={tab === "queue"} onclick={() => (tab = "queue")}>
      Очередь · {m.queue.length}{#if reviewCount}<span class="count">{reviewCount}</span>{/if}
    </button>
    <button class="chip" class:active={tab === "history"} onclick={() => (tab = "history")}>История</button>
    <button class="chip" class:active={tab === "settings"} onclick={() => (tab = "settings")}>Настройки</button>
  </div>

  {#if tab === "settings"}
    <section class="card"><MusicSettings /></section>
  {:else}
    {@const list = tab === "queue" ? m.queue : m.history}
    {#if list.length === 0}
      <p class="faint empty-list">{tab === "queue" ? "В очереди пусто." : "Здесь появятся сыгранные и отклонённые треки."}</p>
    {/if}
    <ul class="list">
      {#each list as t, i (t.id)}
        {@const st = STATUS[t.status]}
        <li class="track card" class:review={t.status === "needs_review"}>
          <div class="track-row">
            <button class="track-main" onclick={() => (open = open === t.id ? null : t.id)}>
              {#if tab === "queue"}<span class="pos mono faint">{i + 1}</span>{/if}
              <span class="track-title">
                <span class="name">{title(t)}</span>
                <span class="faint small">
                  {VIA[t.requested_by.via]} {t.requested_by.name}
                  {#if t.meta?.duration} · {clock(t.meta.duration)}{/if}
                  {#if t.mutes.some((x) => x.enabled)} · 🔇 {t.mutes.filter((x) => x.enabled).length}{/if}
                  {#if t.check?.hate} · <span class="error-text">оскорбления: {t.check.hate}</span>{/if}
                </span>
              </span>
              <span class="status {st.tone}">{st.text}</span>
            </button>
            {#if tab === "queue"}
              <div class="track-actions">
                {#if i > 0}
                  <button class="btn small ghost" title="Играть следующим" onclick={() => trackCmd(t.id, { action: "move", to: 0 })}
                    >⤒</button
                  >
                {/if}
                <button class="btn small ghost" title="Убрать" onclick={() => trackCmd(t.id, { action: "remove" })}>✕</button>
              </div>
            {/if}
          </div>
          {#if open === t.id || (t.status === "needs_review" && open === null)}
            <TrackReview track={t} />
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
{:else}
  <p class="faint">Загрузка…</p>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    margin-bottom: 16px;
  }
  .head p {
    margin: 4px 0 0;
  }
  .small {
    font-size: 12px;
  }
  .panic {
    border-color: color-mix(in srgb, var(--bad) 50%, transparent);
    color: var(--bad);
    font-weight: 600;
  }
  .panic:hover:not(:disabled) {
    background: color-mix(in srgb, var(--bad) 12%, transparent);
    border-color: var(--bad);
  }
  .panic.on {
    background: var(--bad);
    color: #fff;
  }
  .notice {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 14px;
    border-color: color-mix(in srgb, var(--warn) 45%, transparent);
  }
  .row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .steps {
    margin: 0;
    padding-left: 20px;
    color: var(--text-2);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .steps b {
    color: var(--text);
  }
  .player-ok {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 10px;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent-strong);
    text-decoration: underline;
    cursor: pointer;
    font: inherit;
  }
  .now {
    display: grid;
    grid-template-columns: 64px 1fr auto;
    gap: 16px;
    align-items: center;
  }
  .cover {
    width: 64px;
    height: 64px;
    border-radius: 10px;
    object-fit: cover;
  }
  .cover.empty {
    display: grid;
    place-items: center;
    background: var(--surface-2);
    font-size: 26px;
  }
  .now-info {
    min-width: 0;
  }
  .now-title {
    font-weight: 650;
    font-size: 16px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .progress {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 10px;
    align-items: center;
    margin-top: 8px;
  }
  .bar {
    position: relative;
    height: 5px;
    border-radius: 5px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: linear-gradient(90deg, var(--accent), var(--love));
    transition: width 1s linear;
  }
  .mute-mark {
    position: absolute;
    top: 0;
    bottom: 0;
    min-width: 2px;
    background: var(--bad);
    opacity: 0.75;
  }
  .controls {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .volume {
    width: 110px;
    accent-color: var(--accent);
  }
  .add {
    display: flex;
    gap: 8px;
    margin: 16px 0 8px;
  }
  .tabs {
    display: flex;
    gap: 6px;
    margin: 18px 0 12px;
  }
  .chip {
    height: 30px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-2);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .chip.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .count {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 9px;
    background: var(--warn);
    color: #000;
    font-size: 11px;
    font-weight: 700;
    display: grid;
    place-items: center;
  }
  .empty-list {
    padding: 20px 0;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .track {
    padding: 10px 12px;
  }
  .track.review {
    border-color: color-mix(in srgb, var(--warn) 55%, transparent);
  }
  .track-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .track-main {
    flex: 1;
    min-width: 0;
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 0;
    border: 0;
    background: none;
    text-align: left;
    cursor: pointer;
  }
  .pos {
    width: 20px;
    text-align: right;
    font-size: 12px;
  }
  .track-title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .status {
    font-size: 12px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-2);
    white-space: nowrap;
  }
  .status.ok {
    color: var(--ok);
    border-color: color-mix(in srgb, var(--ok) 40%, transparent);
  }
  .status.warn {
    color: var(--warn);
    border-color: color-mix(in srgb, var(--warn) 50%, transparent);
  }
  .status.bad {
    color: var(--bad);
    border-color: color-mix(in srgb, var(--bad) 45%, transparent);
  }
  .track-actions {
    display: flex;
    gap: 2px;
  }
  @media (max-width: 760px) {
    .head {
      flex-direction: column;
    }
    .now {
      grid-template-columns: 52px 1fr;
    }
    .cover {
      width: 52px;
      height: 52px;
    }
    .controls {
      grid-column: 1 / -1;
    }
    .volume {
      flex: 1;
    }
    .status {
      display: none;
    }
  }
</style>
