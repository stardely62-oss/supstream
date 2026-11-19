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
    let saved = cfg.clone();
    state.store.update_config(|c| c.music = saved).map_err(ApiError::from)?;
    Ok(Json(cfg))
}

/// Звук трека для плеера (с поддержкой Range — перемотка и потоковая загрузка).
pub async fn media(State(state): State<Arc<AppState>>, Path(id): Path<String>, req: Request) -> Response {
    let Some(path) = state.music.track(&id).and_then(|t| t.audio_file) else {
        return (StatusCode::NOT_FOUND, "трек не найден").into_response();
    };
    // Отдаём только файлы из кеша музыки.
    if !path.starts_with(state.music.cache_dir()) {
        return (StatusCode::FORBIDDEN, "нет доступа").into_response();
    }
    // У ServeFile ошибок не бывает (тип ошибки — Infallible).
    let Ok(resp) = ServeFile::new(path).oneshot(req).await;
    resp.into_response()
}
