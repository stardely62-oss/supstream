//! Интеграция с DonationAlerts: OAuth и приём донатов в реальном времени.
//!
//! Деньги идут через DA, мы только слушаем события — никакой своей платёжки.
//! Документация: https://www.donationalerts.com/apidoc

pub mod socket;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::sync::Notify;

use crate::app::{AppState, NotConfigured};
use crate::store::{DaTokens, random_token};

pub const SCOPES: &str = "oauth-user-show oauth-donation-subscribe oauth-donation-index";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: String,
    pub centrifugo_ws: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            base: var("SUPSTREAM_DA_URL", "https://www.donationalerts.com"),
            centrifugo_ws: var(
                "SUPSTREAM_DA_CENTRIFUGO_URL",
                "wss://centrifugo.donationalerts.com/connection/websocket",
            ),
        }
    }
}

pub struct DaService {
    pub endpoints: Endpoints,
    pub restart: Notify,
    /// Одноразовые `state` для защиты OAuth-колбэка от подделки.
    pending_states: std::sync::Mutex<Vec<(String, Instant)>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

impl DaService {
    pub fn new(endpoints: Endpoints) -> Self {
        Self {
            endpoints,
            restart: Notify::new(),
            pending_states: std::sync::Mutex::new(Vec::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn take_state(&self, state: &str) -> bool {
        let mut pending = self.pending_states.lock().unwrap();
        pending.retain(|(_, at)| at.elapsed() < Duration::from_secs(600));
        let before = pending.len();
        pending.retain(|(s, _)| s != state);
        pending.len() != before
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaUser {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    pub socket_connection_token: String,
}

fn creds(state: &AppState) -> Result<(String, String)> {
    let cfg = state.store.config();
    if !cfg.da_configured() {
        bail!(NotConfigured(
            "Укажите ID и ключ приложения DonationAlerts (Подключения → DonationAlerts)".into()
        ));
    }
    Ok((cfg.donationalerts.client_id.trim().into(), cfg.donationalerts.client_secret.trim().into()))
}

/// Ссылка на страницу входа DA. Открывается в обычном браузере.
pub fn authorize_url(state: &AppState) -> Result<String> {
    let (client_id, _) = creds(state)?;
    let csrf = random_token();
    state.da.pending_states.lock().unwrap().push((csrf.clone(), Instant::now()));
    let mut url = url::Url::parse(&format!("{}/oauth/authorize", state.da.endpoints.base))?;
    url.query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &state.store.config().da_redirect_uri())
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("state", &csrf);
    Ok(url.into())
}

/// Обработка редиректа после входа: меняем `code` на токены.
pub async fn complete_login(state: &Arc<AppState>, code: &str, csrf: &str) -> Result<String> {
    if !state.da.take_state(csrf) {
        bail!("ссылка входа устарела или подделана — начните вход заново из панели");
    }
    let (client_id, client_secret) = creds(state)?;
    let resp = state
        .http
        .post(format!("{}/oauth/token", state.da.endpoints.base))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("redirect_uri", state.store.config().da_redirect_uri().as_str()),
            ("code", code),
        ])
        .send()
        .await
        .context("DonationAlerts недоступен")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("DonationAlerts отклонил вход ({status}): {body}");
    }
    let tok: TokenResponse = resp.json().await?;
    save_tokens(state, tok)?;
    let user = fetch_user(state).await?;
    state.da.restart.notify_one();
    Ok(user.name)
}

fn save_tokens(state: &AppState, tok: TokenResponse) -> Result<DaTokens> {
    let tokens = DaTokens {
        access_token: tok.access_token,
        refresh_token: tok.refresh_token,
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(tok.expires_in),
    };
    state.store.update_secrets(|s| s.donationalerts = Some(tokens.clone()))?;
    Ok(tokens)
}

/// Действующий access-токен (обновляется заранее, за сутки до истечения).
pub async fn access_token(state: &Arc<AppState>) -> Result<String> {
    let Some(tokens) = state.store.secrets().donationalerts else {
        bail!(NotConfigured("Войдите в DonationAlerts".into()));
    };
    if tokens.expires_at - chrono::Utc::now() > chrono::Duration::days(1) {
        return Ok(tokens.access_token);
    }
    let _guard = state.da.refresh_lock.lock().await;
    let (client_id, client_secret) = creds(state)?;
    let resp = state
        .http
        .post(format!("{}/oauth/token", state.da.endpoints.base))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", tokens.refresh_token.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("scope", SCOPES),
        ])
        .send()
        .await
        .context("DonationAlerts недоступен")?;
    if resp.status().is_client_error() {
        state.store.update_secrets(|s| s.donationalerts = None)?;
        bail!(NotConfigured("Сессия DonationAlerts истекла — войдите заново".into()));
    }
    if !resp.status().is_success() {
        bail!("DonationAlerts: ошибка обновления токена ({})", resp.status());
    }
    Ok(save_tokens(state, resp.json().await?)?.access_token)
}

pub async fn fetch_user(state: &Arc<AppState>) -> Result<DaUser> {
    #[derive(Deserialize)]
    struct Wrap {
        data: DaUser,
    }
    let token = access_token(state).await?;
    let resp = state
        .http
        .get(format!("{}/api/v1/user/oauth", state.da.endpoints.base))
        .bearer_auth(token)
        .send()
        .await
        .context("DonationAlerts недоступен")?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        state.store.update_secrets(|s| s.donationalerts = None)?;
        bail!(NotConfigured("Доступ к DonationAlerts отозван — войдите заново".into()));
    }
    if !resp.status().is_success() {
        bail!("DonationAlerts: не удалось получить профиль ({})", resp.status());
    }
    Ok(resp.json::<Wrap>().await?.data)
}

pub async fn supervise(state: Arc<AppState>) {
    let st = state.clone();
    crate::app::supervise(&state.da.restart, |s| st.status.set_da(s), || socket::run(state.clone())).await;
}

pub fn logout(state: &AppState) -> Result<()> {
    state.store.update_secrets(|s| s.donationalerts = None)?;
    state.da.restart.notify_one();
    Ok(())
}
