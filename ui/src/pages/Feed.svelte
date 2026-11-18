<script lang="ts">
  import { store, type EventType } from "../lib/api.svelte";
  import { EVENT_META, SOURCE_LABEL, describe, time } from "../lib/format";

  type Filter = "all" | EventType;
  const FILTERS: { id: Filter; label: string }[] = [
    { id: "all", label: "Все" },
    { id: "donation", label: "Донаты" },
    { id: "redemption", label: "Баллы" },
    { id: "chat_message", label: "Чат" },
    { id: "follow", label: "Фолловы" },
    { id: "subscription", label: "Сабки" },
    { id: "cheer", label: "Битсы" },
    { id: "raid", label: "Рейды" },
  ];
  const TEST_TYPES = Object.keys(EVENT_META) as EventType[];

  let filter = $state<Filter>("all");
  let hideChat = $state(false);
  let testOpen = $state(false);
  let error = $state("");

  const visible = $derived(
    store.events.filter((e) => {
      if (filter === "all") return !(hideChat && e.type === "chat_message");
      if (filter === "subscription") return e.type === "subscription" || e.type === "gift_subs";
      return e.type === filter;
    }),
  );

  async function sendTest(type: EventType) {
    testOpen = false;
    error = "";
    try {
      await store.post("/events/test", { type });
    } catch (e) {
      error = (e as Error).message;
    }
  }
</script>

<header class="head">
  <div>
    <h1>Лента</h1>
    <p class="muted">Все события со всех площадок в реальном времени.</p>
  </div>
  <div class="test">
    <button class="btn" onclick={() => (testOpen = !testOpen)}>Тестовое событие ▾</button>
    {#if testOpen}
      <div class="menu card">
        {#each TEST_TYPES as t}
          <button class="menu-item" onclick={() => sendTest(t)}>
            <span>{EVENT_META[t].icon}</span>{EVENT_META[t].label}
          </button>
        {/each}
      </div>
    {/if}
  </div>
</header>

{#if error}<p class="error-text">{error}</p>{/if}

<div class="filters">
  {#each FILTERS as f}
    <button class="chip" class:active={filter === f.id} onclick={() => (filter = f.id)}>{f.label}</button>
  {/each}
  {#if filter === "all"}
    <label class="hide-chat faint">
      <input type="checkbox" bind:checked={hideChat} /> скрыть чат
    </label>
  {/if}
</div>

{#if visible.length === 0}
  <div class="empty card">
    <div class="empty-icon">📭</div>
    <p>Событий пока нет.</p>
    <p class="faint">
      Подключите Twitch и DonationAlerts во вкладке «Подключения» или отправьте тестовое событие кнопкой справа сверху.
    </p>
  </div>
{:else}
  <ul class="list">
    {#each visible as e (e.id)}
      {@const m = EVENT_META[e.type]}
      {@const d = describe(e)}
      <li class="item tone-{m.tone}" class:chat={e.type === "chat_message"}>
        <span class="icon">{m.icon}</span>
        <div class="body">
          <div class="line">
            <strong class="who" style:color={e.type === "chat_message" ? e.data.color || undefined : undefined}>{d.who}</strong>
            {#if d.what}<span class="what">{d.what}</span>{/if}
          </div>
          {#if d.text}<div class="text">{d.text}</div>{/if}
        </div>
        <div class="meta">
          {#if e.test}<span class="badge">тест</span>{/if}
          <span class="faint">{SOURCE_LABEL[e.source]}</span>
          <span class="faint mono">{time(e.at)}</span>
        </div>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    margin-bottom: 18px;
  }
  .head p {
    margin: 4px 0 0;
  }
  .test {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    top: 40px;
    z-index: 5;
    padding: 6px;
    min-width: 210px;
    display: flex;
    flex-direction: column;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.35);
  }
  .menu-item {
    display: flex;
    gap: 10px;
    padding: 8px 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    cursor: pointer;
  }
  .menu-item:hover {
    background: var(--surface-2);
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-bottom: 14px;
  }
  .chip {
    height: 30px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-2);
    cursor: pointer;
  }
  .chip.active {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .hide-chat {
    margin-left: 8px;
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 13px;
    cursor: pointer;
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .item {
    display: grid;
    grid-template-columns: 32px 1fr auto;
    gap: 12px;
    align-items: start;
    padding: 12px 14px;
    border-radius: var(--radius);
    background: var(--surface);
    border: 1px solid var(--border);
    border-left: 3px solid var(--tone, var(--border));
    animation: pop 0.25s ease-out;
  }
  .item.chat {
    padding: 8px 14px;
    background: transparent;
  }
  .tone-money {
    --tone: var(--money);
  }
  .tone-points {
    --tone: var(--points);
  }
  .tone-love {
    --tone: var(--love);
  }
  .tone-sub {
    --tone: var(--sub);
  }
  .icon {
    font-size: 20px;
    line-height: 1.2;
    text-align: center;
  }
  .chat .icon {
    font-size: 15px;
  }
  .body {
    min-width: 0;
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    align-items: baseline;
  }
  .what {
    color: var(--tone, var(--text-2));
    font-weight: 550;
  }
  .text {
    margin-top: 2px;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .chat .text {
    color: var(--text);
    margin: 0;
  }
  .meta {
    display: flex;
    gap: 10px;
    align-items: center;
    font-size: 12px;
    white-space: nowrap;
  }
  .badge {
    padding: 0 7px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-2);
    border: 1px solid var(--border);
  }
  .empty {
    text-align: center;
    padding: 48px 20px;
  }
  .empty p {
    margin: 4px auto;
    max-width: 440px;
  }
  .empty-icon {
    font-size: 34px;
    margin-bottom: 6px;
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
  @media (max-width: 760px) {
    .head {
      flex-direction: column;
    }
    .menu {
      left: 0;
      right: auto;
    }
    .item {
      grid-template-columns: 28px 1fr;
    }
    .meta {
      grid-column: 2;
    }
  }
</style>
