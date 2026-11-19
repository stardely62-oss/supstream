<script lang="ts">
  import { store, type Hit, type Track } from "../lib/api.svelte";
  import { clock, clockFine } from "../lib/format";

  let { track }: { track: Track } = $props();

  let error = $state("");
  let showAll = $state(false);

  const check = $derived(track.check);
  const flaggedLines = $derived(check?.lines.filter((l) => l.hits?.length) ?? []);
  const lines = $derived(showAll ? (check?.lines ?? []) : flaggedLines);
  const canDecide = $derived(["needs_review", "rejected", "ready", "failed"].includes(track.status));

  async function cmd(body: Record<string, unknown>) {
    error = "";
    try {
      await store.post(`/music/tracks/${track.id}`, body);
    } catch (e) {
      error = (e as Error).message;
    }
  }

  /** Строка → куски текста с пометкой нарушений. */
  function segments(text: string, hits: Hit[] = []) {
    const chars = Array.from(text);
    const out: { text: string; hit?: Hit }[] = [];
    let i = 0;
    for (const h of [...hits].sort((a, b) => a.start - b.start)) {
      if (h.start > i) out.push({ text: chars.slice(i, h.start).join("") });
      out.push({ text: chars.slice(h.start, h.end).join(""), hit: h });
      i = h.end;
    }
    if (i < chars.length) out.push({ text: chars.slice(i).join("") });
    return out;
  }

  function lineMuted(time?: number, end?: number) {
    if (time === undefined || end === undefined) return false;
    return track.mutes.some((m) => m.enabled && !m.auto && m.start <= time + 0.01 && m.end >= end - 0.01);
  }

  function toggleLine(time: number, end: number, text: string) {
    const existing = track.mutes.find((m) => !m.auto && Math.abs(m.start - time) < 0.01 && Math.abs(m.end - end) < 0.01);
    if (existing) cmd({ action: "remove_mute", mute_id: existing.id });
    else cmd({ action: "add_mute", start: time, end, label: `строка: ${text.slice(0, 40)}` });
  }
</script>

<div class="review">
  {#if track.error}
    <p class="error-text">{track.error}</p>
  {/if}

  {#if check}
    <ul class="reasons">
      {#each check.reasons as r}<li>{r}</li>{/each}
    </ul>
    {#if check.lyrics}
      <p class="faint small">
        Текст: {check.lyrics.matched_artist} — {check.lyrics.matched_title}
        ({check.lyrics.synced ? "с таймкодами" : "без таймкодов"}, {check.lyrics.source}).
        {#if check.precision === "aligned"}
          Места слов найдены по звуку — отметки точные.
        {:else if check.precision === "partial"}
          Часть слов найдена по звуку, остальные — оценка по строке (с запасом).
        {:else if check.precision === "line_estimate"}
          Место слова оценено по строке, поэтому отметки с запасом. Точнее — с «Точным глушением» в настройках.
        {/if}
      </p>
    {/if}
  {/if}

  {#if track.mutes.length}
    <div class="block">
      <h4>Глушение</h4>
      <ul class="mutes">
        {#each track.mutes as m (m.id)}
          <li>
            <label class="mute">
              <input
                type="checkbox"
                checked={m.enabled}
                onchange={(e) => cmd({ action: "set_mute", mute_id: m.id, enabled: e.currentTarget.checked })}
              />
              <span class="mono">{clockFine(m.start)}–{clockFine(m.end)}</span>
              <span class="label" class:hate={m.category === "hate"}>{m.label}</span>
              {#if !m.auto}<span class="faint small">вручную</span>{/if}
            </label>
            {#if m.category === "hate"}
              <span class="style faint small" title="Запретки только вырезаются">🔇 тишина</span>
            {:else}
              <button
                class="btn small ghost style"
                title="Переключить: пик или тишина"
                onclick={() => cmd({ action: "set_mute", mute_id: m.id, enabled: m.enabled, bleep: !m.bleep })}
              >
                {m.bleep ? "📢 пик" : "🔇 тишина"}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if check?.lines.length}
    <div class="block">
      <div class="lyrics-head">
        <h4>{showAll ? "Весь текст" : `Строки с нарушениями (${flaggedLines.length})`}</h4>
        <button class="btn small ghost" onclick={() => (showAll = !showAll)}>
          {showAll ? "Только нарушения" : "Показать весь текст"}
        </button>
      </div>
      {#if lines.length === 0}
        <p class="faint small">Нарушений нет.</p>
      {/if}
      <ol class="lyrics">
        {#each lines as l}
          <li class:flagged={l.hits?.length}>
            <span class="mono faint time">{l.time !== undefined ? clock(l.time) : ""}</span>
            <span class="text">
              {#each segments(l.text, l.hits) as seg}
                {#if seg.hit}<mark class={seg.hit.category}>{seg.text}</mark>{:else}{seg.text}{/if}
              {/each}
            </span>
            {#if l.time !== undefined && l.end !== undefined && l.text}
              <button
                class="btn small ghost"
                title="Заглушить всю строку"
                onclick={() => toggleLine(l.time!, l.end!, l.text)}
              >
                {lineMuted(l.time, l.end) ? "🔈 вернуть" : "🔇 строку"}
              </button>
            {/if}
          </li>
        {/each}
      </ol>
    </div>
  {/if}

  {#if error}<p class="error-text">{error}</p>{/if}

  {#if canDecide}
    <div class="actions">
      {#if track.status !== "ready"}
        <button class="btn primary" onclick={() => cmd({ action: "approve" })}>Пропустить в эфир</button>
      {/if}
      {#if track.status !== "rejected"}
        <button class="btn" onclick={() => cmd({ action: "reject" })}>Отклонить</button>
      {/if}
      <button class="btn ghost" onclick={() => cmd({ action: "recheck" })}>Проверить заново</button>
    </div>
  {/if}
</div>

<style>
  .review {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 4px 0 2px;
  }
  .review p {
    margin: 0;
  }
  .small {
    font-size: 12px;
  }
  h4 {
    margin: 0 0 6px;
    font-size: 13px;
    font-weight: 600;
  }
  .reasons {
    margin: 0;
    padding-left: 18px;
    color: var(--text-2);
  }
  .block {
    border-top: 1px solid var(--border);
    padding-top: 10px;
  }
  .mutes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .mutes li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .style {
    flex: none;
  }
  .mute {
    display: flex;
    gap: 10px;
    align-items: center;
    cursor: pointer;
  }
  .label {
    overflow-wrap: anywhere;
  }
  .label.hate {
    color: var(--bad);
    font-weight: 600;
  }
  .lyrics-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }
  .lyrics {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 340px;
    overflow-y: auto;
  }
  .lyrics li {
    display: grid;
    grid-template-columns: 48px 1fr auto;
    gap: 10px;
    align-items: center;
    padding: 3px 4px;
    border-radius: 6px;
  }
  .lyrics li.flagged {
    background: color-mix(in srgb, var(--warn) 8%, transparent);
  }
  .time {
    font-size: 12px;
  }
  .text {
    overflow-wrap: anywhere;
  }
  mark {
    border-radius: 4px;
    padding: 0 3px;
    color: inherit;
  }
  mark.profanity {
    background: color-mix(in srgb, var(--warn) 35%, transparent);
  }
  mark.hate {
    background: color-mix(in srgb, var(--bad) 40%, transparent);
    font-weight: 600;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  @media (max-width: 760px) {
    .lyrics li {
      grid-template-columns: 40px 1fr;
    }
    .lyrics li .btn {
      grid-column: 2;
      justify-self: start;
    }
  }
</style>
