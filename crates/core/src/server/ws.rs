//! WebSocket `/api/ws`: поток событий и статусов для панели, виджетов и скриптов.
//!
//! Сервер → клиент:
//! * `{ "kind": "hello", "version", "status", "events": [..] }` — сразу после подключения;
//! * `{ "kind": "event", "event": {..} }`;
//! * `{ "kind": "status", "status": {..} }`;
//! * `{ "kind": "music", "music": {..} }` — очередь и плеер;
//! * `{ "kind": "music_progress", "progress": {..} }` — позиция трека;
//! * `{ "kind": "analysis", "analysis": {..} }` — компоненты точного глушения (загрузка, состояние).
//!
//! `?client=player` — подключился виджет-плеер (панель показывает, что он есть).

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::app::{AppState, VERSION};

#[derive(serde::Deserialize)]
pub struct WsQuery {
    client: Option<String>,
}

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(q): Query<WsQuery>,
) -> Response {
    let is_player = q.client.as_deref() == Some("player");
    ws.on_upgrade(move |socket| async move {
        // Счётчик плееров уменьшится, даже если соединение оборвётся.
        struct PlayerGuard(Arc<AppState>);
        impl Drop for PlayerGuard {
            fn drop(&mut self) {
                self.0.music.player_connected(-1);
            }
        }
        let _guard = is_player.then(|| {
            state.music.player_connected(1);
            PlayerGuard(state.clone())
        });
        session(socket, state).await
    })
}

async fn send(socket: &mut WebSocket, value: serde_json::Value) -> bool {
    socket.send(Message::text(value.to_string())).await.is_ok()
}

async fn session(mut socket: WebSocket, state: Arc<AppState>) {
    // Подписываемся до отправки истории, чтобы не потерять события между ними.
    let mut events = state.bus.subscribe();
    let mut status = state.status.subscribe();
    let mut music = state.music.subscribe();
    let mut progress = state.music.subscribe_progress();
    let mut analysis = state.analysis.subscribe();
    analysis.borrow_and_update();
    music.borrow_and_update();
    progress.borrow_and_update();
    let hello = json!({
        "kind": "hello",
        "version": VERSION,
        "status": *status.borrow_and_update(),
        "events": state.bus.recent(100),
        "music": state.music.view(),
        "analysis": state.analysis.status(),
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
            changed = music.changed() => {
                if changed.is_err() { return; }
                music.borrow_and_update();
                if !send(&mut socket, json!({ "kind": "music", "music": state.music.view() })).await { return; }
            },
            changed = progress.changed() => {
                if changed.is_err() { return; }
                let p = progress.borrow_and_update().clone();
                if !send(&mut socket, json!({ "kind": "music_progress", "progress": p })).await { return; }
            },
            changed = analysis.changed() => {
                if changed.is_err() { return; }
                analysis.borrow_and_update();
                if !send(&mut socket, json!({ "kind": "analysis", "analysis": state.analysis.status() })).await { return; }
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {} // команды от клиента появятся вместе с системой действий
            },
        }
    }
}
