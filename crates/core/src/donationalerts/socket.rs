//! Донаты в реальном времени через Centrifugo DonationAlerts.
//!
//! Порядок: подключиться → отправить socket_connection_token → получить client id
//! → запросить у API токен канала `$alerts:donation_<id>` → подписаться.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::{Instant, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::app::AppState;
use crate::bus::ConnState;
use crate::events::{Donation, Event, EventKind, Source};

const PING_EVERY: Duration = Duration::from_secs(25);
const SILENCE_LIMIT: Duration = Duration::from_secs(90);

/// Centrifugo может прислать несколько JSON-сообщений в одном кадре через `\n`.
fn split_frames(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok())
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.replace(',', ".").parse().ok(),
        _ => None,
    }
}

/// Достаёт донат из публикации Centrifugo, если это она.
pub fn parse_publication(msg: &Value) -> Option<Donation> {
    let channel = msg.pointer("/result/channel")?.as_str()?;
    if !channel.starts_with("$alerts:donation_") {
        return None;
    }
    let d = msg.pointer("/result/data/data")?;
    let donor = d
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Аноним")
        .to_string();
    Some(Donation {
        donor,
        amount: d.get("amount").and_then(as_f64)?,
        currency: d.get("currency").and_then(Value::as_str).unwrap_or("RUB").to_string(),
        amount_in_user_currency: d.get("amount_in_user_currency").and_then(as_f64),
        message: d
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        external_id: d.get("id").map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }),
    })
}

#[derive(Deserialize)]
struct SubscribeResponse {
    channels: Vec<ChannelToken>,
}

#[derive(Deserialize)]
struct ChannelToken {
    channel: String,
    token: String,
}

pub async fn run(state: Arc<AppState>) -> Result<()> {
    let user = super::fetch_user(&state).await?;
    let ep = &state.da.endpoints;
    let (mut ws, _) = timeout(Duration::from_secs(15), connect_async(ep.centrifugo_ws.as_str()))
        .await
        .context("DonationAlerts: таймаут подключения")?
        .context("DonationAlerts: не удалось подключиться")?;

    ws.send(Message::text(
        json!({ "params": { "token": user.socket_connection_token }, "id": 1 }).to_string(),
    ))
    .await?;

    // Ждём ответ на connect — в нём client id для подписки.
    let client_id = timeout(Duration::from_secs(15), async {
        while let Some(msg) = ws.next().await {
            if let Message::Text(t) = msg? {
                for v in split_frames(&t) {
                    if v.get("id").and_then(Value::as_u64) == Some(1) {
                        if let Some(err) = v.get("error") {
                            bail!("DonationAlerts отклонил подключение: {err}");
                        }
                        return v
                            .pointer("/result/client")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| anyhow!("DonationAlerts: нет client id в ответе"));
                    }
                }
            }
        }
        bail!("DonationAlerts: соединение закрыто")
    })
    .await
    .context("DonationAlerts: нет ответа на подключение")??;

    let channel = format!("$alerts:donation_{}", user.id);
    let token = super::access_token(&state).await?;
    let resp = state
        .http
        .post(format!("{}/api/v1/centrifuge/subscribe", ep.base))
        .bearer_auth(token)
        .json(&json!({ "channels": [channel], "client": client_id }))
        .send()
        .await
        .context("DonationAlerts недоступен")?;
    if !resp.status().is_success() {
        bail!("DonationAlerts: не удалось получить доступ к каналу донатов ({})", resp.status());
    }
    let sub: SubscribeResponse = resp.json().await?;
    let ch = sub.channels.into_iter().find(|c| c.channel == channel).context("нет токена канала")?;
    ws.send(Message::text(
        json!({ "params": { "channel": ch.channel, "token": ch.token }, "method": 1, "id": 2 }).to_string(),
    ))
    .await?;

    state.status.set_da(ConnState::Connected { account: user.name.clone() });
    tracing::info!("DonationAlerts: подключено ({})", user.name);

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut next_id: u64 = 3;
    let mut last_seen = Instant::now();

    loop {
        tokio::select! {
            _ = ping.tick() => {
                if last_seen.elapsed() > SILENCE_LIMIT {
                    bail!("DonationAlerts: сервер не отвечает, переподключаюсь");
                }
                // В протоколе Centrifugo v2 метод 7 — ping.
                ws.send(Message::text(json!({ "method": 7, "id": next_id }).to_string())).await?;
                next_id += 1;
            }
            msg = ws.next() => {
                let msg = msg.ok_or_else(|| anyhow!("DonationAlerts: соединение закрыто"))??;
                last_seen = Instant::now();
                match msg {
                    Message::Text(t) => {
                        for v in split_frames(&t) {
                            if v.get("id").and_then(Value::as_u64) == Some(2)
                                && let Some(err) = v.get("error") {
                                    bail!("DonationAlerts: подписка отклонена: {err}");
                                }
                            if let Some(d) = parse_publication(&v) {
                                state.bus.publish(Event::new(Source::DonationAlerts, EventKind::Donation(d)));
                            }
                        }
                    }
                    Message::Close(frame) => bail!("DonationAlerts закрыл соединение: {frame:?}"),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_donation_publication() {
        let raw = r#"{"result":{"channel":"$alerts:donation_12345","data":{"seq":7,"data":{"id":30489,"name":"donation","username":"Ivan","message_type":"text","message":"Привет! Сыграй трек","amount":500,"currency":"RUB","amount_in_user_currency":500.0,"is_shown":0,"created_at":"2026-11-18 20:00:00","shown_at":null}}}}"#;
        let frames: Vec<Value> = split_frames(raw).collect();
        let d = parse_publication(&frames[0]).unwrap();
        assert_eq!(d.donor, "Ivan");
        assert_eq!(d.amount, 500.0);
        assert_eq!(d.currency, "RUB");
        assert_eq!(d.message.as_deref(), Some("Привет! Сыграй трек"));
        assert_eq!(d.external_id.as_deref(), Some("30489"));
    }

    #[test]
    fn anonymous_and_string_amount() {
        let v = json!({"result":{"channel":"$alerts:donation_1","data":{"data":{"id":"x","username":null,"message":"","amount":"99.5","currency":"USD"}}}});
        let d = parse_publication(&v).unwrap();
        assert_eq!(d.donor, "Аноним");
        assert_eq!(d.amount, 99.5);
        assert!(d.message.is_none());
    }

    #[test]
    fn multiple_frames_and_non_publications() {
        let raw =
            "{\"id\":1,\"result\":{\"client\":\"abc\"}}\n{\"result\":{\"channel\":\"other\",\"data\":{}}}\n";
        let frames: Vec<Value> = split_frames(raw).collect();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| parse_publication(f).is_none()));
    }
}
