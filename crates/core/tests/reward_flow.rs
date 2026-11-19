//! Награда «Заказать трек», созданная приложением: трек заиграл → активация подтверждена,
//! трек отклонён или убран → активация отменена (Twitch возвращает баллы).
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message as AxMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

#[derive(Default)]
struct Mock {
    rewards_created: Vec<Value>,
    rewards_updated: Vec<Value>,
    rewards_deleted: Vec<String>,
    /// (reward_id, redemption_id, status)
    redemptions: Vec<(String, String, String)>,
}
type Shared = Arc<Mutex<Mock>>;
type Q = Query<Vec<(String, String)>>;

fn q(query: &[(String, String)], k: &str) -> String {
    query.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default()
}

async fn validate() -> Json<Value> {
    Json(
        json!({ "client_id": "cid", "login": "streamer", "user_id": "42", "scopes": [], "expires_in": 5000 }),
    )
}

async fn subscribe() -> Response {
    (StatusCode::ACCEPTED, Json(json!({ "data": [] }))).into_response()
}

async fn ws(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut s: WebSocket| async move {
        let welcome = json!({
            "metadata": { "message_id": "w", "message_type": "session_welcome" },
            "payload": { "session": { "id": "s", "keepalive_timeout_seconds": 30 } }
        });
        s.send(AxMsg::text(welcome.to_string())).await.unwrap();
        while let Some(Ok(_)) = s.recv().await {}
    })
}

async fn create_reward(State(m): State<Shared>, Query(query): Q, Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(q(&query, "broadcaster_id"), "42");
    m.lock().unwrap().rewards_created.push(body);
    Json(json!({ "data": [{ "id": "rw-1" }] }))
}

async fn update_reward(State(m): State<Shared>, Query(query): Q, Json(body): Json<Value>) -> Response {
    if q(&query, "id") != "rw-1" {
        return (StatusCode::NOT_FOUND, Json(json!({ "message": "not found" }))).into_response();
    }
    m.lock().unwrap().rewards_updated.push(body);
    Json(json!({ "data": [{ "id": "rw-1" }] })).into_response()
}

async fn delete_reward(State(m): State<Shared>, Query(query): Q) -> StatusCode {
    m.lock().unwrap().rewards_deleted.push(q(&query, "id"));
    StatusCode::NO_CONTENT
}

async fn redemption(State(m): State<Shared>, Query(query): Q, Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(q(&query, "broadcaster_id"), "42");
    m.lock().unwrap().redemptions.push((
        q(&query, "reward_id"),
        q(&query, "id"),
        body["status"].as_str().unwrap().to_string(),
    ));
    Json(json!({ "data": [] }))
}

async fn lrclib(Query(query): Q) -> Json<Value> {
    let track = q(&query, "track_name");
    let lrc = match track.as_str() {
        "Clean Song" => "[00:01.00]hello\n[00:05.00]world",
        "Slur Song" => "[00:02.00]ты пидор\n[00:05.00]ok",
        _ => return Json(json!([])),
    };
    Json(
        json!([{ "trackName": track, "artistName": "x", "duration": 30.0, "syncedLyrics": lrc, "plainLyrics": lrc }]),
    )
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("127.0.0.1:{}", addr.port())
}

