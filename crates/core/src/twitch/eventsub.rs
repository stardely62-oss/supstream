//! EventSub через WebSocket: Twitch сам присылает события в открытое соединение,
//! поэтому публичный адрес и сервер не нужны — идеально для локального приложения.
//! Документация: https://dev.twitch.tv/docs/eventsub/handling-websocket-events/

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use crate::app::AppState;
use crate::bus::ConnState;
use crate::events::*;
use crate::store::TwitchTokens;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Подписки, которые оформляем при подключении: (тип, версия, условие).
pub fn subscription_specs(user_id: &str) -> Vec<(&'static str, &'static str, Value)> {
    let b = json!({ "broadcaster_user_id": user_id });
    vec![
        ("channel.chat.message", "1", json!({ "broadcaster_user_id": user_id, "user_id": user_id })),
        ("channel.channel_points_custom_reward_redemption.add", "1", b.clone()),
        ("channel.follow", "2", json!({ "broadcaster_user_id": user_id, "moderator_user_id": user_id })),
        ("channel.subscribe", "1", b.clone()),
        ("channel.subscription.message", "1", b.clone()),
        ("channel.subscription.gift", "1", b.clone()),
        ("channel.cheer", "1", b.clone()),
        ("channel.raid", "1", json!({ "to_broadcaster_user_id": user_id })),
    ]
}

#[derive(Debug, Deserialize)]
struct Envelope {
    metadata: Metadata,
    #[serde(default)]
    payload: Value,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    message_id: String,
    message_type: String,
    #[serde(default)]
    subscription_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionInfo {
    id: String,
    #[serde(default)]
    keepalive_timeout_seconds: Option<u64>,
    #[serde(default)]
    reconnect_url: Option<String>,
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn opt_s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string)
}

fn num(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or_default()
}

/// Переводит уведомление EventSub в наше событие. `None` — тип нам не интересен.
pub fn parse_notification(sub_type: &str, e: &Value) -> Option<EventKind> {
    Some(match sub_type {
        "channel.chat.message" => EventKind::ChatMessage(ChatMessage {
            message_id: s(e, "message_id"),
            user_id: s(e, "chatter_user_id"),
            user_login: s(e, "chatter_user_login"),
            user_name: s(e, "chatter_user_name"),
            text: e.pointer("/message/text").and_then(Value::as_str).unwrap_or_default().into(),
            color: opt_s(e, "color"),
            badges: e
                .get("badges")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|b| s(b, "set_id")).collect())
                .unwrap_or_default(),
        }),
        "channel.channel_points_custom_reward_redemption.add" => {
            let reward = e.get("reward").cloned().unwrap_or_default();
            EventKind::Redemption(Redemption {
                redemption_id: s(e, "id"),
                reward_id: s(&reward, "id"),
                reward_title: s(&reward, "title"),
                cost: num(&reward, "cost"),
                user_id: s(e, "user_id"),
                user_login: s(e, "user_login"),
                user_name: s(e, "user_name"),
                user_input: opt_s(e, "user_input"),
                status: s(e, "status"),
            })
        }
        "channel.follow" => EventKind::Follow(Follow {
            user_id: s(e, "user_id"),
            user_login: s(e, "user_login"),
            user_name: s(e, "user_name"),
        }),
        "channel.subscribe" => EventKind::Subscription(Subscription {
            user_id: s(e, "user_id"),
            user_login: s(e, "user_login"),
            user_name: s(e, "user_name"),
            tier: s(e, "tier"),
            months: None,
            message: None,
            is_gift: e.get("is_gift").and_then(Value::as_bool).unwrap_or(false),
        }),
        "channel.subscription.message" => EventKind::Subscription(Subscription {
            user_id: s(e, "user_id"),
            user_login: s(e, "user_login"),
            user_name: s(e, "user_name"),
            tier: s(e, "tier"),
            months: e.get("cumulative_months").and_then(Value::as_u64).map(|m| m as u32),
            message: e
                .pointer("/message/text")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
            is_gift: false,
        }),
        "channel.subscription.gift" => EventKind::GiftSubs(GiftSubs {
            user_name: if e.get("is_anonymous").and_then(Value::as_bool).unwrap_or(false) {
                None
            } else {
                opt_s(e, "user_name")
            },
            total: num(e, "total") as u32,
            tier: s(e, "tier"),
        }),
        "channel.cheer" => EventKind::Cheer(Cheer {
            user_name: if e.get("is_anonymous").and_then(Value::as_bool).unwrap_or(false) {
                None
            } else {
                opt_s(e, "user_name")
            },
            bits: num(e, "bits") as u32,
            message: s(e, "message"),
        }),
        "channel.raid" => EventKind::Raid(Raid {
            from_login: s(e, "from_broadcaster_user_login"),
            from_name: s(e, "from_broadcaster_user_name"),
            viewers: num(e, "viewers") as u32,
        }),
        _ => return None,
    })
}

