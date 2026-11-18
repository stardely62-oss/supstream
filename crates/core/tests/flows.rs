//! Сквозные проверки на имитации серверов Twitch и DonationAlerts:
//! вход → токены → подписки → событие с площадки → WebSocket панели.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message as AxMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
struct Mock {
    token_polls: usize,
    subscriptions: Vec<Value>,
    da_messages: Vec<Value>,
}

type Shared = Arc<Mutex<Mock>>;

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("127.0.0.1:{}", addr.port())
}

// ---------- имитация Twitch ----------

async fn tw_device() -> Json<Value> {
    Json(json!({
        "device_code": "dc", "user_code": "ABCD-EFGH",
        "verification_uri": "https://www.twitch.tv/activate?device-code=ABCDEFGH",
        "expires_in": 60, "interval": 1
    }))
}

async fn tw_token(State(m): State<Shared>, Form(f): Form<Vec<(String, String)>>) -> Response {
    let get = |k: &str| f.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(get("client_id"), "cid");
    if get("grant_type").ends_with("device_code") {
        let mut m = m.lock().unwrap();
        m.token_polls += 1;
        if m.token_polls == 1 {
            // Первый опрос — пользователь ещё не подтвердил код.
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"status": 400, "message": "authorization_pending"})),
            )
                .into_response();
        }
    }
    Json(json!({ "access_token": "at", "refresh_token": "rt", "scope": ["user:read:chat"], "token_type": "bearer" }))
        .into_response()
}

async fn tw_validate(headers: HeaderMap) -> Response {
    assert_eq!(headers["authorization"], "OAuth at");
    Json(json!({ "client_id": "cid", "login": "streamer", "user_id": "42", "scopes": ["user:read:chat"], "expires_in": 5000 }))
        .into_response()
}

async fn tw_subscribe(State(m): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    assert_eq!(headers["client-id"], "cid");
    assert_eq!(headers["authorization"], "Bearer at");
    assert_eq!(body["transport"]["session_id"], "session-1");
    m.lock().unwrap().subscriptions.push(body);
    (StatusCode::ACCEPTED, Json(json!({ "data": [] }))).into_response()
}

async fn tw_ws(ws: WebSocketUpgrade, State(m): State<Shared>) -> Response {
    ws.on_upgrade(move |mut s: WebSocket| async move {
        let welcome = json!({
            "metadata": { "message_id": "m0", "message_type": "session_welcome", "message_timestamp": "2026-11-18T00:00:00Z" },
            "payload": { "session": { "id": "session-1", "status": "connected", "keepalive_timeout_seconds": 10, "reconnect_url": null } }
        });
        s.send(AxMsg::text(welcome.to_string())).await.unwrap();
        // Ждём, пока приложение оформит подписки.
        for _ in 0..100 {
            if m.lock().unwrap().subscriptions.len() >= 8 { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let chat = json!({
            "metadata": { "message_id": "m1", "message_type": "notification", "subscription_type": "channel.chat.message" },
            "payload": { "event": {
                "chatter_user_id": "7", "chatter_user_login": "viewer", "chatter_user_name": "Viewer",
                "message_id": "x", "message": { "text": "привет" }, "badges": [], "color": ""
            } }
        });
        s.send(AxMsg::text(chat.to_string())).await.unwrap();
        // Повтор того же message_id — должен быть отброшен.
        s.send(AxMsg::text(chat.to_string())).await.unwrap();
        let redemption = json!({
            "metadata": { "message_id": "m2", "message_type": "notification", "subscription_type": "channel.channel_points_custom_reward_redemption.add" },
            "payload": { "event": {
                "id": "r1", "user_id": "7", "user_login": "viewer", "user_name": "Viewer", "user_input": "",
                "status": "unfulfilled", "reward": { "id": "rw", "title": "Перевернуть камеру", "cost": 500 }
            } }
        });
        s.send(AxMsg::text(redemption.to_string())).await.unwrap();
        while let Some(Ok(_)) = s.recv().await {}
    })
}

// ---------- имитация DonationAlerts ----------

async fn da_token(Form(f): Form<Vec<(String, String)>>) -> Json<Value> {
    let get = |k: &str| f.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(get("grant_type"), "authorization_code");
    assert_eq!(get("code"), "the-code");
    assert_eq!(get("client_secret"), "dsecret");
    Json(
        json!({ "token_type": "Bearer", "expires_in": 31536000, "access_token": "da_at", "refresh_token": "da_rt" }),
    )
}

async fn da_user(headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers["authorization"], "Bearer da_at");
    Json(
        json!({ "data": { "id": 77, "code": "streamer", "name": "StreamerDA", "socket_connection_token": "sct" } }),
    )
}

async fn da_centrifuge_subscribe(Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["client"], "client-1");
    assert_eq!(body["channels"][0], "$alerts:donation_77");
    Json(json!({ "channels": [{ "channel": "$alerts:donation_77", "token": "chan-token" }] }))
}

