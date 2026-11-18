<script lang="ts">
  import { onMount } from "svelte";
  import { store, type Settings } from "../lib/api.svelte";
  import { connLabel, copy } from "../lib/format";

  let settings = $state<Settings | null>(null);
  let busy = $state<string | null>(null);
  let error = $state<Record<string, string>>({});
  let copied = $state(false);
  let now = $state(Date.now());

  let twitchClientId = $state("");
  let daClientId = $state("");
  let daSecret = $state("");

  const twitch = $derived(store.connections?.twitch);
  const da = $derived(store.connections?.donationalerts);
  const deviceLogin = $derived(store.connections?.twitch_device_login);
  const secondsLeft = $derived(
    deviceLogin ? Math.max(0, Math.round((new Date(deviceLogin.expires_at).getTime() - now) / 1000)) : 0,
  );

  async function load() {
    settings = await store.get<Settings>("/settings");
    twitchClientId = settings.twitch.client_id;
    daClientId = settings.donationalerts.client_id;
  }

  // Статус приходит по WebSocket; при смене подключения перечитываем настройки
  // (логин, недостающие права).
  $effect(() => {
    twitch?.state;
    da?.state;
    load().catch(() => {});
  });

  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  async function run(key: string, fn: () => Promise<unknown>) {
    busy = key;
    error = { ...error, [key]: "" };
    try {
      await fn();
      await load();
    } catch (e) {
      error = { ...error, [key]: (e as Error).message };
    } finally {
      busy = null;
    }
  }

  const saveTwitch = () => run("twitch-save", () => store.put("/settings", { twitch_client_id: twitchClientId }));
  const loginTwitch = () => run("twitch", () => store.post("/auth/twitch/login"));
  const logoutTwitch = () => run("twitch", () => store.post("/auth/twitch/logout"));
  const openActivate = () => store.post("/system/open", { target: "twitch_activate" });
  const saveDa = () =>
    run("da-save", () =>
      store.put("/settings", { da_client_id: daClientId, ...(daSecret ? { da_client_secret: daSecret } : {}) }).then(
        () => (daSecret = ""),
      ),
    );
  const loginDa = () => run("da", () => store.post("/auth/donationalerts/login", { open_browser: true }));
  const logoutDa = () => run("da", () => store.post("/auth/donationalerts/logout"));

  async function copyRedirect() {
    if (!settings) return;
    await copy(settings.donationalerts.redirect_uri);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<header class="head">
  <h1>Подключения</h1>
  <p class="muted">Площадки, с которых Supstream получает события. Ключи хранятся только на этом ПК.</p>
</header>

<div class="grid">
  <!-- ───────── Twitch ───────── -->
  <section class="card platform">
    <div class="title">
      <span class="logo twitch">T</span>
      <div>
        <h2>Twitch</h2>
        <div class="state"><span class="dot {connLabel(twitch).tone}"></span>{connLabel(twitch).text}</div>
      </div>
    </div>

    {#if settings && !settings.twitch.configured}
      <ol class="steps">
        <li>
          Откройте <button class="link" onclick={() => store.post("/system/open", { target: "twitch_console" })}
            >Twitch Developer Console</button
          > и нажмите <b>Register Your Application</b> (нужна включённая 2FA).
        </li>
        <li>
          OAuth Redirect URL: <code>http://localhost</code>, категория <b>Application Integration</b>,
          <b>Client Type: Public</b>.
        </li>
        <li>Скопируйте <b>Client ID</b> сюда:</li>
      </ol>
      <div class="row">
        <input class="input mono" placeholder="Client ID" bind:value={twitchClientId} />
        <button class="btn primary" disabled={!twitchClientId.trim() || busy !== null} onclick={saveTwitch}>Сохранить</button>
      </div>
      {#if error["twitch-save"]}<p class="error-text">{error["twitch-save"]}</p>{/if}
    {:else if deviceLogin}
      <div class="device">
        <p class="muted">Подтвердите вход на Twitch. Страница уже открылась в браузере — проверьте, что код совпадает:</p>
        <div class="code mono">{deviceLogin.user_code}</div>
        <div class="row center">
          <button class="btn primary" onclick={openActivate}>Открыть страницу Twitch</button>
          <button class="btn ghost" onclick={logoutTwitch}>Отмена</button>
        </div>
        <p class="faint small">Код действует ещё {Math.floor(secondsLeft / 60)}:{String(secondsLeft % 60).padStart(2, "0")}</p>
      </div>
    {:else if twitch?.state === "connected"}
      <p class="muted">Получаем чат, награды за баллы, фолловы, сабки, битсы и рейды.</p>
      {#if settings?.twitch.missing_scopes.length}
        <p class="warn-box">
          Приложению нужны новые права: <code>{settings.twitch.missing_scopes.join(", ")}</code>. Войдите заново.
        </p>
      {/if}
      <div class="row">
        {#if settings?.twitch.missing_scopes.length}
          <button class="btn primary" disabled={busy !== null} onclick={loginTwitch}>Войти заново</button>
        {/if}
        <button class="btn" disabled={busy !== null} onclick={logoutTwitch}>Выйти</button>
      </div>
    {:else}
      {#if twitch?.state === "not_configured" && twitch.reason}<p class="muted">{twitch.reason}</p>{/if}
      {#if twitch?.state === "error"}<p class="error-text">{twitch.message}</p>{/if}
      <div class="row">
        <button class="btn primary" disabled={busy !== null} onclick={loginTwitch}>
          {busy === "twitch" ? "Запрашиваю код…" : "Войти через Twitch"}
        </button>
        {#if settings?.twitch.login}
          <button class="btn" disabled={busy !== null} onclick={logoutTwitch}>Выйти</button>
        {/if}
      </div>
    {/if}
    {#if error["twitch"]}<p class="error-text">{error["twitch"]}</p>{/if}

    {#if settings?.twitch.configured && !settings.twitch.has_builtin_client_id}
      <details class="advanced">
        <summary class="faint">Client ID приложения</summary>
        <div class="row">
          <input class="input mono" bind:value={twitchClientId} />
          <button class="btn" disabled={busy !== null} onclick={saveTwitch}>Сохранить</button>
        </div>
        <p class="faint small">При смене Client ID нужно будет войти заново.</p>
      </details>
    {/if}
  </section>

  <!-- ───────── DonationAlerts ───────── -->
  <section class="card platform">
    <div class="title">
      <span class="logo da">DA</span>
      <div>
        <h2>DonationAlerts</h2>
        <div class="state"><span class="dot {connLabel(da).tone}"></span>{connLabel(da).text}</div>
      </div>
    </div>

    {#if da?.state === "connected"}
      <p class="muted">Донаты приходят в реальном времени. Деньги идут напрямую через DonationAlerts.</p>
      <div class="row">
        <button class="btn" disabled={busy !== null} onclick={logoutDa}>Выйти</button>
      </div>
    {:else if settings}
      <ol class="steps">
        <li>
          Откройте <button class="link" onclick={() => store.post("/system/open", { target: "donationalerts_apps" })}
            >приложения DonationAlerts</button
          > и создайте новое.
        </li>
        <li>
          URL перенаправления:
          <span class="copyable">
            <code>{settings.donationalerts.redirect_uri}</code>
            <button class="btn small" onclick={copyRedirect}>{copied ? "Скопировано" : "Копировать"}</button>
          </span>
        </li>
        <li>Вставьте ID и API-ключ приложения:</li>
      </ol>
      <div class="fields">
        <input class="input mono" placeholder="ID приложения" bind:value={daClientId} />
        <input
          class="input mono"
          type="password"
          placeholder={settings.donationalerts.client_secret_set ? "API-ключ сохранён (••••••)" : "API-ключ"}
          bind:value={daSecret}
        />
        <button
          class="btn"
          disabled={busy !== null || !daClientId.trim() || (!daSecret && !settings.donationalerts.client_secret_set)}
          onclick={saveDa}>Сохранить</button
        >
      </div>
      {#if error["da-save"]}<p class="error-text">{error["da-save"]}</p>{/if}
      {#if da?.state === "error"}<p class="error-text">{da.message}</p>{/if}
      <div class="row">
        <button
          class="btn primary"
          disabled={busy !== null || !settings.donationalerts.client_id || !settings.donationalerts.client_secret_set}
          onclick={loginDa}>Войти через DonationAlerts</button
        >
      </div>
      <p class="faint small">Откроется браузер; после входа вернитесь сюда — статус обновится сам.</p>
    {/if}
    {#if error["da"]}<p class="error-text">{error["da"]}</p>{/if}
  </section>
</div>

<style>
  .head {
    margin-bottom: 20px;
  }
  .head p {
    margin: 4px 0 0;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
    gap: 16px;
    align-items: start;
  }
  .platform {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .platform p {
    margin: 0;
  }
  .title {
    display: flex;
    gap: 12px;
    align-items: center;
  }
  .logo {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    font-weight: 800;
    color: #fff;
  }
  .logo.twitch {
    background: #9146ff;
  }
  .logo.da {
    background: #f57d07;
    font-size: 13px;
  }
  h2 {
    font-size: 16px;
  }
  .state {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 13px;
    color: var(--text-2);
  }
  .steps {
    margin: 0;
    padding-left: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    color: var(--text-2);
  }
  .steps b {
    color: var(--text);
    font-weight: 600;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent-strong);
    text-decoration: underline;
    cursor: pointer;
  }
  .row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .row .input {
    flex: 1;
    min-width: 180px;
  }
  .row.center {
    justify-content: center;
  }
  .fields {
    display: grid;
    gap: 8px;
  }
  .fields .btn {
    justify-self: start;
  }
  .copyable {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-top: 4px;
  }
  .copyable code {
    padding: 3px 8px;
    border-radius: 6px;
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .device {
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .code {
    font-size: 30px;
    font-weight: 700;
    letter-spacing: 0.12em;
    padding: 14px;
    border-radius: var(--radius);
    background: var(--accent-soft);
    border: 1px dashed var(--accent);
  }
  .small {
    font-size: 12px;
  }
  .warn-box {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--warn) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--warn) 40%, transparent);
  }
  .advanced summary {
    cursor: pointer;
    font-size: 13px;
    margin-bottom: 8px;
  }
  .advanced p {
    margin-top: 6px;
  }
  @media (max-width: 760px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
