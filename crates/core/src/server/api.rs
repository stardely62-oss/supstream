use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::Html;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ApiError, ApiResult};
use crate::app::{AppState, VERSION};
use crate::bus::ConnectionsStatus;
use crate::events::{self, Event, EventKind, Source};
use crate::{donationalerts, twitch};

#[derive(Serialize)]
pub struct StatusResponse {
    version: &'static str,
    connections: ConnectionsStatus,
}

pub async fn status(State(state): State<Arc<AppState>>) -> ApiResult<StatusResponse> {
    Ok(Json(StatusResponse { version: VERSION, connections: state.status.get() }))
}

#[derive(Deserialize)]
pub struct RecentQuery {
    limit: Option<usize>,
}

pub async fn recent_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<RecentQuery>,
) -> ApiResult<Vec<Event>> {
    Ok(Json(state.bus.recent(q.limit.unwrap_or(100).min(500))))
}

#[derive(Deserialize)]
pub struct TestEventBody {
    #[serde(rename = "type")]
    kind: String,
}

pub async fn test_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TestEventBody>,
) -> ApiResult<Event> {
    let kind = events::sample(&body.kind).ok_or_else(|| {
        ApiError::bad_request(format!("неизвестный тип; доступны: {}", events::SAMPLE_TYPES.join(", ")))
    })?;
    let ev = Event::test(kind);
    state.bus.publish(ev.clone());
    Ok(Json(ev))
}

/// Опубликовать событие от внешнего скрипта/плагина: `{ "type": "donation", "data": {..} }`.
pub async fn publish_event(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Event> {
    let test = body.get("test").and_then(Value::as_bool).unwrap_or(false);
    let kind: EventKind =
        serde_json::from_value(body).map_err(|e| ApiError::bad_request(format!("неверное событие: {e}")))?;
    let mut ev = Event::new(Source::Supstream, kind);
    ev.test = test;
    state.bus.publish(ev.clone());
    Ok(Json(ev))
}

#[derive(Serialize)]
pub struct SettingsResponse {
    twitch: TwitchSettings,
    donationalerts: DaSettings,
    server: ServerSettings,
    data_dir: String,
}

#[derive(Serialize)]
struct TwitchSettings {
    /// Свой Client ID (пусто — используется вшитый).
    client_id: String,
    has_builtin_client_id: bool,
    configured: bool,
    login: Option<String>,
    missing_scopes: Vec<&'static str>,
}

#[derive(Serialize)]
struct DaSettings {
    client_id: String,
    client_secret_set: bool,
    redirect_uri: String,
    logged_in: bool,
}

#[derive(Serialize)]
struct ServerSettings {
    port: u16,
    allow_lan: bool,
}

pub async fn get_settings(State(state): State<Arc<AppState>>) -> ApiResult<SettingsResponse> {
    let cfg = state.store.config();
    let secrets = state.store.secrets();
    Ok(Json(SettingsResponse {
        twitch: TwitchSettings {
            client_id: cfg.twitch.client_id.clone(),
            has_builtin_client_id: crate::store::BUILTIN_TWITCH_CLIENT_ID.is_some_and(|s| !s.is_empty()),
            configured: cfg.twitch_client_id().is_some(),
            login: secrets.twitch.as_ref().map(|t| t.login.clone()),
            missing_scopes: secrets
                .twitch
                .as_ref()
                .map(|t| twitch::auth::missing_scopes(&t.scopes))
                .unwrap_or_default(),
        },
        donationalerts: DaSettings {
            client_id: cfg.donationalerts.client_id.clone(),
            client_secret_set: !cfg.donationalerts.client_secret.is_empty(),
            redirect_uri: cfg.da_redirect_uri(),
            logged_in: secrets.donationalerts.is_some(),
        },
        server: ServerSettings { port: cfg.server.port, allow_lan: cfg.server.allow_lan },
        data_dir: state.store.dir().display().to_string(),
    }))
}

#[derive(Deserialize)]
pub struct SettingsPatch {
    twitch_client_id: Option<String>,
    da_client_id: Option<String>,
    da_client_secret: Option<String>,
}

pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(patch): Json<SettingsPatch>,
) -> ApiResult<SettingsResponse> {
    let before = state.store.config();
    state
        .store
        .update_config(|c| {
            if let Some(v) = patch.twitch_client_id {
                c.twitch.client_id = v.trim().to_string();
            }
            if let Some(v) = patch.da_client_id {
                c.donationalerts.client_id = v.trim().to_string();
            }
            if let Some(v) = patch.da_client_secret {
                c.donationalerts.client_secret = v.trim().to_string();
            }
        })
        .map_err(ApiError::from)?;
    let after = state.store.config();
    if before.twitch_client_id() != after.twitch_client_id() {
        state.twitch.restart.notify_one();
    }
    if before.donationalerts.client_id != after.donationalerts.client_id
        || before.donationalerts.client_secret != after.donationalerts.client_secret
    {
        state.da.restart.notify_one();
    }
    get_settings(State(state)).await
}