async fn eventually<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..150 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    panic!("не дождались: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reward_redemptions_are_fulfilled_or_refunded() {
    let mock: Shared = Arc::default();
    let tw = serve(
        Router::new()
            .route("/oauth2/validate", get(validate))
            .route("/helix/eventsub/subscriptions", post(subscribe))
            .route("/helix/chat/messages", post(subscribe))
            .route(
                "/helix/channel_points/custom_rewards",
                post(create_reward).patch(update_reward).delete(delete_reward),
            )
            .route("/helix/channel_points/custom_rewards/redemptions", axum::routing::patch(redemption))
            .route("/ws", get(ws))
            .with_state(mock.clone()),
    )
    .await;
    let lrc = serve(Router::new().route("/api/search", get(lrclib))).await;
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-yt-dlp.py");
    unsafe {
        std::env::set_var("SUPSTREAM_YTDLP", &fake);
        std::env::set_var("SUPSTREAM_LRCLIB_URL", format!("http://{lrc}"));
        std::env::set_var("SUPSTREAM_NO_BROWSER", "1");
    }

    // Уже выполненный вход в Twitch.
    let dir = std::env::temp_dir().join(format!("supstream-reward-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), r#"{ "twitch": { "client_id": "cid" } }"#).unwrap();
    std::fs::write(
        dir.join("secrets.json"),
        r#"{ "twitch": { "access_token": "at", "refresh_token": "rt", "scopes": [], "user_id": "42", "login": "streamer", "client_id": "cid" } }"#,
    )
    .unwrap();

    let running = supstream_core::start(supstream_core::StartOptions {
        data_dir: Some(dir.clone()),
        port: Some(0),
        twitch_endpoints: Some(supstream_core::twitch::Endpoints {
            id_base: format!("http://{tw}/oauth2"),
            helix_base: format!("http://{tw}/helix"),
            eventsub_ws: format!("ws://{tw}/ws"),
        }),
        da_endpoints: Some(supstream_core::donationalerts::Endpoints {
            base: "http://127.0.0.1:9".into(),
            centrifugo_ws: "ws://127.0.0.1:9/ws".into(),
        }),
    })
    .await
    .unwrap();
    let base = format!("http://127.0.0.1:{}/api", running.addr.port());
    let token = running.state.store.config().api_token;
    let http = reqwest::Client::new();
    let call = |method: reqwest::Method, path: &str, body: Value| {
        http.request(method, format!("{base}{path}")).bearer_auth(&token).json(&body).send()
    };

    let st = running.state.clone();
    eventually("Twitch подключён", || {
        matches!(st.status.get().twitch, supstream_core::bus::ConnState::Connected { .. }).then_some(())
    })
    .await;

    // Создание награды, затем обновление той же награды (не новой).
    let r = call(reqwest::Method::PUT, "/music/reward", json!({ "title": "Заказать трек", "cost": 500 }))
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let r = call(reqwest::Method::PUT, "/music/reward", json!({ "title": "Заказать трек", "cost": 700 }))
        .await
        .unwrap();
    let cfg: Value = r.json().await.unwrap();
    assert_eq!(cfg["reward_id"], "rw-1");
    assert_eq!(cfg["reward_cost"], 700);
    {
        let m = mock.lock().unwrap();
        assert_eq!(m.rewards_created.len(), 1);
        assert_eq!(m.rewards_created[0]["is_user_input_required"], true);
        assert_eq!(m.rewards_created[0]["cost"], 500);
        assert_eq!(m.rewards_updated.len(), 1);
        assert_eq!(m.rewards_updated[0]["cost"], 700);
    }

    let redeem = |redemption_id: &str, reward_id: &str, input: &str, test: bool| {
        json!({ "type": "redemption", "test": test, "data": {
            "redemption_id": redemption_id, "reward_id": reward_id, "reward_title": "Заказать трек", "cost": 700,
            "user_id": redemption_id, "user_login": "viewer", "user_name": "Viewer", "user_input": input,
            "status": "unfulfilled"
        }})
    };
    for (red, input) in [("red-ok", "clean"), ("red-bad", "slur"), ("red-rm", "nolyrics")] {
        call(reqwest::Method::POST, "/events", redeem(red, "rw-1", input, false)).await.unwrap();
    }
    // Чужая награда с другим названием и тестовое событие — Twitch не трогаем.
    let mut other = redeem("red-other", "other-reward", "clean again", false);
    other["data"]["reward_title"] = json!("Другая награда");
    call(reqwest::Method::POST, "/events", other).await.unwrap();
    call(reqwest::Method::POST, "/events", redeem("test", "rw-1", "test tune", true)).await.unwrap();

    let m2 = mock.clone();
    eventually("подтверждение и возврат", || {
        let r = m2.lock().unwrap().redemptions.clone();
        (r.len() >= 2).then_some(())
    })
    .await;
    {
        let r = mock.lock().unwrap().redemptions.clone();
        assert!(r.contains(&("rw-1".into(), "red-ok".into(), "FULFILLED".into())), "{r:?}");
        assert!(r.contains(&("rw-1".into(), "red-bad".into(), "CANCELED".into())), "{r:?}");
    }

    // Трек без текста ждёт проверки; стример убирает его — баллы возвращаются.
    let view: Value =
        http.get(format!("{base}/music")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let rm = view["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["query"] == "nolyrics")
        .unwrap_or_else(|| panic!("нет трека: {view}"))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        view["queue"].as_array().unwrap().iter().all(|t| t["query"] != "clean again"),
        "чужая награда не заказывает"
    );
    assert!(
        view["queue"].as_array().unwrap().iter().any(|t| t["query"] == "test tune"),
        "тестовое событие заказывает"
    );
    call(reqwest::Method::POST, &format!("/music/tracks/{rm}"), json!({ "action": "remove" })).await.unwrap();
    let m2 = mock.clone();
    eventually("возврат за убранный трек", || {
        m2.lock()
            .unwrap()
            .redemptions
            .iter()
            .any(|(_, id, s)| id == "red-rm" && s == "CANCELED")
            .then_some(())
    })
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    {
        let r = mock.lock().unwrap().redemptions.clone();
        assert_eq!(r.len(), 3, "каждая активация — ровно один раз, тестовые не трогаем: {r:?}");
    }

    // Удаление награды.
    let r = call(reqwest::Method::DELETE, "/music/reward", json!({})).await.unwrap();
    let cfg: Value = r.json().await.unwrap();
    assert_eq!(cfg["reward_id"], "");
    assert_eq!(mock.lock().unwrap().rewards_deleted, vec!["rw-1".to_string()]);

    running.shutdown();
    std::fs::remove_dir_all(dir).ok();
}
