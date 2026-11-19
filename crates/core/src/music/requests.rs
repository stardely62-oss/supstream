//! Заказы музыки из событий: команда в чате, награда за баллы, донат.
//! Плюс ответы в чат Twitch о результате проверки.

use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;

use super::track::{ChatRole, Requester, Status, Via};
use crate::app::AppState;
use crate::events::{ChatMessage, Event, EventKind};

pub async fn run(state: Arc<AppState>) {
    let mut rx = state.bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(ev) => handle(&state, &ev),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        }
    }
}

fn role_of(m: &ChatMessage) -> ChatRole {
    let has = |b: &str| m.badges.iter().any(|x| x == b);
    if has("broadcaster") || has("moderator") {
        ChatRole::Moderator
    } else if has("vip") {
        ChatRole::Vip
    } else if has("subscriber") || has("founder") {
        ChatRole::Subscriber
    } else {
        ChatRole::Everyone
    }
}

/// `!sr запрос` → `Some("запрос")`; другая команда → `None`.
pub fn parse_command<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let text = text.trim();
    let cmd_len = command.chars().count();
    let head: String = text.chars().take(cmd_len).collect();
    if !head.eq_ignore_ascii_case(command) && head.to_lowercase() != command.to_lowercase() {
        return None;
    }
    let rest = &text[head.len()..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None; // «!srlol» — не наша команда
    }
    Some(rest.trim())
}

/// Ссылка из текста доната, иначе весь текст как поисковый запрос.
pub fn query_from_message(text: &str) -> String {
    text.split_whitespace()
        .find(|w| w.starts_with("http://") || w.starts_with("https://"))
        .map(str::to_string)
        .unwrap_or_else(|| text.trim().to_string())
}

fn handle(state: &Arc<AppState>, ev: &Event) {
    let cfg = state.store.config().music;
    if !cfg.enabled {
        return;
    }
    let (query, requester) = match &ev.kind {
        EventKind::ChatMessage(m) if cfg.chat_enabled => {
            if let Some(q) = parse_command(&m.text, "!song") {
                let _ = q;
                let now = state.music.view().now.map(|t| t.display_title());
                reply(
                    state,
                    format!(
                        "@{} {}",
                        m.user_name,
                        match now {
                            Some(t) => format!("сейчас играет: {t}"),
                            None => "сейчас ничего не играет".into(),
                        }
                    ),
                );
                return;
            }
            let Some(q) = parse_command(&m.text, &cfg.chat_command) else { return };
            if role_of(m) < cfg.chat_min_role {
                return;
            }
            if q.is_empty() {
                reply(
                    state,
                    format!("@{} напишите {} ссылка или название трека", m.user_name, cfg.chat_command),
                );
                return;
            }
            (
                q.to_string(),
                Requester {
                    name: m.user_name.clone(),
                    via: Via::Chat,
                    user_id: Some(m.user_id.clone()),
                    redemption: None,
                    amount: None,
                },
            )
        }
        EventKind::Redemption(r)
            if !cfg.reward_title.trim().is_empty()
                && r.reward_title.trim().to_lowercase() == cfg.reward_title.trim().to_lowercase() =>
        {
            let Some(q) = r.user_input.as_deref().filter(|s| !s.trim().is_empty()) else { return };
            (
                q.to_string(),
                Requester {
                    name: r.user_name.clone(),
                    via: Via::Reward,
                    user_id: Some(r.user_id.clone()),
                    redemption: Some((r.reward_id.clone(), r.redemption_id.clone())),
                    amount: Some(format!("{} баллов", r.cost)),
                },
            )
        }
        EventKind::Donation(d) if cfg.donation_min_amount > 0.0 => {
            let amount = d.amount_in_user_currency.unwrap_or(d.amount);
            let Some(msg) = d.message.as_deref().filter(|s| !s.trim().is_empty()) else { return };
            if amount < cfg.donation_min_amount {
                return;
            }
            (
                query_from_message(msg),
                Requester {
                    name: d.donor.clone(),
                    via: Via::Donation,
                    user_id: None,
                    redemption: None,
                    amount: Some(format!("{} {}", d.amount, d.currency)),
                },
            )
        }
        _ => return,
    };
    let via = requester.via;
    let name = requester.name.clone();
    if let Err(e) = super::request(state, &query, requester) {
        tracing::info!("музыка: заказ от {name} отклонён: {e:#}");
        if via == Via::Chat {
            reply(state, format!("@{name} {e:#}"));
        }
    }
}

fn reply(state: &Arc<AppState>, text: String) {
    if !state.store.config().music.chat_replies {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::twitch::send_chat(&state, &text).await {
            tracing::debug!("не удалось ответить в чат: {e:#}");
        }
    });
}

/// Сообщение в чат после проверки трека, заказанного из чата.
pub fn notify_result(state: &Arc<AppState>, id: &str) {
    let Some(t) = state.music.track(id) else { return };
    if t.requested_by.via != Via::Chat {
        return;
    }
    let who = &t.requested_by.name;
    let title = t.display_title();
    let text = match t.status {
        Status::Ready | Status::Playing => {
            let pos = state.music.view().queue.iter().position(|x| x.id == t.id).map(|p| p + 1);
            let muted = t.mutes.iter().filter(|m| m.enabled).count();
            let mut s = format!("@{who} добавлен: {title}");
            if let Some(p) = pos {
                s += &format!(" (#{p} в очереди)");
            }
            if muted > 0 {
                s += &format!(", заглушено мест: {muted}");
            }
            s
        }
        Status::NeedsReview => format!("@{who} {title} — ждёт проверки стримером"),
        Status::Rejected | Status::Failed => {
            let reason = t
                .error
                .clone()
                .or_else(|| t.check.as_ref().and_then(|c| c.reasons.last().cloned()))
                .unwrap_or_else(|| "не прошёл проверку".into());
            format!("@{who} {title} не добавлен: {reason}")
        }
        _ => return,
    };
    reply(state, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parsing() {
        assert_eq!(parse_command("!sr Numb", "!sr"), Some("Numb"));
        assert_eq!(parse_command("  !SR   linkin park numb ", "!sr"), Some("linkin park numb"));
        assert_eq!(parse_command("!sr", "!sr"), Some(""));
        assert_eq!(parse_command("!srlol", "!sr"), None);
        assert_eq!(parse_command("привет !sr x", "!sr"), None);
        assert_eq!(parse_command("!трек Кино", "!трек"), Some("Кино"));
        assert_eq!(parse_command("!ТРЕК Кино", "!трек"), Some("Кино"));
    }

    #[test]
    fn donation_query_prefers_link() {
        assert_eq!(query_from_message("врубай https://youtu.be/abc пж"), "https://youtu.be/abc");
        assert_eq!(query_from_message("  Кино Кукушка "), "Кино Кукушка");
    }
}
