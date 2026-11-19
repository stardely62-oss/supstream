import type { ConnState, EventType, StreamEvent } from "./api.svelte";

export const EVENT_META: Record<EventType, { icon: string; label: string; tone: string }> = {
  donation: { icon: "💸", label: "Донат", tone: "money" },
  redemption: { icon: "🎁", label: "Награда", tone: "points" },
  chat_message: { icon: "💬", label: "Чат", tone: "muted" },
  follow: { icon: "❤️", label: "Фоллоу", tone: "love" },
  subscription: { icon: "⭐", label: "Подписка", tone: "sub" },
  gift_subs: { icon: "🎉", label: "Подарочные сабки", tone: "sub" },
  cheer: { icon: "💎", label: "Битсы", tone: "points" },
  raid: { icon: "🚀", label: "Рейд", tone: "love" },
};

const tier = (t: string) => ({ "1000": "1", "2000": "2", "3000": "3" })[t] ?? t;

function money(amount: number, currency: string) {
  try {
    return new Intl.NumberFormat("ru-RU", { style: "currency", currency, maximumFractionDigits: 2 }).format(amount);
  } catch {
    return `${amount} ${currency}`;
  }
}

/** Заголовок и подробности события для ленты. */
export function describe(e: StreamEvent): { who: string; what: string; text?: string } {
  const d = e.data;
  switch (e.type) {
    case "donation":
      return { who: d.donor, what: money(d.amount, d.currency), text: d.message };
    case "redemption":
      return { who: d.user_name, what: `${d.reward_title} · ${d.cost.toLocaleString("ru-RU")} баллов`, text: d.user_input };
    case "chat_message":
      return { who: d.user_name, what: "", text: d.text };
    case "follow":
      return { who: d.user_name, what: "теперь фолловер" };
    case "subscription":
      return {
        who: d.user_name,
        what: `сабка ур. ${tier(d.tier)}${d.months ? ` · ${d.months} мес.` : ""}${d.is_gift ? " · подарок" : ""}`,
        text: d.message,
      };
    case "gift_subs":
      return { who: d.user_name ?? "Аноним", what: `дарит ${d.total} саб(ок) ур. ${tier(d.tier)}` };
    case "cheer":
      return { who: d.user_name ?? "Аноним", what: `${d.bits.toLocaleString("ru-RU")} битс`, text: d.message };
    case "raid":
      return { who: d.from_name, what: `рейд · ${d.viewers} зрителей` };
  }
}

export const SOURCE_LABEL = { twitch: "Twitch", donationalerts: "DonationAlerts", supstream: "Supstream" } as const;

export function connLabel(s: ConnState | undefined): { text: string; tone: "ok" | "warn" | "bad" | "idle" } {
  if (!s) return { text: "…", tone: "idle" };
  switch (s.state) {
    case "connected":
      return { text: s.account, tone: "ok" };
    case "connecting":
      return { text: "подключение…", tone: "warn" };
    case "not_configured":
      return { text: "не подключено", tone: "idle" };
    case "error":
      return { text: `ошибка, повтор через ${s.retry_in} с`, tone: "bad" };
  }
}

export function time(iso: string) {
  return new Date(iso).toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.append(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
}

/** 75.4 → «1:15». */
export function clock(sec: number | undefined | null) {
  if (sec === undefined || sec === null || !Number.isFinite(sec)) return "–:––";
  const s = Math.max(0, Math.floor(sec));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