async fn connect_and_welcome(url: &str) -> Result<(Ws, SessionInfo)> {
    let (mut ws, _) = timeout(Duration::from_secs(15), connect_async(url))
        .await
        .context("Twitch EventSub: таймаут подключения")?
        .context("Twitch EventSub: не удалось подключиться")?;
    let deadline = Duration::from_secs(15);
    loop {
        let msg = timeout(deadline, ws.next())
            .await
            .context("Twitch EventSub: не пришло приветствие")?
            .ok_or_else(|| anyhow!("Twitch EventSub: соединение закрыто"))??;
        if let Message::Text(text) = msg {
            let env: Envelope = serde_json::from_str(&text)?;
            if env.metadata.message_type == "session_welcome" {
                let session: SessionInfo = serde_json::from_value(env.payload["session"].clone())?;
                return Ok((ws, session));
            }
        }
    }
}

async fn create_subscriptions(state: &Arc<AppState>, tokens: &TwitchTokens, session_id: &str) -> Result<()> {
    let mut ok = 0;
    for (kind, version, condition) in subscription_specs(&tokens.user_id) {
        let body = json!({
            "type": kind,
            "version": version,
            "condition": condition,
            "transport": { "method": "websocket", "session_id": session_id },
        });
        let resp = super::helix(state, reqwest::Method::POST, "/eventsub/subscriptions", Some(body)).await?;
        let status = resp.status();
        if status.is_success() || status == reqwest::StatusCode::CONFLICT {
            ok += 1;
        } else {
            // 403 обычно значит: нет прав или канал не компаньон (баллы, сабки, битсы).
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!("Twitch: подписка {kind} не оформлена ({status}): {text}");
        }
    }
    if ok == 0 {
        bail!("Twitch не принял ни одной подписки на события");
    }
    Ok(())
}

