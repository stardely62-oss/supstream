//! Вход в Twitch через Device Code Flow (для Public-клиентов).
//!
//! Программа показывает код, человек вводит его на twitch.tv/activate —
//! client secret не нужен, поэтому exe можно спокойно раздавать друзьям.
//! Документация: https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/#device-code-grant-flow

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::Endpoints;

/// Права, которые запрашиваем при входе. Если добавляете новое — пользователям
/// придётся войти заново (панель предупредит об этом).
pub const SCOPES: &[&str] = &[
    "channel:read:redemptions",
    "channel:manage:redemptions",
    "user:read:chat",
    "user:write:chat",
    "user:bot",
    "channel:bot",
    "bits:read",
    "channel:read:subscriptions",
    "moderator:read:followers",
    "channel:manage:polls",
    "channel:manage:predictions",
    "moderator:manage:banned_users",
];

pub fn scopes_param() -> String {
    SCOPES.join(" ")
}

pub fn missing_scopes(granted: &[String]) -> Vec<&'static str> {
    SCOPES.iter().copied().filter(|s| !granted.iter().any(|g| g == s)).collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Validation {
    pub client_id: String,
    pub login: String,
    pub user_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TwitchError {
    #[serde(default)]
    message: String,
}

pub enum Poll {
    Pending,
    SlowDown,
    Done(TokenResponse),
    /// Код истёк или вход отклонён — нужно начинать заново.
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    /// Refresh-токен больше не действует — нужен повторный вход.
    #[error("сессия Twitch истекла, войдите заново")]
    Revoked,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub async fn request_device_code(
    http: &reqwest::Client,
    ep: &Endpoints,
    client_id: &str,
) -> Result<DeviceCode> {
    let resp = http
        .post(format!("{}/device", ep.id_base))
        .form(&[("client_id", client_id), ("scopes", &scopes_param())])
        .send()
        .await
        .context("Twitch недоступен")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "Twitch отклонил запрос кода ({status}): {body}. Проверьте Client ID и что тип клиента — Public"
        );
    }
    Ok(resp.json().await?)
}

pub async fn poll_device_token(
    http: &reqwest::Client,
    ep: &Endpoints,
    client_id: &str,
    device_code: &str,
) -> Result<Poll> {
    let resp = http
        .post(format!("{}/token", ep.id_base))
        .form(&[
            ("client_id", client_id),
            ("scopes", &scopes_param()),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await?;
    if resp.status().is_success() {
        return Ok(Poll::Done(resp.json().await?));
    }
    let err: TwitchError = resp.json().await.unwrap_or(TwitchError { message: String::new() });
    Ok(match err.message.as_str() {
        "authorization_pending" => Poll::Pending,
        "slow_down" => Poll::SlowDown,
        "" => Poll::Pending,
        other => Poll::Failed(other.to_string()),
    })
}

pub async fn refresh(
    http: &reqwest::Client,
    ep: &Endpoints,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, RefreshError> {
    // Public-клиент обновляет токен без секрета. Refresh-токен одноразовый:
    // новый обязательно сохраняем.
    let resp = http
        .post(format!("{}/token", ep.id_base))
        .form(&[("client_id", client_id), ("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
        .send()
        .await
        .context("Twitch недоступен")?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp.json().await.context("неожиданный ответ Twitch")?);
    }
    if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(RefreshError::Revoked);
    }
    Err(anyhow::anyhow!("Twitch: ошибка обновления токена ({status})").into())
}

/// `Ok(None)` — токен недействителен.
pub async fn validate(
    http: &reqwest::Client,
    ep: &Endpoints,
    access_token: &str,
) -> Result<Option<Validation>> {
    let resp = http
        .get(format!("{}/validate", ep.id_base))
        .header("Authorization", format!("OAuth {access_token}"))
        .send()
        .await
        .context("Twitch недоступен")?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("Twitch: ошибка проверки токена ({})", resp.status());
    }
    Ok(Some(resp.json().await?))
}

pub async fn revoke(http: &reqwest::Client, ep: &Endpoints, client_id: &str, token: &str) {
    let _ = http
        .post(format!("{}/revoke", ep.id_base))
        .form(&[("client_id", client_id), ("token", token)])
        .send()
        .await;
}
