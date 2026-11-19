//! Сквозная проверка музыки на имитации yt-dlp и LRCLIB:
//! заказ → поиск → загрузка → проверка текста → очередь → плеер → следующий трек.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use axum::extract::Query;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn lrclib_search(Query(q): Query<Vec<(String, String)>>) -> Json<Value> {
    let get = |k: &str| q.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let track = get("track_name");
    let item = |artist: &str, track: &str, lrc: &str| {
        json!([{ "trackName": track, "artistName": artist, "duration": 30.0, "instrumental": false,
                 "plainLyrics": lrc, "syncedLyrics": lrc }])
    };
    Json(match track.as_str() {
        "Clean Song" => item("Good Band", "Clean Song", "[00:01.00]hello world\n[00:05.00]sunny day"),
        "Dirty Song" => item("Bad Band", "Dirty Song", "[00:02.00]иди нахуй отсюда\n[00:06.00]всё хорошо"),
        "Slur Song" => item("Worse Band", "Slur Song", "[00:02.00]ты пидор\n[00:05.00]ok"),
        _ => json!([]),
    })
}

struct App {
    base: String,
    token: String,
    http: reqwest::Client,
    port: u16,
}

impl App {
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
    async fn get(&self, path: &str) -> Value {
        self.http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn ws(&self, client: &str) -> WsStream {
        tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{}/api/ws?client={client}&token={}",
            self.port, self.token
        ))
        .await
        .unwrap()
        .0
    }
}

fn all_tracks(view: &Value) -> Vec<Value> {
    let mut v = Vec::new();
    if !view["now"].is_null() {
        v.push(view["now"].clone());
    }
    for k in ["queue", "history"] {
        v.extend(view[k].as_array().cloned().unwrap_or_default());
    }
    v
}

