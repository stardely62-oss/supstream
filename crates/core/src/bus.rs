//! Шина событий и состояния подключений.
//!
//! Любой модуль публикует событие в шину, а все подписчики (лента в панели,
//! виджеты OBS, правила, внешние скрипты через API) получают его одновременно.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::{broadcast, watch};

use crate::events::Event;

const HISTORY_LIMIT: usize = 500;
const CHANNEL_CAPACITY: usize = 1024;

pub struct EventBus {
    tx: broadcast::Sender<Event>,
    history: Mutex<VecDeque<Event>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx, history: Mutex::new(VecDeque::with_capacity(HISTORY_LIMIT)) }
    }

    pub fn publish(&self, event: Event) {
        tracing::info!(
            target: "supstream::events",
            kind = event.kind.type_name(),
            source = ?event.source,
            test = event.test,
            "событие"
        );
        {
            let mut h = self.history.lock().unwrap();
            if h.len() == HISTORY_LIMIT {
                h.pop_front();
            }
            h.push_back(event.clone());
        }
        // Ошибка означает только «сейчас нет подписчиков» — это нормально.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Последние `limit` событий, от старых к новым.
    pub fn recent(&self, limit: usize) -> Vec<Event> {
        let h = self.history.lock().unwrap();
        let skip = h.len().saturating_sub(limit);
        h.iter().skip(skip).cloned().collect()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnState {
    /// Нет ключей или не выполнен вход.
    NotConfigured {
        reason: String,
    },
    Connecting,
    Connected {
        account: String,
    },
    /// Ошибка; будет повторная попытка через `retry_in` секунд.
    Error {
        message: String,
        retry_in: u64,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConnectionsStatus {
    pub twitch: ConnState,
    pub donationalerts: ConnState,
    /// Идёт вход в Twitch по коду устройства.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twitch_device_login: Option<DeviceLoginStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeviceLoginStatus {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct StatusHub {
    tx: watch::Sender<ConnectionsStatus>,
}

impl Default for StatusHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusHub {
    pub fn new() -> Self {
        let init = ConnectionsStatus {
            twitch: ConnState::Connecting,
            donationalerts: ConnState::Connecting,
            twitch_device_login: None,
        };
        Self { tx: watch::channel(init).0 }
    }

    pub fn get(&self) -> ConnectionsStatus {
        self.tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<ConnectionsStatus> {
        self.tx.subscribe()
    }

    pub fn update(&self, f: impl FnOnce(&mut ConnectionsStatus)) {
        self.tx.send_if_modified(|s| {
            let before = s.clone();
            f(s);
            *s != before
        });
    }

    pub fn set_twitch(&self, st: ConnState) {
        self.update(|s| s.twitch = st);
    }

    pub fn set_da(&self, st: ConnState) {
        self.update(|s| s.donationalerts = st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, sample};

    #[test]
    fn history_is_bounded_and_ordered() {
        let bus = EventBus::new();
        for _ in 0..(HISTORY_LIMIT + 10) {
            bus.publish(Event::test(sample("follow").unwrap()));
        }
        assert_eq!(bus.recent(10_000).len(), HISTORY_LIMIT);
        let last = bus.recent(2);
        assert_eq!(last.len(), 2);
        assert!(last[0].at <= last[1].at);
    }

    #[tokio::test]
    async fn subscribers_receive_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(Event::test(sample("donation").unwrap()));
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.kind.type_name(), "donation");
    }
}
