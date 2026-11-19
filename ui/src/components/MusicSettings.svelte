<script lang="ts">
  import { onMount } from "svelte";
  import { store, type Category, type MusicSettings, type Policy } from "../lib/api.svelte";

  let s = $state<MusicSettings | null>(null);
  let hateWords = $state("");
  let matWords = $state("");
  let allowWords = $state("");
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
  });

  async function save() {
    if (!s) return;
    saving = true;
    error = "";
    try {
      s.custom_rules = [...fromText(hateWords, "hate"), ...fromText(matWords, "profanity")];
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
          >Награда за баллы (название)
          <input class="input" placeholder="например: Заказать трек" bind:value={s.reward_title} />
        </label>
        <label
          >Донат от суммы (0 — выключено)
          <input class="input" type="number" min="0" step="10" bind:value={s.donation_min_amount} />
        </label>
      </div>
      <p class="hint">Для награды текст зрителя — запрос трека. Для доната — ссылка из сообщения или весь текст.</p>
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
</style>
