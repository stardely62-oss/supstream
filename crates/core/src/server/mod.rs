//! Локальный HTTP-сервер: API, WebSocket-шина, панель и (позже) виджеты для OBS.
//!
//! Все `/api/*` требуют ключ: заголовок `Authorization: Bearer <token>`
//! или параметр `?token=<token>` (для WebSocket и browser source в OBS).

mod api;
mod static_files;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::app::{AppState, NotConfigured};

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/status", get(api::status))
        .route("/events", get(api::recent_events).post(api::publish_event))
        .route("/events/test", post(api::test_event))
        .route("/ws", get(ws::handler))
        .route("/settings", get(api::get_settings).put(api::put_settings))
        .route("/auth/twitch/login", post(api::twitch_login))
        .route("/auth/twitch/logout", post(api::twitch_logout))
        .route("/auth/donationalerts/login", post(api::da_login))
        .route("/auth/donationalerts/logout", post(api::da_logout))
        .route("/system/open", post(api::open_target))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        // Ключ не передаётся браузером автоматически, поэтому открытый CORS
        // безопасен и позволяет внешним скриптам и виджетам ходить в API.
        .layer(CorsLayer::permissive());

    Router::new()
        .nest("/api", api)
        .route("/auth/donationalerts/callback", get(api::da_callback))
        .fallback(static_files::serve)
        .with_state(state)
}

async fn require_token(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if req.method() == axum::http::Method::OPTIONS {
        return next.run(req).await;
    }
    let expected = state.store.config().api_token;
    let from_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let from_query = req.uri().query().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == "token").map(|(_, v)| v.into_owned())
    });
    let ok = from_header.map(|t| ct_eq(t, &expected)).unwrap_or(false)
        || from_query.map(|t| ct_eq(&t, &expected)).unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, axum::Json(json!({ "error": "нужен ключ доступа (token)" })))
            .into_response()
    }
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Ошибка API → JSON `{ "error": "..." }`.
pub struct ApiError(StatusCode, String);

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        if let Some(nc) = e.downcast_ref::<NotConfigured>() {
            return Self(StatusCode::CONFLICT, nc.0.clone());
        }
        tracing::warn!("ошибка API: {e:#}");
        Self(StatusCode::BAD_GATEWAY, format!("{e:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(json!({ "error": self.1 }))).into_response()
    }
}

pub type ApiResult<T> = Result<axum::Json<T>, ApiError>;