async fn recv_json(s: &mut WebSocket) -> Value {
    loop {
        match s.recv().await {
            Some(Ok(AxMsg::Text(t))) => return serde_json::from_str(&t).unwrap(),
            Some(Ok(_)) => continue,
            _ => panic!("клиент DA отключился"),
        }
    }
}

async fn da_ws(ws: WebSocketUpgrade, State(m): State<Shared>) -> Response {
    ws.on_upgrade(move |mut s: WebSocket| async move {
        let connect = recv_json(&mut s).await;
        m.lock().unwrap().da_messages.push(connect.clone());
        assert_eq!(connect["params"]["token"], "sct");
        s.send(AxMsg::text(r#"{"id":1,"result":{"client":"client-1","version":"2.8.0"}}"#)).await.unwrap();
        let sub = recv_json(&mut s).await;
        m.lock().unwrap().da_messages.push(sub.clone());
        assert_eq!(sub["method"], 1);
        assert_eq!(sub["params"]["token"], "chan-token");
        // Ответ на подписку и публикация в одном кадре — как бывает у Centrifugo.
        let frame = format!(
            "{}\n{}",
            r#"{"id":2,"result":{}}"#,
            json!({"result":{"channel":"$alerts:donation_77","data":{"seq":1,"data":{
                "id": 555, "username": "Донатер", "message": "на музыку", "amount": 250, "currency": "RUB"
            }}}})
        );
        s.send(AxMsg::text(frame)).await.unwrap();
        while let Some(Ok(_)) = s.recv().await {}
    })
}

// ---------- клиент панели ----------

struct Panel {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Panel {
    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status(), r.json().await.unwrap_or(Value::Null))
    }
    async fn put(&self, path: &str, body: Value) -> Value {
        let r = self
            .http
            .put(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        r.json().await.unwrap()
    }
}

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Ждём сообщение WS, подходящее под условие.
async fn wait_for(ws: &mut WsStream, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let fut = async {
        loop {
            let Some(Ok(Message::Text(t))) = ws.next().await else { continue };
            let v: Value = serde_json::from_str(&t).unwrap();
            if pred(&v) {
                return v;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .unwrap_or_else(|_| panic!("не дождались: {what}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn twitch_and_donationalerts_end_to_end() {
    unsafe { std::env::set_var("SUPSTREAM_NO_BROWSER", "1") };
    let mock: Shared = Arc::default();

    let tw = serve(
        Router::new()
            .route("/oauth2/device", post(tw_device))
            .route("/oauth2/token", post(tw_token))
            .route("/oauth2/validate", get(tw_validate))
            .route("/helix/eventsub/subscriptions", post(tw_subscribe))
            .route("/ws", get(tw_ws))
            .with_state(mock.clone()),
    )
    .await;
    let da = serve(
        Router::new()
            .route("/oauth/token", post(da_token))
            .route("/api/v1/user/oauth", get(da_user))
            .route("/api/v1/centrifuge/subscribe", post(da_centrifuge_subscribe))
            .route("/connection/websocket", get(da_ws))
            .with_state(mock.clone()),
    )
    .await;

    let dir = std::env::temp_dir().join(format!("supstream-e2e-{}", uuid::Uuid::new_v4()));
    let running = supstream_core::start(supstream_core::StartOptions {
        data_dir: Some(dir.clone()),
        port: Some(0),
        twitch_endpoints: Some(supstream_core::twitch::Endpoints {
            id_base: format!("http://{tw}/oauth2"),
            helix_base: format!("http://{tw}/helix"),
            eventsub_ws: format!("ws://{tw}/ws"),
        }),
        da_endpoints: Some(supstream_core::donationalerts::Endpoints {
            base: format!("http://{da}"),
            centrifugo_ws: format!("ws://{da}/connection/websocket"),
        }),
    })
    .await
    .unwrap();

    let port = running.addr.port();
    let panel = Panel {
        base: format!("http://127.0.0.1:{port}/api"),
        token: running.state.store.config().api_token,
        http: reqwest::Client::new(),
    };

    // Без ключа API закрыто.
    let r = panel.http.get(format!("{}/status", panel.base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // Панель (статика) открыта.
    let r = panel.http.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/api/ws?token={}", panel.token))
            .await
            .unwrap();
    let hello = wait_for(&mut ws, "hello", |v| v["kind"] == "hello").await;
    assert!(hello["events"].is_array());

    // Без Client ID вход невозможен — понятная ошибка, а не падение.
    let (code, body) = panel.post("/auth/twitch/login", json!({})).await;
    assert_eq!(code, StatusCode::CONFLICT, "{body}");

    // ---- Twitch ----
    panel.put("/settings", json!({ "twitch_client_id": "cid" })).await;
    let (code, login) = panel.post("/auth/twitch/login", json!({})).await;
    assert_eq!(code, StatusCode::OK, "{login}");
    assert_eq!(login["user_code"], "ABCD-EFGH");

    wait_for(&mut ws, "twitch connected", |v| {
        v["kind"] == "status"
            && v["status"]["twitch"]["state"] == "connected"
            && v["status"]["twitch"]["account"] == "streamer"
    })
    .await;
    let chat =
        wait_for(&mut ws, "chat", |v| v["kind"] == "event" && v["event"]["type"] == "chat_message").await;
    assert_eq!(chat["event"]["data"]["text"], "привет");
    assert_eq!(chat["event"]["source"], "twitch");
    let red =
        wait_for(&mut ws, "redemption", |v| v["kind"] == "event" && v["event"]["type"] == "redemption").await;
    assert_eq!(red["event"]["data"]["reward_title"], "Перевернуть камеру");
    // Дубликат чата отброшен: в истории ровно одно сообщение.
    let recent: Vec<Value> = panel
        .http
        .get(format!("{}/events?limit=50", panel.base))
        .bearer_auth(&panel.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recent.iter().filter(|e| e["type"] == "chat_message").count(), 1);
    {
        let m = mock.lock().unwrap();
        assert_eq!(m.subscriptions.len(), 8);
        assert!(
            m.subscriptions
                .iter()
                .any(|s| s["type"] == "channel.channel_points_custom_reward_redemption.add")
        );
    }
    let secrets = running.state.store.secrets();
    assert_eq!(secrets.twitch.as_ref().unwrap().login, "streamer");

    // ---- DonationAlerts ----
    panel.put("/settings", json!({ "da_client_id": "did", "da_client_secret": "dsecret" })).await;
    let (code, body) = panel.post("/auth/donationalerts/login", json!({ "open_browser": false })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let url = url::Url::parse(body["url"].as_str().unwrap()).unwrap();
    let csrf = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();

    // Подделанный state отклоняется.
    let bad = panel
        .http
        .get(format!("http://127.0.0.1:{port}/auth/donationalerts/callback?code=the-code&state=evil"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(bad.contains("Не получилось"), "{bad}");

    let ok = panel
        .http
        .get(format!("http://127.0.0.1:{port}/auth/donationalerts/callback?code=the-code&state={csrf}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(ok.contains("Готово"), "{ok}");

    let don =
        wait_for(&mut ws, "donation", |v| v["kind"] == "event" && v["event"]["type"] == "donation").await;
    assert_eq!(don["event"]["source"], "donationalerts");
    assert_eq!(don["event"]["data"]["donor"], "Донатер");
    assert_eq!(don["event"]["data"]["amount"], 250.0);
    assert_eq!(don["event"]["data"]["message"], "на музыку");
    assert_eq!(
        running.state.status.get().donationalerts,
        supstream_core::bus::ConnState::Connected { account: "StreamerDA".into() }
    );

    // ---- Тестовые события и внешнее API ----
    let (code, _) = panel.post("/events/test", json!({ "type": "raid" })).await;
    assert_eq!(code, StatusCode::OK);
    let raid = wait_for(&mut ws, "test raid", |v| v["kind"] == "event" && v["event"]["type"] == "raid").await;
    assert_eq!(raid["event"]["test"], true);

    let (code, ev) = panel
        .post(
            "/events",
            json!({ "type": "follow", "data": { "user_id": "1", "user_login": "a", "user_name": "A" } }),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{ev}");
    let (code, _) = panel.post("/events", json!({ "type": "nonsense", "data": {} })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    // ---- Выход ----
    panel.post("/auth/donationalerts/logout", json!({})).await;
    wait_for(&mut ws, "da not configured", |v| {
        v["kind"] == "status" && v["status"]["donationalerts"]["state"] == "not_configured"
    })
    .await;

    running.shutdown();
    std::fs::remove_dir_all(dir).ok();
}
