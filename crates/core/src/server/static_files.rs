//! Панель управления (Svelte, `ui/dist`) вшивается в exe при release-сборке.
//! В debug rust-embed читает файлы с диска — достаточно пересобрать UI.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../ui/dist"]
struct Panel;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(resp) = file(path) {
        return resp;
    }
    // Одностраничное приложение: неизвестный путь без расширения → index.html.
    if !path.contains('.')
        && let Some(resp) = file("index.html")
    {
        return resp;
    }
    (StatusCode::NOT_FOUND, "не найдено").into_response()
}

fn file(path: &str) -> Option<Response> {
    let f = Panel::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable" // у сборки Vite в именах хэши
    } else {
        "no-cache"
    };
    Some(
        (
            [(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.into())],
            f.data.into_owned(),
        )
            .into_response(),
    )
}
