<script lang="ts">
  import { onMount } from "svelte";
  import { store, type Settings } from "../lib/api.svelte";
  import { copy } from "../lib/format";

  let settings = $state<Settings | null>(null);
  let showToken = $state(false);
  let copiedKey = $state("");

  onMount(async () => {
    settings = await store.get<Settings>("/settings");
  });

  const base = $derived(`http://127.0.0.1:${settings?.server.port ?? 7272}`);
  const token = $derived(store.token ?? "");

  async function copyText(key: string, text: string) {
    await copy(text);
    copiedKey = key;
    setTimeout(() => (copiedKey = ""), 1500);
  }

  const examples = $derived([
    {
      key: "ws",
      title: "Поток событий (WebSocket)",
      note: "Виджеты, скрипты, моды к играм — подписка на все события в реальном времени.",
      code: `${base.replace("http", "ws")}/api/ws?token=<ключ>`,
    },
    {
      key: "test",
      title: "Тестовое событие",
      note: "Типы: donation, redemption, chat_message, follow, subscription, gift_subs, cheer, raid.",
      code: `curl -X POST ${base}/api/events/test \\\n  -H "Authorization: Bearer <ключ>" \\\n  -H "Content-Type: application/json" \\\n  -d '{"type":"donation"}'`,
    },
    {
      key: "publish",
      title: "Своё событие",
      note: "Событие от вашего скрипта попадёт в ленту и (скоро) в правила «событие → действие».",
      code: `curl -X POST ${base}/api/events \\\n  -H "Authorization: Bearer <ключ>" \\\n  -H "Content-Type: application/json" \\\n  -d '{"type":"follow","data":{"user_id":"1","user_login":"bot","user_name":"Bot"}}'`,
    },
  ]);
</script>

<header class="head">
  <h1>Настройки</h1>
  <p class="muted">Общие параметры и доступ к API для своих скриптов и виджетов.</p>
</header>

{#if settings}
  <div class="stack">
    <section class="card">
      <h2>Приложение</h2>
      <dl>
        <dt>Адрес панели</dt>
        <dd class="mono">{base}</dd>
        <dt>Доступ из сети</dt>
        <dd>{settings.server.allow_lan ? "открыт для локальной сети" : "только этот ПК"}</dd>
        <dt>Папка данных</dt>
        <dd class="dir">
          <span class="mono">{settings.data_dir}</span>
          <button class="btn small" onclick={() => store.post("/system/open", { target: "data_dir" })}>Открыть</button>
        </dd>
      </dl>
      <p class="faint small">Порт и доступ из сети пока меняются в <code>config.json</code>, нужен перезапуск.</p>
    </section>

    <section class="card">
      <h2>Ключ доступа к API</h2>
      <p class="muted">
        Нужен скриптам, виджетам и панели с телефона. Никому его не показывайте — с ним можно управлять приложением.
      </p>
      <div class="token">
        <input class="input mono" readonly type={showToken ? "text" : "password"} value={token} />
        <button class="btn" onclick={() => (showToken = !showToken)}>{showToken ? "Скрыть" : "Показать"}</button>
        <button class="btn" onclick={() => copyText("token", token)}>{copiedKey === "token" ? "Скопировано" : "Копировать"}</button>
      </div>
    </section>

    <section class="card">
      <h2>API</h2>
      <p class="muted">
        Ключ передаётся заголовком <code>Authorization: Bearer &lt;ключ&gt;</code> или параметром <code>?token=</code>.
      </p>
      {#each examples as ex}
        <div class="example">
          <div class="ex-head">
            <h3>{ex.title}</h3>
            <button class="btn small ghost" onclick={() => copyText(ex.key, ex.code.replaceAll("<ключ>", token))}>
              {copiedKey === ex.key ? "Скопировано" : "Копировать с ключом"}
            </button>
          </div>
          <p class="faint small">{ex.note}</p>
          <pre class="mono">{ex.code}</pre>
        </div>
      {/each}
    </section>
  </div>
{:else}
  <p class="faint">Загрузка…</p>
{/if}

<style>
  .head {
    margin-bottom: 20px;
  }
  .head p {
    margin: 4px 0 0;
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 860px;
  }
  h2 {
    font-size: 16px;
    margin-bottom: 10px;
  }
  h3 {
    font-size: 14px;
  }
  .card p {
    margin: 0 0 12px;
  }
  dl {
    display: grid;
    grid-template-columns: 160px 1fr;
    gap: 10px 16px;
    margin: 0 0 12px;
  }
  dt {
    color: var(--text-2);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .dir {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  .token {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .token .input {
    flex: 1;
    min-width: 200px;
  }
  .example {
    padding-top: 14px;
    margin-top: 14px;
    border-top: 1px solid var(--border);
  }
  .example p {
    margin: 2px 0 8px;
  }
  .ex-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }
  pre {
    margin: 0;
    padding: 12px;
    border-radius: var(--radius-sm);
    background: var(--bg);
    border: 1px solid var(--border);
    overflow-x: auto;
    white-space: pre;
  }
  .small {
    font-size: 12px;
  }
  @media (max-width: 760px) {
    dl {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    dd {
      margin-bottom: 8px;
    }
  }
</style>
