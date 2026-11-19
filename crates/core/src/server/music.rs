//! API музыки и раздача звука для виджета-плеера.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{ApiError, ApiResult};
use crate::app::AppState;
use crate::music::track::{MusicConfig, Requester, Track, Via};
use crate::music::{self, MusicView, PlayerCommand, PlayerReport, Refused, TrackCommand};

impl From<Refused> for ApiError {
    fn from(e: Refused) -> Self {
        ApiError::bad_request(e.0)
    }
}

fn map_err(e: anyhow::Error) -> ApiError {
    match e.downcast::<Refused>() {
        Ok(r) => r.into(),
        Err(e) => e.into(),
    }
}

pub async fn view(State(state): State<Arc<AppState>>) -> ApiResult<MusicView> {
    Ok(Json(state.music.view()))
}

#[derive(Deserialize)]
pub struct RequestBody {
    query: String,
}

pub async fn request(State(state): State<Arc<AppState>>, Json(b): Json<RequestBody>) -> ApiResult<Track> {
    let requester =
        Requester {
            name: "Стример".into(), via: Via::Panel, user_id: None, redemption: None, amount: None
        };
    music::request(&state, &b.query, requester).map(Json).map_err(map_err)
}

pub async fn player(State(state): State<Arc<AppState>>, Json(cmd): Json<PlayerCommand>) -> ApiResult<Value> {
    music::player_command(&state, cmd).map_err(map_err)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn report(State(state): State<Arc<AppState>>, Json(r): Json<PlayerReport>) -> ApiResult<Value> {
    music::player_report(&state, r);
    Ok(Json(json!({ "ok": true })))
}

pub async fn track(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(cmd): Json<TrackCommand>,
) -> ApiResult<Value> {
    music::track_command(&state, &id, cmd).map_err(map_err)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn get_settings(State(state): State<Arc<AppState>>) -> ApiResult<MusicConfig> {
    Ok(Json(state.store.config().music))
}

pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<MusicConfig>,
) -> ApiResult<MusicConfig> {
    let mut cfg = cfg;
    cfg.chat_command = cfg.chat_command.trim().to_string();
    if cfg.chat_command.is_empty() || cfg.chat_command.contains(char::is_whitespace) {
        return Err(ApiError::bad_request("команда должна быть одним словом, например !sr"));
    }
    cfg.max_duration_secs = cfg.max_duration_secs.clamp(30, 3600);
    cfg.mute_padding = cfg.mute_padding.clamp(0.0, 3.0);
    cfg.volume = cfg.volume.clamp(0.0, 1.0);
    // Мат нельзя «пропустить» на уровне оскорблений: слюры не пропускаем автоматически.
    if cfg.hate == music::track::Policy::Allow {
        cfg.hate = music::track::Policy::Review;
    }
    // Награда меняется только через /music/reward — устаревшая форма в панели её не затрёт.
    cfg.reward_id = state.store.config().music.reward_id;
    let saved = cfg.clone();
    state.store.update_config(|c| c.music = saved).map_err(ApiError::from)?;
    Ok(Json(cfg))
}

/// Звук трека для плеера (с поддержкой Range — перемотка и потоковая загрузка).
pub async fn media(State(state): State<Arc<AppState>>, Path(id): Path<String>, req: Request) -> Response {
    let Some(path) = state.music.track(&id).and_then(|t| t.audio_file) else {
        return (StatusCode::NOT_FOUND, "трек не найден").into_response();
    };
    // Отдаём только файлы из кеша музыки и из папок своей музыки.
    let dirs = state.store.config().music.local_dirs;
    if !path.starts_with(state.music.cache_dir()) && !music::library::contains(&dirs, &path) {
        return (StatusCode::FORBIDDEN, "нет доступа").into_response();
    }
    // У ServeFile ошибок не бывает (тип ошибки — Infallible).
    let Ok(resp) = ServeFile::new(path).oneshot(req).await;
    resp.into_response()
}

#[derive(Deserialize)]
pub struct RewardBody {
    title: String,
    cost: u32,
}

/// Создать или обновить награду «Заказать трек» на Twitch.
pub async fn put_reward(
    State(state): State<Arc<AppState>>,
    Json(b): Json<RewardBody>,
) -> ApiResult<MusicConfig> {
    let title = b.title.trim();
    if title.is_empty() || title.chars().count() > 45 {
        return Err(ApiError::bad_request("название награды — от 1 до 45 символов"));
    }
    let cfg = state.store.config().music;
    let prompt = "Ссылка или «исполнитель — название». Если трек не пройдёт проверку, баллы вернутся.";
    let id = crate::twitch::upsert_reward(&state, &cfg.reward_id, title, b.cost, prompt).await?;
    let title = title.to_string();
    state
        .store
        .update_config(|c| {
            c.music.reward_id = id;
            c.music.reward_title = title;
            c.music.reward_cost = b.cost.max(1);
        })
        .map_err(ApiError::from)?;
    Ok(Json(state.store.config().music))
}

pub async fn delete_reward(State(state): State<Arc<AppState>>) -> ApiResult<MusicConfig> {
    let id = state.store.config().music.reward_id;
    if !id.is_empty() {
        crate::twitch::delete_reward(&state, &id).await?;
    }
    state
        .store
        .update_config(|c| {
            c.music.reward_id.clear();
            c.music.reward_title.clear();
        })
        .map_err(ApiError::from)?;
    Ok(Json(state.store.config().music))
}

#[derive(Deserialize)]
pub struct ChannelBody {
    url: String,
}

/// Добавить разрешённый канал по ссылке на него или на любое его видео.
pub async fn add_safe_channel(
    State(state): State<Arc<AppState>>,
    Json(b): Json<ChannelBody>,
) -> ApiResult<MusicConfig> {
    let ch = state.music.ytdlp.resolve_channel(&state.http, b.url.trim()).await.map_err(map_err)?;
    state
        .store
        .update_config(|c| {
            if !c.music.safe_channels.iter().any(|x| x.channel_id == ch.channel_id) {
                c.music.safe_channels.push(ch);
            }
        })
        .map_err(ApiError::from)?;
    Ok(Json(state.store.config().music))
}
