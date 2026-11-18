//! WebSocket `/api/ws`: поток событий и статусов для панели, виджетов и скриптов.
//!
//! Сервер → клиент:
//! * `{ "kind": "hello", "version", "status", "events": [..] }` — сразу после подключения;
//! * `{ "kind": "event", "event": {..} }`;
//! * `{ "kind": "status", "status": {..} }`.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::app::{AppState, VERSION};

pub async fn handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| session(socket, state))
}

async fn send(socket: &mut WebSocket, value: serde_json::Value) -> bool {
    socket.send(Message::text(value.to_string())).await.is_ok()
}

async fn session(mut socket: WebSocket, state: Arc<AppState>) {
    // Подписываемся до отправки истории, чтобы не потерять события между ними.
    let mut events = state.bus.subscribe();
    let mut status = state.status.subscribe();
    let hello = json!({
        "kind": "hello",
        "version": VERSION,
        "status": *status.borrow_and_update(),
        "events": state.bus.recent(100),
    });
    if !send(&mut socket, hello).await {
        return;
    }
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(ev) => {
                    if !send(&mut socket, json!({ "kind": "event", "event": ev })).await { return; }
                }
                Err(RecvError::Lagged(n)) => tracing::warn!("WS-клиент отстал на {n} событий"),
                Err(RecvError::Closed) => return,
            },
            changed = status.changed() => {
                if changed.is_err() { return; }
                let st = status.borrow_and_update().clone();
                if !send(&mut socket, json!({ "kind": "status", "status": st })).await { return; }
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {} // команды от клиента появятся вместе с системой действий
            },
        }
    }
}