/// Один «сеанс» EventSub. Возвращается только с ошибкой — супервизор переподключит.
pub async fn run(state: Arc<AppState>, tokens: TwitchTokens) -> Result<()> {
    let ep = &state.twitch.endpoints;
    let (mut ws, session) = connect_and_welcome(&ep.eventsub_ws).await?;
    let mut keepalive = Duration::from_secs(session.keepalive_timeout_seconds.unwrap_or(10) + 10);
    create_subscriptions(&state, &tokens, &session.id).await?;
    state.status.set_twitch(ConnState::Connected { account: tokens.login.clone() });
    tracing::info!("Twitch: подключено как {}", tokens.login);

    // Twitch может прислать одно и то же сообщение повторно.
    let mut seen: VecDeque<String> = VecDeque::with_capacity(256);
    // Twitch требует проверять токен раз в час.
    let mut validate_tick = tokio::time::interval(Duration::from_secs(3600));
    validate_tick.tick().await;

    loop {
        let next = tokio::select! {
            r = timeout(keepalive, ws.next()) => r,
            _ = validate_tick.tick() => {
                super::ensure_token(&state).await?;
                continue;
            }
        };
        let msg = next
            .context("Twitch EventSub: нет сигналов от сервера, переподключаюсь")?
            .ok_or_else(|| anyhow!("Twitch EventSub: соединение закрыто"))??;
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(frame) => bail!("Twitch EventSub закрыл соединение: {frame:?}"),
            _ => continue,
        };
        let env: Envelope = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Twitch EventSub: непонятное сообщение: {e}");
                continue;
            }
        };
        if seen.contains(&env.metadata.message_id) {
            continue;
        }
        if seen.len() == 256 {
            seen.pop_front();
        }
        seen.push_back(env.metadata.message_id.clone());

        match env.metadata.message_type.as_str() {
            "notification" => {
                let sub_type = env.metadata.subscription_type.unwrap_or_default();
                if let Some(kind) = parse_notification(&sub_type, &env.payload["event"]) {
                    state.bus.publish(Event::new(Source::Twitch, kind));
                }
            }
            "session_reconnect" => {
                // Подписки переносятся сами. Сначала открываем новое соединение,
                // потом закрываем старое — так события не теряются.
                let session: SessionInfo = serde_json::from_value(env.payload["session"].clone())?;
                let url = session.reconnect_url.context("нет reconnect_url")?;
                tracing::info!("Twitch EventSub: плановое переподключение");
                let (new_ws, new_session) = connect_and_welcome(&url).await?;
                keepalive = Duration::from_secs(new_session.keepalive_timeout_seconds.unwrap_or(10) + 10);
                let mut old = std::mem::replace(&mut ws, new_ws);
                let _ = old.close(None).await;
            }
            "revocation" => {
                let t = env.payload.pointer("/subscription/type").and_then(Value::as_str);
                tracing::warn!("Twitch отозвал подписку {t:?}");
            }
            _ => {} // session_keepalive — просто сбрасывает таймер
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_message() {
        let e = json!({
            "broadcaster_user_id": "1971641",
            "chatter_user_id": "4145994",
            "chatter_user_login": "viewer32",
            "chatter_user_name": "viewer32",
            "message_id": "cc106a89-1814-919d-454c-f4f2f970aae7",
            "message": { "text": "Hi chat", "fragments": [] },
            "color": "#00FF7F",
            "badges": [
                { "set_id": "moderator", "id": "1", "info": "" },
                { "set_id": "subscriber", "id": "12", "info": "16" }
            ],
            "message_type": "text"
        });
        let EventKind::ChatMessage(m) = parse_notification("channel.chat.message", &e).unwrap() else {
            panic!()
        };
        assert_eq!(m.text, "Hi chat");
        assert_eq!(m.user_name, "viewer32");
        assert_eq!(m.badges, vec!["moderator", "subscriber"]);
        assert_eq!(m.color.as_deref(), Some("#00FF7F"));
    }

    #[test]
    fn parses_redemption() {
        let e = json!({
            "id": "17fa2df1-ad76-4804-bfa5-a40ef63efe63",
            "broadcaster_user_id": "1337",
            "user_id": "9001",
            "user_login": "cooler_user",
            "user_name": "Cooler_User",
            "user_input": "pogchamp",
            "status": "unfulfilled",
            "reward": { "id": "92af127c", "title": "title", "cost": 100, "prompt": "reward prompt" },
            "redeemed_at": "2020-07-15T17:16:03.17106713Z"
        });
        let EventKind::Redemption(r) =
            parse_notification("channel.channel_points_custom_reward_redemption.add", &e).unwrap()
        else {
            panic!()
        };
        assert_eq!(r.cost, 100);
        assert_eq!(r.reward_title, "title");
        assert_eq!(r.user_input.as_deref(), Some("pogchamp"));
    }

    #[test]
    fn parses_anonymous_gift_and_cheer_and_raid() {
        let gift =
            json!({ "user_id": null, "user_name": null, "total": 2, "tier": "1000", "is_anonymous": true });
        let EventKind::GiftSubs(g) = parse_notification("channel.subscription.gift", &gift).unwrap() else {
            panic!()
        };
        assert_eq!(g.total, 2);
        assert!(g.user_name.is_none());

        let cheer =
            json!({ "is_anonymous": false, "user_name": "Cool", "message": "pogchamp", "bits": 1000 });
        let EventKind::Cheer(c) = parse_notification("channel.cheer", &cheer).unwrap() else { panic!() };
        assert_eq!(c.bits, 1000);
        assert_eq!(c.user_name.as_deref(), Some("Cool"));

        let raid =
            json!({ "from_broadcaster_user_login": "a", "from_broadcaster_user_name": "A", "viewers": 9001 });
        let EventKind::Raid(r) = parse_notification("channel.raid", &raid).unwrap() else { panic!() };
        assert_eq!(r.viewers, 9001);
    }

    #[test]
    fn resub_message_has_months_and_text() {
        let e = json!({
            "user_id": "1", "user_login": "a", "user_name": "A", "tier": "1000",
            "message": { "text": "Love the stream!", "emotes": [] },
            "cumulative_months": 15, "streak_months": 1, "duration_months": 6
        });
        let EventKind::Subscription(s) = parse_notification("channel.subscription.message", &e).unwrap()
        else {
            panic!()
        };
        assert_eq!(s.months, Some(15));
        assert_eq!(s.message.as_deref(), Some("Love the stream!"));
    }

    #[test]
    fn unknown_type_is_ignored() {
        assert!(parse_notification("channel.update", &json!({})).is_none());
    }
}
