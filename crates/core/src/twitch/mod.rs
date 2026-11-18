//! Интеграция с Twitch: вход, токены, Helix API и EventSub.

pub mod auth;
pub mod eventsub;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::app::{AppState, NotConfigured};
use crate::bus::{ConnState, DeviceLoginStatus};
use crate::store::TwitchTokens;

/// Адреса Twitch. Переопределяются переменными окружения — например, чтобы
/// разрабатывать на mock-сервере `twitch event websocket start-server` из Twitch CLI.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub id_base: String,
    pub helix_base: String,
    pub eventsub_ws: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            id_base: var("SUPSTREAM_TWITCH_ID_URL", "https://id.twitch.tv/oauth2"),
            helix_base: var("SUPSTREAM_TWITCH_HELIX_URL", "https://api.twitch.tv/helix"),
            eventsub_ws: var("SUPSTREAM_TWITCH_EVENTSUB_URL", "wss://eventsub.wss.twitch.tv/ws"),
        }
    }
}

pub struct TwitchService {
    pub endpoints: Endpoints,
    /// Разбудить супервизор: переподключиться с новыми настройками/токенами.
    pub restart: Notify,
    /// Refresh-токен одноразовый, поэтому обновляем строго по одному.
    refresh_lock: tokio::sync::Mutex<()>,
    login_task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TwitchService {
    pub fn new(endpoints: Endpoints) -> Self {
        Self {
            endpoints,
            restart: Notify::new(),
            refresh_lock: tokio::sync::Mutex::new(()),
            login_task: std::sync::Mutex::new(None),
        }
    }
}

fn client_id(state: &AppState) -> Result<String> {
    state.store.config().twitch_client_id().ok_or_else(|| {
        NotConfigured("Не задан Client ID Twitch-приложения (Настройки → Twitch)".into()).into()
    })
}

/// Действующий токен: проверяем его у Twitch и при необходимости обновляем.
pub async fn ensure_token(state: &Arc<AppState>) -> Result<TwitchTokens> {
    let client_id = client_id(state)?;
    let Some(tokens) = state.store.secrets().twitch else {
        bail!(NotConfigured("Войдите в Twitch".into()));
    };
    if tokens.client_id != client_id {
        bail!(NotConfigured("Client ID изменился — войдите в Twitch заново".into()));
    }
    let tokens = match auth::validate(&state.http, &state.twitch.endpoints, &tokens.access_token).await? {
        Some(_) => tokens,
        None => refresh_tokens(state, &tokens.access_token).await?,
    };
    let missing = auth::missing_scopes(&tokens.scopes);
    if !missing.is_empty() {
        tracing::warn!("Twitch: не хватает прав {missing:?} — войдите заново, чтобы их выдать");
    }
    Ok(tokens)
}

async fn refresh_tokens(state: &Arc<AppState>, stale_access: &str) -> Result<TwitchTokens> {
    let _guard = state.twitch.refresh_lock.lock().await;
    let Some(current) = state.store.secrets().twitch else {
        bail!(NotConfigured("Войдите в Twitch".into()));
    };
    if current.access_token != stale_access {
        return Ok(current); // пока ждали блокировку, токен уже обновили
    }
    match auth::refresh(&state.http, &state.twitch.endpoints, &current.client_id, &current.refresh_token)
        .await
    {
        Ok(fresh) => {
            let updated = TwitchTokens {
                access_token: fresh.access_token,
                refresh_token: fresh.refresh_token,
                scopes: if fresh.scope.is_empty() { current.scopes.clone() } else { fresh.scope },
                ..current
            };
            state.store.update_secrets(|s| s.twitch = Some(updated.clone()))?;
            tracing::info!("Twitch: токен обновлён");
            Ok(updated)
        }
        Err(auth::RefreshError::Revoked) => {
            state.store.update_secrets(|s| s.twitch = None)?;
            bail!(NotConfigured("Сессия Twitch истекла — войдите заново".into()))
        }
        Err(auth::RefreshError::Other(e)) => Err(e),
    }
}

/// Запрос к Helix API с автоматическим обновлением токена при 401.
pub async fn helix(
    state: &Arc<AppState>,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<reqwest::Response> {
    for attempt in 0..2 {
        let Some(tokens) = state.store.secrets().twitch else {
            bail!(NotConfigured("Войдите в Twitch".into()));
        };
        let mut req = state
            .http
            .request(method.clone(), format!("{}{}", state.twitch.endpoints.helix_base, path))
            .header("Client-Id", &tokens.client_id)
            .bearer_auth(&tokens.access_token);
        if let Some(b) = &body {
            req = req.json(b);
        }
        let resp = req.send().await.context("Twitch API недоступен")?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
            refresh_tokens(state, &tokens.access_token).await?;
            continue;
        }
        return Ok(resp);
    }
    unreachable!()
}

/// Фоновая задача: держит подключение к Twitch, переподключается при обрывах.
pub async fn supervise(state: Arc<AppState>) {
    let st = state.clone();
    crate::app::supervise(
        &state.twitch.restart,
        |s| st.status.set_twitch(s),
        || {
            let state = state.clone();
            async move {
                let tokens = ensure_token(&state).await?;
                eventsub::run(state, tokens).await
            }
        },
    )
    .await;
}

/// Начать вход по коду устройства. Код и ссылка уходят в статус (панель их покажет),
/// а опрос Twitch идёт в фоне до подтверждения или истечения кода.
pub async fn start_device_login(state: &Arc<AppState>) -> Result<DeviceLoginStatus> {
    let client_id = client_id(state)?;
    let code = auth::request_device_code(&state.http, &state.twitch.endpoints, &client_id).await?;
    let login = DeviceLoginStatus {
        user_code: code.user_code.clone(),
        verification_uri: code.verification_uri.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(code.expires_in as i64),
    };
    state.status.update(|s| s.twitch_device_login = Some(login.clone()));

    let st = state.clone();
    let handle = tokio::spawn(async move {
        let result = poll_device_login(&st, &client_id, code).await;
        st.status.update(|s| s.twitch_device_login = None);
        match result {
            Ok(login) => {
                tracing::info!("Twitch: выполнен вход как {login}");
                st.twitch.restart.notify_one();
            }
            Err(e) => {
                tracing::warn!("Twitch: вход не завершён: {e:#}");
                st.status.set_twitch(ConnState::NotConfigured {
                    reason: format!("Вход не завершён: {e:#}")
                });
            }
        }
    });
    if let Some(old) = state.twitch.login_task.lock().unwrap().replace(handle) {
        old.abort();
    }
    Ok(login)
}

async fn poll_device_login(state: &Arc<AppState>, client_id: &str, code: auth::DeviceCode) -> Result<String> {
    let ep = &state.twitch.endpoints;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.max(1));
    loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() > deadline {
            bail!("код истёк, попробуйте ещё раз");
        }
        match auth::poll_device_token(&state.http, ep, client_id, &code.device_code).await? {
            auth::Poll::Pending => {}
            auth::Poll::SlowDown => interval += Duration::from_secs(5),
            auth::Poll::Failed(msg) => bail!("Twitch: {msg}"),
            auth::Poll::Done(tok) => {
                let v = auth::validate(&state.http, ep, &tok.access_token)
                    .await?
                    .context("Twitch выдал недействительный токен")?;
                let tokens = TwitchTokens {
                    access_token: tok.access_token,
                    refresh_token: tok.refresh_token,
                    scopes: v.scopes,
                    user_id: v.user_id,
                    login: v.login.clone(),
                    client_id: client_id.to_string(),
                };
                state.store.update_secrets(|s| s.twitch = Some(tokens))?;
                return Ok(v.login);
            }
        }
    }
}

pub async fn logout(state: &Arc<AppState>) -> Result<()> {
    if let Some(task) = state.twitch.login_task.lock().unwrap().take() {
        task.abort();
    }
    state.status.update(|s| s.twitch_device_login = None);
    let old = state.store.secrets().twitch;
    state.store.update_secrets(|s| s.twitch = None)?;
    if let Some(t) = old {
        auth::revoke(&state.http, &state.twitch.endpoints, &t.client_id, &t.access_token).await;
    }
    state.twitch.restart.notify_one();
    Ok(())
}