/// Ждём, пока состояние музыки (по WebSocket) удовлетворит условию.
async fn wait_music(ws: &mut WsStream, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let fut = async {
        loop {
            let Some(Ok(Message::Text(t))) = ws.next().await else { continue };
            let m: Value = serde_json::from_str(&t).unwrap();
            if (m["kind"] == "music" || m["kind"] == "hello") && pred(&m["music"]) {
                return m["music"].clone();
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .unwrap_or_else(|_| panic!("не дождались: {what}"))
}

fn track_by_query<'a>(tracks: &'a [Value], q: &str) -> Option<&'a Value> {
    tracks.iter().find(|t| t["query"] == q)
}

async fn start(dir: &std::path::Path) -> (supstream_core::Running, App) {
    let running = supstream_core::start(supstream_core::StartOptions {
        data_dir: Some(dir.to_path_buf()),
        port: Some(0),
        // Площадки не нужны: указываем заведомо пустые адреса.
        twitch_endpoints: Some(supstream_core::twitch::Endpoints {
            id_base: "http://127.0.0.1:9/oauth2".into(),
            helix_base: "http://127.0.0.1:9/helix".into(),
            eventsub_ws: "ws://127.0.0.1:9/ws".into(),
        }),
        da_endpoints: Some(supstream_core::donationalerts::Endpoints {
            base: "http://127.0.0.1:9".into(),
            centrifugo_ws: "ws://127.0.0.1:9/ws".into(),
        }),
    })
    .await
    .unwrap();
    let port = running.addr.port();
    let app = App {
        base: format!("http://127.0.0.1:{port}/api"),
        token: running.state.store.config().api_token,
        http: reqwest::Client::new(),
        port,
    };
    (running, app)
}

#[tokio::test(flavor = "multi_thread")]
async fn music_request_check_and_playback() {
    let lrclib = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/api/search", get(lrclib_search))).await.unwrap()
        });
        format!("http://{addr}")
    };
    let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-yt-dlp.py");
    unsafe {
        std::env::set_var("SUPSTREAM_YTDLP", &fake);
        std::env::set_var("SUPSTREAM_LRCLIB_URL", &lrclib);
        std::env::set_var("SUPSTREAM_NO_BROWSER", "1");
    }

    let dir = std::env::temp_dir().join(format!("supstream-music-{}", uuid::Uuid::new_v4()));
    let (running, app) = start(&dir).await;

    // Панель и «плеер» на WebSocket.
    let mut panel = app.ws("panel").await;
    let _player = app.ws("player").await;
    wait_music(&mut panel, "плеер подключён", |m| m["players_connected"] == 1).await;

    // 1. Чистый трек: найден → проверен → сразу играет (очередь была пуста).
    let (code, t) = app.post("/music/request", json!({ "query": "clean" })).await;
    assert_eq!(code, StatusCode::OK, "{t}");
    let m = wait_music(&mut panel, "clean играет", |m| {
        m["now"]["query"] == "clean" && m["now"]["status"] == "playing"
    })
    .await;
    let clean_id = m["now"]["id"].as_str().unwrap().to_string();
    assert_eq!(m["now"]["meta"]["artist"], "Good Band");
    assert_eq!(m["now"]["check"]["lyrics"]["synced"], true);
    assert!(m["now"]["mutes"].as_array().unwrap().is_empty());

    // Звук отдаётся с поддержкой Range (перемотка в плеере).
    let media = format!("{}/media/{clean_id}?token={}", app.base, app.token);
    let full = app.http.get(&media).send().await.unwrap();
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(full.bytes().await.unwrap().len(), 900);
    let part = app.http.get(&media).header("Range", "bytes=0-8").send().await.unwrap();
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&part.bytes().await.unwrap()[..], b"FAKEAUDIO");
    // Без ключа — нельзя.
    let r = app.http.get(format!("{}/media/{clean_id}", app.base)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // 2. Остальные случаи.
    for q in ["dirty", "slur", "nolyrics", "long", "fail"] {
        let (code, body) = app.post("/music/request", json!({ "query": q })).await;
        assert_eq!(code, StatusCode::OK, "{q}: {body}");
    }
    let m = wait_music(&mut panel, "все проверены", |m| {
        let t = all_tracks(m);
        ["dirty", "slur", "nolyrics", "long", "fail"].iter().all(|q| {
            track_by_query(&t, q)
                .is_some_and(|x| !matches!(x["status"].as_str(), Some("resolving" | "checking")))
        })
    })
    .await;
    let t = all_tracks(&m);
    let dirty = track_by_query(&t, "dirty").unwrap();
    assert_eq!(dirty["status"], "ready", "мат по умолчанию глушится: {dirty}");
    let mutes = dirty["mutes"].as_array().unwrap();
    assert_eq!(mutes.len(), 1);
    assert_eq!(mutes[0]["enabled"], true);
    assert!(mutes[0]["label"].as_str().unwrap().contains("нахуй"));
    let (s, e) = (mutes[0]["start"].as_f64().unwrap(), mutes[0]["end"].as_f64().unwrap());
    assert!(s >= 1.7 && e <= 6.0 && s < e, "отметка внутри строки 2–6 с: {s}–{e}");

    let slur = track_by_query(&t, "slur").unwrap();
    assert_eq!(slur["status"], "rejected", "оскорбления по умолчанию отклоняются");
    assert_eq!(slur["check"]["hate"], 1);
    assert_eq!(track_by_query(&t, "nolyrics").unwrap()["status"], "needs_review");
    let long = track_by_query(&t, "long").unwrap();
    assert_eq!(long["status"], "rejected");
    assert!(long["error"].as_str().unwrap().contains("длиннее"), "{long}");
    let fail = track_by_query(&t, "fail").unwrap();
    assert_eq!(fail["status"], "failed");
    assert!(fail["error"].as_str().unwrap().contains("Video unavailable"), "{fail}");

    // Повтор трека, который уже в очереди, отклоняется.
    app.post("/music/request", json!({ "query": "clean again" })).await; // другой трек
    let (_, dup) = app.post("/music/request", json!({ "query": "clean" })).await;
    let dup_id = dup["id"].as_str().unwrap().to_string();
    let m = wait_music(&mut panel, "дубль отклонён", |m| {
        all_tracks(m).iter().any(|t| t["id"] == dup_id && t["status"] == "rejected")
    })
    .await;
    let d = all_tracks(&m).into_iter().find(|t| t["id"] == dup_id).unwrap();
    assert!(d["error"].as_str().unwrap().contains("уже в очереди"));

    // 3. Плеер сообщил позицию → лёгкое сообщение music_progress.
    app.post("/music/player/report", json!({ "track_id": clean_id, "position": 12.5, "duration": 30.0 }))
        .await;
    let progress = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(Ok(Message::Text(t))) = panel.next().await else { continue };
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["kind"] == "music_progress" {
                return v;
            }
        }
    })
    .await
    .expect("нет music_progress");
    assert_eq!(progress["progress"]["position"], 12.5);

    // Трек закончился → следующий готовый (dirty). Повторный «ended» не перескакивает дальше.
    app.post("/music/player/report", json!({ "track_id": clean_id, "ended": true })).await;
    app.post("/music/player/report", json!({ "track_id": clean_id, "ended": true })).await;
    let m = wait_music(&mut panel, "dirty играет", |m| m["now"]["query"] == "dirty").await;
    let dirty_id = m["now"]["id"].as_str().unwrap().to_string();
    assert!(all_tracks(&m).iter().any(|t| t["id"] == clean_id && t["status"] == "played"));

    // 4. Ручная проверка: одобряем трек без текста и ставим его следующим.
    let nol_id = track_by_query(&all_tracks(&m), "nolyrics").unwrap()["id"].as_str().unwrap().to_string();
    let (code, body) = app.post(&format!("/music/tracks/{nol_id}"), json!({ "action": "approve" })).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    app.post(&format!("/music/tracks/{nol_id}"), json!({ "action": "move", "to": 0 })).await;
    let m = wait_music(&mut panel, "nolyrics одобрен и первый", |m| {
        m["queue"][0]["id"] == nol_id.as_str() && m["queue"][0]["status"] == "ready"
    })
    .await;
    assert_eq!(m["queue"][0]["approved"], true);

    // Своя отметка глушения + выключение автоматической.
    app.post(
        &format!("/music/tracks/{dirty_id}"),
        json!({ "action": "add_mute", "start": 10.0, "end": 12.0, "label": "строка" }),
    )
    .await;
    app.post(
        &format!("/music/tracks/{dirty_id}"),
        json!({ "action": "set_mute", "mute_id": 1, "enabled": false }),
    )
    .await;
    let m = wait_music(&mut panel, "отметки", |m| {
        m["now"]["mutes"].as_array().is_some_and(|a| a.len() == 2 && a[0]["enabled"] == false)
    })
    .await;
    let mutes = m["now"]["mutes"].as_array().unwrap();
    assert_eq!(mutes[0]["enabled"], false);
    assert_eq!(mutes[1]["auto"], false);
    let (code, _) = app
        .post(
            &format!("/music/tracks/{dirty_id}"),
            json!({ "action": "add_mute", "start": 5.0, "end": 1.0, "label": "x" }),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    // 5. Тревожная кнопка: пауза + паника; «играть» снимает обе.
    app.post("/music/player", json!({ "action": "panic", "on": true })).await;
    wait_music(&mut panel, "паника", |m| m["player"]["panic"] == true && m["player"]["paused"] == true).await;
    app.post("/music/player", json!({ "action": "play" })).await;
    wait_music(&mut panel, "снята", |m| m["player"]["panic"] == false && m["player"]["paused"] == false)
        .await;
    app.post("/music/player", json!({ "action": "skip" })).await;
    wait_music(&mut panel, "skip → nolyrics", |m| m["now"]["id"] == nol_id.as_str()).await;

    // 6. Заказы из событий: чат (с лимитом на зрителя) и донат от суммы.
    for q in ["first song", "second song", "third song"] {
        let ev = json!({ "type": "chat_message", "data": {
            "message_id": q, "user_id": "777", "user_login": "fan", "user_name": "Fan",
            "text": format!("!sr {q}"), "badges": []
        }});
        app.post("/events", ev).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ev = json!({ "type": "chat_message", "data": {
        "message_id": "x", "user_id": "778", "user_login": "other", "user_name": "Other", "text": "просто болтаю", "badges": []
    }});
    app.post("/events", ev).await;

    let mut cfg = app.get("/music/settings").await;
    cfg["donation_min_amount"] = json!(100);
    let (code, _) = app.post("/music/settings", json!({})).await; // POST не поддерживается
    assert_eq!(code, StatusCode::METHOD_NOT_ALLOWED);
    let r = app
        .http
        .put(format!("{}/music/settings", app.base))
        .bearer_auth(&app.token)
        .json(&cfg)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    for (amount, msg) in [(50, "cheap song"), (150, "врубай https://youtu.be/donated")] {
        let ev = json!({ "type": "donation", "data": { "donor": "Rich", "amount": amount, "currency": "RUB", "message": msg } });
        app.post("/events", ev).await;
    }

    let m = wait_music(&mut panel, "заказы из событий", |m| {
        let t = all_tracks(m);
        t.iter().filter(|x| x["requested_by"]["via"] == "chat").count() == 2
            && t.iter().any(|x| x["requested_by"]["via"] == "donation")
    })
    .await;
    let t = all_tracks(&m);
    let from_chat: Vec<&Value> = t.iter().filter(|x| x["requested_by"]["via"] == "chat").collect();
    assert!(from_chat.iter().all(|x| x["requested_by"]["name"] == "Fan"));
    assert!(track_by_query(&t, "third song").is_none(), "третий трек сверх лимита");
    let donated = t.iter().find(|x| x["requested_by"]["via"] == "donation").unwrap();
    assert_eq!(donated["query"], "https://youtu.be/donated");
    assert!(track_by_query(&t, "cheap song").is_none(), "донат меньше порога не заказывает");

    // 7. Безопасный режим: только разрешённые каналы и своя папка.
    let put_cfg = |cfg: Value| {
        let app = &app;
        async move {
            let r = app
                .http
                .put(format!("{}/music/settings", app.base))
                .bearer_auth(&app.token)
                .json(&cfg)
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success());
        }
    };
    let mut cfg = app.get("/music/settings").await;
    cfg["source_mode"] = json!("safe");
    put_cfg(cfg.clone()).await;

    let request_and_wait = |q: &'static str| {
        let app = &app;
        async move {
            let (code, t) = app.post("/music/request", json!({ "query": q })).await;
            assert_eq!(code, StatusCode::OK, "{q}: {t}");
            t["id"].as_str().unwrap().to_string()
        }
    };
    let settled = |m: &Value, id: &str| {
        all_tracks(m)
            .into_iter()
            .find(|t| t["id"] == id && !matches!(t["status"].as_str(), Some("resolving" | "checking")))
    };

    let id = request_and_wait("some hit").await;
    let m = wait_music(&mut panel, "нет источников", |m| settled(m, &id).is_some()).await;
    let t = settled(&m, &id).unwrap();
    assert_eq!(t["status"], "rejected");
    assert!(t["error"].as_str().unwrap().contains("не добавил разрешённые источники"), "{t}");

    let (code, cfg_after) =
        app.post("/music/safe-channels", json!({ "url": "https://www.youtube.com/@safebeats" })).await;
    assert_eq!(code, StatusCode::OK, "{cfg_after}");
    assert_eq!(cfg_after["safe_channels"][0]["channel_id"], "UCSAFE");
    assert_eq!(cfg_after["safe_channels"][0]["name"], "Safe Beats");
    // Повторное добавление не дублирует канал.
    let (_, again) =
        app.post("/music/safe-channels", json!({ "url": "https://www.youtube.com/@safebeats" })).await;
    assert_eq!(again["safe_channels"].as_array().unwrap().len(), 1);

    let found = request_and_wait("chill vibes").await;
    let major = request_and_wait("https://www.youtube.com/watch?v=major1").await;
    let safe_url = request_and_wait("https://www.youtube.com/watch?v=safe2").await;
    let missing = request_and_wait("missing thing").await;
    let m = wait_music(&mut panel, "безопасный режим", |m| {
        [&found, &major, &safe_url, &missing].iter().all(|id| settled(m, id).is_some())
    })
    .await;
    let t = settled(&m, &found).unwrap();
    assert_eq!(t["meta"]["channel_id"], "UCSAFE", "поиск идёт внутри разрешённого канала: {t}");
    assert!(!matches!(t["status"].as_str(), Some("rejected" | "failed")), "{t}");
    let t = settled(&m, &major).unwrap();
    assert_eq!(t["status"], "rejected");
    assert!(t["error"].as_str().unwrap().contains("Major Label"), "{t}");
    assert!(!matches!(settled(&m, &safe_url).unwrap()["status"].as_str(), Some("rejected" | "failed")));
    let t = settled(&m, &missing).unwrap();
    assert!(t["error"].as_str().unwrap().contains("не найден в разрешённых"), "{t}");

    // Своя папка: трек берётся с диска, не скачивается, и плеер может его получить.
    let lib = dir.join("my-music");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("Local Artist - Own Song.mp3"), b"OWNAUDIO").unwrap();
    let mut cfg = app.get("/music/settings").await;
    cfg["local_dirs"] = json!([lib.display().to_string()]);
    put_cfg(cfg).await;
    let own = request_and_wait("own song").await;
    let m = wait_music(&mut panel, "своя музыка", |m| settled(m, &own).is_some()).await;
    let t = settled(&m, &own).unwrap();
    assert_eq!(t["meta"]["local"], true, "{t}");
    assert_eq!(t["meta"]["artist"], "Local Artist");
    let bytes = app
        .http
        .get(format!("{}/media/{own}?token={}", app.base, app.token))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"OWNAUDIO");

    // Награда меняется только своим адресом — сохранение формы её не затирает.
    let mut cfg = app.get("/music/settings").await;
    cfg["reward_id"] = json!("hacked");
    put_cfg(cfg).await;
    assert_eq!(app.get("/music/settings").await["reward_id"], "");

    // 8. После перезапуска очередь на месте.
    let queued_before = m["queue"].as_array().unwrap().len();
    drop(panel);
    running.shutdown();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (running, app) = start(&dir).await;
    let view = app.get("/music").await;
    let restored = all_tracks(&view).len();
    assert!(restored >= queued_before, "очередь не восстановилась: {restored} < {queued_before}");
    assert!(all_tracks(&view).iter().any(|t| t["query"] == "dirty"));

    running.shutdown();
    std::fs::remove_dir_all(dir).ok();
}