pub async fn twitch_login(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let login = twitch::start_device_login(&state).await?;
    // Сразу открываем страницу подтверждения в браузере — код уже подставлен в ссылку.
    open_in_browser(&login.verification_uri);
    Ok(Json(json!(login)))
}

pub async fn twitch_logout(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    twitch::logout(&state).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct DaLoginBody {
    #[serde(default = "yes")]
    open_browser: bool,
}

/// `SUPSTREAM_NO_BROWSER=1` — не открывать браузер (сервер без экрана, тесты).
fn open_in_browser(url: &str) {
    if std::env::var_os("SUPSTREAM_NO_BROWSER").is_none() {
        let _ = open::that_detached(url);
    }
}

fn yes() -> bool {
    true
}

pub async fn da_login(
    State(state): State<Arc<AppState>>,
    body: Option<Json<DaLoginBody>>,
) -> ApiResult<Value> {
    let url = donationalerts::authorize_url(&state)?;
    if body.map(|b| b.open_browser).unwrap_or(true) {
        open_in_browser(&url);
    }
    Ok(Json(json!({ "url": url })))
}

pub async fn da_logout(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    donationalerts::logout(&state)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Сюда DonationAlerts возвращает браузер после входа. Без ключа API — его
/// вызывает сам DA, а защиту даёт одноразовый `state`.
pub async fn da_callback(State(app): State<Arc<AppState>>, Query(q): Query<CallbackQuery>) -> Html<String> {
    let result = match (q.code, q.state, q.error) {
        (_, _, Some(err)) => Err(format!("DonationAlerts вернул ошибку: {err}")),
        (Some(code), Some(csrf), None) => {
            donationalerts::complete_login(&app, &code, &csrf).await.map_err(|e| format!("{e:#}"))
        }
        _ => Err("не хватает параметров в ответе DonationAlerts".into()),
    };
    let (title, text) = match result {
        Ok(name) => (
            "Готово!".to_string(),
            format!("DonationAlerts подключён ({}). Вкладку можно закрыть.", html_escape(&name)),
        ),
        Err(e) => ("Не получилось".to_string(), html_escape(&e)),
    };
    Html(format!(
        r#"<!doctype html><html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Supstream</title>
<style>body{{font-family:system-ui,sans-serif;background:#111014;color:#eceaf2;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px}}main{{max-width:420px;text-align:center}}h1{{font-size:22px}}p{{color:#a9a5b8;line-height:1.5}}</style></head>
<body><main><h1>{title}</h1><p>{text}</p></main></body></html>"#
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenTarget {
    TwitchActivate,
    TwitchConsole,
    DonationalertsApps,
    DataDir,
}

#[derive(Deserialize)]
pub struct OpenBody {
    target: OpenTarget,
}

/// Открыть ссылку или папку на ПК стримера. Только из фиксированного списка —
/// произвольные адреса через API открывать нельзя.
pub async fn open_target(State(state): State<Arc<AppState>>, Json(body): Json<OpenBody>) -> ApiResult<Value> {
    let target = match body.target {
        OpenTarget::TwitchActivate => state
            .status
            .get()
            .twitch_device_login
            .map(|l| l.verification_uri)
            .unwrap_or_else(|| "https://www.twitch.tv/activate".into()),
        OpenTarget::TwitchConsole => "https://dev.twitch.tv/console/apps".into(),
        OpenTarget::DonationalertsApps => "https://www.donationalerts.com/application/clients".into(),
        OpenTarget::DataDir => state.store.dir().display().to_string(),
    };
    open::that_detached(&target).map_err(|e| ApiError::from(anyhow::Error::from(e)))?;
    Ok(Json(json!({ "ok": true })))
}
