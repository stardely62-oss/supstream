<script lang="ts">
  import { onMount } from "svelte";
  import { store, type Category, type MusicSettings, type Policy } from "../lib/api.svelte";

  let s = $state<MusicSettings | null>(null);
  let hateWords = $state("");
  let matWords = $state("");
  let allowWords = $state("");
  let localDirs = $state("");
  let channelUrl = $state("");
  let rewardTitle = $state("Заказать трек");
  let rewardCost = $state(1000);
  let busy = $state<string | null>(null);
  let actionError = $state<Record<string, string>>({});
  const twitchConnected = $derived(store.connections?.twitch.state === "connected");
  let saving = $state(false);
  let saved = $state(false);
  let error = $state("");

  const POLICY: Record<Policy, string> = {
    allow: "пропускать",
    mute: "глушить",
    review: "на мою проверку",
    reject: "отклонять",
  };

  /** «слово» — точно, «слово*» — начало слова, «*слово*» — где угодно в слове. */
  function toText(cat: Category) {
    return (s?.custom_rules ?? [])
      .filter((r) => r.category === cat)
      .map((r) => (r.kind === "contains" ? `*${r.pattern}*` : r.kind === "prefix" || r.kind === "root" ? `${r.pattern}*` : r.pattern))
      .join("\n");
  }

  function fromText(text: string, category: Category) {
    return text
      .split("\n")
      .map((l) => l.trim().toLowerCase())
      .filter(Boolean)
      .map((l) => {
        const contains = l.startsWith("*") && l.endsWith("*") && l.length > 2;
        const prefix = !contains && l.endsWith("*");
        const pattern = l.replace(/^\*+|\*+$/g, "");
        return { pattern, category, kind: contains ? "contains" : prefix ? "prefix" : "exact" } as const;
      })
      .filter((r) => r.pattern.length > 0);
  }

  onMount(async () => {
    s = await store.get<MusicSettings>("/music/settings");
    hateWords = toText("hate");
    matWords = toText("profanity");
    allowWords = s.allow_words.join("\n");
    localDirs = s.local_dirs.join("\n");
    if (s.reward_id) {
      rewardTitle = s.reward_title;
      rewardCost = s.reward_cost;
    }
  });

  /** Действия, которые сохраняются сразу (каналы, награда). */
  async function act(key: string, fn: () => Promise<MusicSettings>) {
    busy = key;
    actionError = { ...actionError, [key]: "" };
    try {
      const next = await fn();
      if (s) {
        s.safe_channels = next.safe_channels;
        s.reward_id = next.reward_id;
        s.reward_title = next.reward_title;
        s.reward_cost = next.reward_cost;
      }
      return true;
    } catch (e) {
      actionError = { ...actionError, [key]: (e as Error).message };
      return false;
    } finally {
      busy = null;
    }
  }

  async function addChannel() {
    if (await act("channel", () => store.post<MusicSettings>("/music/safe-channels", { url: channelUrl.trim() }))) channelUrl = "";
  }

  function removeChannel(id: string) {
    if (!s) return;
    s.safe_channels = s.safe_channels.filter((c) => c.channel_id !== id);
    void save();
  }

  const saveReward = () =>
    act("reward", () => store.put<MusicSettings>("/music/reward", { title: rewardTitle.trim(), cost: Number(rewardCost) }));
  const deleteReward = () => act("reward", () => store.request<MusicSettings>("DELETE", "/music/reward", {}));

  async function save() {
    if (!s) return;
    saving = true;
    error = "";
    try {
      s.custom_rules = [...fromText(hateWords, "hate"), ...fromText(matWords, "profanity")];
      s.local_dirs = localDirs
        .split("\n")
        .map((d) => d.trim())
        .filter(Boolean);
      s.allow_words = allowWords
        .split("\n")
        .map((w) => w.trim())
        .filter(Boolean);
      s = await store.put<MusicSettings>("/music/settings", s);
      saved = true;
      setTimeout(() => (saved = false), 1500);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }
</script>

{#if s}
  <div class="settings">
    <label class="switch">
      <input type="checkbox" bind:checked={s.enabled} /> Принимать заказы музыки
    </label>

    <fieldset>
      <legend>Источники музыки</legend>
      <label class="radio">
        <input type="radio" bind:group={s.source_mode} value="safe" />
        <span>
          <b>Только безопасная музыка</b>
          <span class="hint block">Разрешённые каналы и своя папка. Без риска жалоб правообладателей.</span>
        </span>
      </label>
      <label class="radio">
        <input type="radio" bind:group={s.source_mode} value="any" />
        <span>
          <b>Любая музыка</b>
          <span class="hint block">
            Всё с YouTube и SoundCloud. Twitch запрещает чужую музыку без лицензии: возможны жалобы (DMCA) на записи и
            клипы, три жалобы — бан канала.
          </span>
        </span>
      </label>

      <div class="sub">
        <h5>Разрешённые каналы</h5>
        {#if s.safe_channels.length === 0}
          <p class="hint">Пока пусто. Например: StreamBeats, NoCopyrightSounds, Monstercat (с подпиской Gold).</p>
        {/if}
        <ul class="channels">
          {#each s.safe_channels as c (c.channel_id)}
            <li>
              <span>{c.name}</span>
              <span class="hint mono">{c.channel_id}</span>
              <button class="btn small ghost" title="Убрать" onclick={() => removeChannel(c.channel_id)}>✕</button>
            </li>
          {/each}
        </ul>
        <form
          class="row"
          onsubmit={(e) => {
            e.preventDefault();
            void addChannel();
          }}
        >
          <input class="input" placeholder="Ссылка на канал YouTube или любое его видео" bind:value={channelUrl} />
          <button class="btn" type="submit" disabled={!channelUrl.trim() || busy !== null}>
            {busy === "channel" ? "Проверяю…" : "Добавить"}
          </button>
        </form>
        {#if actionError["channel"]}<p class="error-text">{actionError["channel"]}</p>{/if}
      </div>

      <label class="field"
        >Папки со своей музыкой — по одной в строке
        <textarea class="input area mono" placeholder="D:\Music\Stream" bind:value={localDirs}></textarea>
      </label>
      <p class="hint">
        Файлы вида «Исполнитель - Название.mp3» (mp3, ogg, opus, flac, wav). Ищутся первыми в любом режиме и не
        скачиваются.
      </p>
    </fieldset>

    <fieldset>
      <legend>Проверка текста</legend>
      <div class="grid">
        <label
          >Оскорбления и слюры
          <select bind:value={s.hate}>
            {#each ["mute", "review", "reject"] as p}<option value={p}>{POLICY[p as Policy]}</option>{/each}
          </select>
        </label>
        <label
          >Мат
          <select bind:value={s.profanity}>
            {#each ["allow", "mute", "review", "reject"] as p}<option value={p}>{POLICY[p as Policy]}</option>{/each}
          </select>
        </label>
        <label
          >Текст не найден
          <select bind:value={s.no_lyrics}>
            {#each ["allow", "review", "reject"] as p}<option value={p}>{POLICY[p as Policy]}</option>{/each}
          </select>
        </label>
        <label
          >Запас вокруг слова, сек
          <input class="input" type="number" min="0" max="3" step="0.1" bind:value={s.mute_padding} />
        </label>
      </div>
      <p class="hint">
        Оскорбления глушатся всегда, даже если трек одобрен вручную. Мат на Twitch разрешён, но с меткой контента —
        решайте сами.
      </p>
    </fieldset>

    <fieldset>
      <legend>Ограничения</legend>
      <div class="grid">
        <label
          >Максимальная длина, мин
          <input
            class="input"
            type="number"
            min="1"
            max="60"
            value={Math.round(s.max_duration_secs / 60)}
            oninput={(e) => s && (s.max_duration_secs = Number(e.currentTarget.value) * 60)}
          />
        </label>
        <label
          >Треков на зрителя в очереди
          <input class="input" type="number" min="1" max="20" bind:value={s.per_user_limit} />
        </label>
      </div>
    </fieldset>

    <fieldset>
      <legend>Откуда принимать заказы</legend>
      <label class="switch"><input type="checkbox" bind:checked={s.chat_enabled} /> Команда в чате Twitch</label>
      <div class="grid">
        <label
          >Команда
          <input class="input mono" bind:value={s.chat_command} />
        </label>
        <label
          >Кто может заказывать
          <select bind:value={s.chat_min_role}>
            <option value="everyone">все</option>
            <option value="subscriber">сабы и выше</option>
            <option value="vip">VIP и модераторы</option>
            <option value="moderator">только модераторы</option>
          </select>
        </label>
      </div>
      <label class="switch"><input type="checkbox" bind:checked={s.chat_replies} /> Отвечать в чат о результате</label>
      <div class="grid">
        <label
          >Донат от суммы (0 — выключено)
          <input class="input" type="number" min="0" step="10" bind:value={s.donation_min_amount} />
        </label>
      </div>
      <p class="hint">Для доната запрос — ссылка из сообщения или весь текст.</p>

      <div class="sub">
        <h5>Награда за баллы канала</h5>
        {#if s.reward_id}
          <p class="hint">
            Награда «{s.reward_title}» создана Supstream. Если трек не пройдёт проверку или вы его уберёте, Twitch вернёт
            зрителю баллы.
          </p>
        {:else}
          <p class="hint">
            Supstream создаст награду на Twitch сам — только для таких наград можно автоматически возвращать баллы.
          </p>
        {/if}
        <div class="row">
          <input class="input" placeholder="Название" maxlength="45" bind:value={rewardTitle} />
          <input class="input cost" type="number" min="1" bind:value={rewardCost} title="Цена в баллах" />
          <button class="btn" disabled={!twitchConnected || busy !== null || !rewardTitle.trim()} onclick={saveReward}>
            {busy === "reward" ? "…" : s.reward_id ? "Обновить" : "Создать на Twitch"}
          </button>
          {#if s.reward_id}
            <button class="btn ghost" disabled={busy !== null} onclick={deleteReward}>Удалить</button>
          {/if}
        </div>
        {#if !twitchConnected}<p class="hint">Сначала войдите в Twitch (Подключения).</p>{/if}
        {#if actionError["reward"]}<p class="error-text">{actionError["reward"]}</p>{/if}
        {#if !s.reward_id}
          <details>
            <summary class="hint">Или своя награда, созданная в панели Twitch (без возврата баллов)</summary>
            <input class="input" placeholder="Точное название награды" bind:value={s.reward_title} />
          </details>
        {/if}
      </div>
    </fieldset>

    <fieldset>
      <legend>Свои слова</legend>
      <p class="hint">
        По одному в строке. <code>слово</code> — точное совпадение, <code>слово*</code> — начало слова,
        <code>*слово*</code> — где угодно внутри слова.
      </p>
      <div class="grid three">
        <label>Оскорбления<textarea class="input area mono" bind:value={hateWords}></textarea></label>
        <label>Мат<textarea class="input area mono" bind:value={matWords}></textarea></label>
        <label>Исключения (не нарушение)<textarea class="input area mono" bind:value={allowWords}></textarea></label>
      </div>
    </fieldset>

    {#if error}<p class="error-text">{error}</p>{/if}
    <div>
      <button class="btn primary" disabled={saving} onclick={save}>{saved ? "Сохранено ✓" : "Сохранить"}</button>
    </div>
  </div>
{:else}
  <p class="faint">Загрузка…</p>
{/if}

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  fieldset {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 12px 14px 14px;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  legend {
    padding: 0 6px;
    font-weight: 600;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 10px 14px;
  }
  .grid label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
    color: var(--text-2);
  }
  .switch {
    display: flex;
    gap: 8px;
    align-items: center;
    cursor: pointer;
  }
  select {
    height: 36px;
    padding: 0 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    font: inherit;
  }
  .area {
    height: 110px;
    padding: 8px 10px;
    resize: vertical;
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-3);
  }
  .hint.block {
    display: block;
    margin-top: 2px;
  }
  .radio {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .radio input {
    margin-top: 4px;
  }
  .sub {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  h5 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .channels {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .channels li {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .channels li .mono {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .row .input {
    flex: 1;
    min-width: 160px;
  }
  .row .input.cost {
    flex: 0 0 110px;
    min-width: 0;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
    color: var(--text-2);
  }
  details .input {
    margin-top: 6px;
  }
</style>
