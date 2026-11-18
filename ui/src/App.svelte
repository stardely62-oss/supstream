<script lang="ts">
  import { onMount } from "svelte";
  import { store } from "./lib/api.svelte";
  import { connLabel } from "./lib/format";
  import Feed from "./pages/Feed.svelte";
  import Connections from "./pages/Connections.svelte";
  import Settings from "./pages/Settings.svelte";

  type Page = "feed" | "connections" | "settings";

  const NAV: { id: Page | string; label: string; icon: string; soon?: boolean }[] = [
    { id: "feed", label: "Лента", icon: "⚡" },
    { id: "connections", label: "Подключения", icon: "🔌" },
    { id: "music", label: "Музыка", icon: "🎵", soon: true },
    { id: "actions", label: "Действия", icon: "🧩", soon: true },
    { id: "alerts", label: "Алерты", icon: "🔔", soon: true },
    { id: "settings", label: "Настройки", icon: "⚙️" },
  ];

  function initialPage(): Page {
    const h = location.hash.slice(1);
    return h === "connections" || h === "settings" ? h : "feed";
  }

  let page = $state<Page>(initialPage());
  let tokenInput = $state("");

  function go(p: Page) {
    page = p;
    history.replaceState(null, "", `#${p}`);
  }

  const twitch = $derived(connLabel(store.connections?.twitch));
  const da = $derived(connLabel(store.connections?.donationalerts));

  onMount(() => store.connect());
</script>

{#if !store.token || store.unauthorized}
  <main class="gate">
    <div class="card gate-card">
      <div class="logo big">Supstream</div>
      <p class="muted">
        {store.unauthorized ? "Ключ доступа не подошёл." : "Панель нужно открыть из приложения Supstream."}
        Либо вставьте ключ доступа — он есть в <code>config.json</code> (поле <code>api_token</code>).
      </p>
      <form
        class="row"
        onsubmit={(e) => {
          e.preventDefault();
          if (tokenInput.trim()) store.setToken(tokenInput);
        }}
      >
        <input class="input mono" placeholder="Ключ доступа" bind:value={tokenInput} />
        <button class="btn primary" type="submit">Войти</button>
      </form>
    </div>
  </main>
{:else}
  <div class="shell">
    <aside class="sidebar">
      <div class="logo">Supstream</div>
      <nav>
        {#each NAV as item}
          <button
            class="nav-item"
            class:active={page === item.id}
            disabled={item.soon}
            onclick={() => !item.soon && go(item.id as Page)}
          >
            <span class="nav-icon">{item.icon}</span>
            <span class="nav-label">{item.label}</span>
            {#if item.soon}<span class="soon">скоро</span>{/if}
          </button>
        {/each}
      </nav>
      <div class="side-status">
        <button class="conn" onclick={() => go("connections")}>
          <span class="dot {twitch.tone}"></span>
          <span class="conn-name">Twitch</span>
          <span class="conn-state">{twitch.text}</span>
        </button>
        <button class="conn" onclick={() => go("connections")}>
          <span class="dot {da.tone}"></span>
          <span class="conn-name">DonationAlerts</span>
          <span class="conn-state">{da.text}</span>
        </button>
        <div class="version faint">
          {#if store.online}v{store.version}{:else}<span class="error-text">нет связи с ядром…</span>{/if}
        </div>
      </div>
    </aside>

    <main class="content">
      {#if page === "feed"}
        <Feed />
      {:else if page === "connections"}
        <Connections />
      {:else}
        <Settings />
      {/if}
    </main>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 232px 1fr;
    min-height: 100vh;
  }
  .sidebar {
    position: sticky;
    top: 0;
    height: 100vh;
    display: flex;
    flex-direction: column;
    gap: 20px;
    padding: 20px 12px;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }
  .logo {
    padding: 0 10px;
    font-weight: 750;
    font-size: 18px;
    letter-spacing: -0.02em;
    background: linear-gradient(90deg, var(--accent-strong), var(--love));
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .logo.big {
    font-size: 26px;
    padding: 0;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 38px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-2);
    cursor: pointer;
    text-align: left;
  }
  .nav-item:hover:not(:disabled) {
    background: var(--surface-2);
    color: var(--text);
  }
  .nav-item.active {
    background: var(--accent-soft);
    color: var(--text);
  }
  .nav-item:disabled {
    cursor: default;
    opacity: 0.55;
  }
  .nav-icon {
    width: 20px;
    text-align: center;
  }
  .nav-label {
    flex: 1;
  }
  .soon {
    font-size: 11px;
    color: var(--text-3);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 7px;
  }
  .side-status {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .conn {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 10px;
    align-items: center;
    padding: 8px 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    cursor: pointer;
    text-align: left;
  }
  .conn:hover {
    background: var(--surface-2);
  }
  .conn-name {
    font-size: 13px;
  }
  .conn-state {
    grid-column: 2;
    font-size: 12px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .version {
    padding: 6px 10px 0;
    font-size: 12px;
  }
  .content {
    min-width: 0;
    padding: 28px 32px 48px;
  }
  .gate {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: 16px;
  }
  .gate-card {
    width: min(460px, 100%);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .row {
    display: flex;
    gap: 8px;
  }

  /* Телефон: меню превращается в нижнюю панель. */
  @media (max-width: 760px) {
    .shell {
      grid-template-columns: 1fr;
    }
    .sidebar {
      position: fixed;
      inset: auto 0 0 0;
      height: auto;
      z-index: 10;
      flex-direction: row;
      padding: 6px;
      border-right: 0;
      border-top: 1px solid var(--border);
    }
    .logo,
    .side-status,
    .soon {
      display: none;
    }
    nav {
      flex-direction: row;
      width: 100%;
      justify-content: space-around;
    }
    .nav-item {
      flex-direction: column;
      height: auto;
      gap: 2px;
      padding: 6px 4px;
      font-size: 11px;
    }
    .nav-item:disabled {
      display: none;
    }
    .content {
      padding: 20px 16px 96px;
    }
  }
</style>
