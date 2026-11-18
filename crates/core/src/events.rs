//! Единая модель событий. Всё, что приходит с площадок (Twitch, DonationAlerts,
//! позже YouTube/VK Play), приводится к этим типам, и дальше система работает
//! только с ними: лента, алерты, правила «событие → действие», внешнее API.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Twitch,
    #[serde(rename = "donationalerts")]
    DonationAlerts,
    /// События, созданные самим приложением (тесты, API, плагины).
    Supstream,
}

/// Событие в том виде, в каком оно уходит в ленту, виджеты и API.
///
/// JSON: `{ "id", "at", "source", "test", "type": "donation", "data": {..} }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub source: Source,
    /// Тестовое событие (кнопка «Тест» в панели) — правила его обрабатывают,
    /// но в статистику и отчёты оно не попадает.
    #[serde(default)]
    pub test: bool,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn new(source: Source, kind: EventKind) -> Self {
        Self { id: Uuid::new_v4(), at: Utc::now(), source, test: false, kind }
    }

    pub fn test(kind: EventKind) -> Self {
        Self { test: true, ..Self::new(Source::Supstream, kind) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum EventKind {
    Donation(Donation),
    Redemption(Redemption),
    ChatMessage(ChatMessage),
    Follow(Follow),
    Subscription(Subscription),
    GiftSubs(GiftSubs),
    Cheer(Cheer),
    Raid(Raid),
}

impl EventKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Donation(_) => "donation",
            Self::Redemption(_) => "redemption",
            Self::ChatMessage(_) => "chat_message",
            Self::Follow(_) => "follow",
            Self::Subscription(_) => "subscription",
            Self::GiftSubs(_) => "gift_subs",
            Self::Cheer(_) => "cheer",
            Self::Raid(_) => "raid",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Donation {
    /// Имя донатера (у анонимов — «Аноним»).
    pub donor: String,
    pub amount: f64,
    pub currency: String,
    /// Сумма в валюте стримера, если площадка её сообщает.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_in_user_currency: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// ID доната на площадке — нужен, чтобы не обработать один донат дважды.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Redemption {
    pub redemption_id: String,
    pub reward_id: String,
    pub reward_title: String,
    pub cost: u64,
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_input: Option<String>,
    /// unfulfilled / fulfilled / canceled
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub message_id: String,
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// broadcaster, moderator, vip, subscriber, …
    #[serde(default)]
    pub badges: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Follow {
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    /// 1000 / 2000 / 3000 (уровни Twitch).
    pub tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub months: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub is_gift: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiftSubs {
    /// `None` — анонимный подарок.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    pub total: u32,
    pub tier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cheer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    pub bits: u32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Raid {
    pub from_login: String,
    pub from_name: String,
    pub viewers: u32,
}

/// Образцы событий для кнопок «Тест» в панели.
pub fn sample(type_name: &str) -> Option<EventKind> {
    Some(match type_name {
        "donation" => EventKind::Donation(Donation {
            donor: "ТестовыйДонатер".into(),
            amount: 300.0,
            currency: "RUB".into(),
            amount_in_user_currency: Some(300.0),
            message: Some("Это тестовый донат! Привет из Supstream 👋".into()),
            external_id: None,
        }),
        "redemption" => EventKind::Redemption(Redemption {
            redemption_id: "test".into(),
            reward_id: "test".into(),
            reward_title: "Перевернуть камеру".into(),
            cost: 1000,
            user_id: "0".into(),
            user_login: "test_viewer".into(),
            user_name: "TestViewer".into(),
            user_input: None,
            status: "unfulfilled".into(),
        }),
        "chat_message" => EventKind::ChatMessage(ChatMessage {
            message_id: "test".into(),
            user_id: "0".into(),
            user_login: "test_viewer".into(),
            user_name: "TestViewer".into(),
            text: "Тестовое сообщение в чат".into(),
            color: Some("#9146FF".into()),
            badges: vec!["subscriber".into()],
        }),
        "follow" => EventKind::Follow(Follow {
            user_id: "0".into(),
            user_login: "new_follower".into(),
            user_name: "NewFollower".into(),
        }),
        "subscription" => EventKind::Subscription(Subscription {
            user_id: "0".into(),
            user_login: "loyal_viewer".into(),
            user_name: "LoyalViewer".into(),
            tier: "1000".into(),
            months: Some(6),
            message: Some("Полгода с вами!".into()),
            is_gift: false,
        }),
        "gift_subs" => EventKind::GiftSubs(GiftSubs {
            user_name: Some("GenerousViewer".into()),
            total: 5,
            tier: "1000".into(),
        }),
        "cheer" => EventKind::Cheer(Cheer {
            user_name: Some("BitsFan".into()),
            bits: 500,
            message: "Cheer500 держи битсы".into(),
        }),
        "raid" => EventKind::Raid(Raid {
            from_login: "friend_streamer".into(),
            from_name: "FriendStreamer".into(),
            viewers: 42,
        }),
        _ => return None,
    })
}

pub const SAMPLE_TYPES: &[&str] =
    &["donation", "redemption", "chat_message", "follow", "subscription", "gift_subs", "cheer", "raid"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_is_flat_and_roundtrips() {
        for t in SAMPLE_TYPES {
            let ev = Event::test(sample(t).unwrap());
            let json = serde_json::to_value(&ev).unwrap();
            assert_eq!(json["type"], *t);
            assert!(json["data"].is_object());
            assert_eq!(json["source"], "supstream");
            let back: Event = serde_json::from_value(json).unwrap();
            assert_eq!(back.kind.type_name(), *t);
            assert!(back.test);
        }
    }
}
