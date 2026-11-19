//! Сборка приложения: общее состояние, фоновые подключения, HTTP-сервер.
//! Одинаково запускается из десктоп-оболочки (Tauri) и из консоли (headless).

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::bus::{ConnState, EventBus, StatusHub};
use crate::donationalerts::{self, DaService};
use crate::store::Store;
use crate::twitch::{self, TwitchService};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AppState {
    pub store: Store,
    pub bus: EventBus,
    pub status: StatusHub,
    pub http: reqwest::Client,
    pub twitch: TwitchService,
    pub da: DaService,
    pub music: crate::music::MusicService,
    pub analysis: Arc<crate::analysis::AnalysisService>,
}

/// Ошибка «нечего подключать»: нет ключей или не выполнен вход.
/// Супервизор не долбит повторами, а ждёт, пока пользователь что-то поменяет.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct NotConfigured(pub String);

/// Держит подключение живым: запускает `run`, при ошибке ждёт с нарастающей
/// паузой (2→4→…→60 с) и пробует снова. `restart` прерывает текущую попытку
/// и запускает новую сразу (после входа, выхода, смены ключей).
pub async fn supervise<F, Fut>(restart: &Notify, set_status: impl Fn(ConnState), mut run: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut backoff = 2u64;
    loop {
        set_status(ConnState::Connecting);
        let started = Instant::now();
        let result = tokio::select! {
            r = run() => Some(r),
            _ = restart.notified() => None,
        };
        let Some(result) = result else {
            backoff = 2;
            continue;
        };
        let err = match result {
            Ok(()) => anyhow::anyhow!("соединение завершилось"),
            Err(e) => e,
        };
        if let Some(nc) = err.downcast_ref::<NotConfigured>() {
            set_status(ConnState::NotConfigured { reason: nc.0.clone() });
            restart.notified().await;
            backoff = 2;
            continue;
        }
        // Если соединение долго жило — это новый обрыв, а не серия неудач.
        if started.elapsed() > Duration::from_secs(120) {
            backoff = 2;
        }
        tracing::warn!("{err:#} — повтор через {backoff} с");
        set_status(ConnState::Error { message: format!("{err:#}"), retry_in: backoff });
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            _ = restart.notified() => {}
        }
        backoff = (backoff * 2).min(60);
    }
}

#[derive(Debug, Default, Clone)]
pub struct StartOptions {
    /// Папка данных; по умолчанию — системная (`%APPDATA%`).
    pub data_dir: Option<PathBuf>,
    /// Временно занять другой порт (не сохраняется в настройках).
    pub port: Option<u16>,
    /// Адреса площадок (для тестов и mock-серверов); по умолчанию — из окружения.
    pub twitch_endpoints: Option<twitch::Endpoints>,
    pub da_endpoints: Option<donationalerts::Endpoints>,
}

pub struct Running {
    pub state: Arc<AppState>,
    pub addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl Running {
    /// Адрес панели с ключом доступа — его открывает окно приложения.
    pub fn panel_url(&self) -> String {
        format!("http://127.0.0.1:{}/?token={}", self.addr.port(), self.state.store.config().api_token)
    }

    pub fn shutdown(self) {
        for t in self.tasks {
            t.abort();
        }
    }
}

pub async fn start(opts: StartOptions) -> Result<Running> {
    let dir = match opts.data_dir {
        Some(d) => d,
        None => Store::default_dir()?,
    };
    let store = Store::open(&dir)?;
    tracing::info!("папка данных: {}", dir.display());

    let http = reqwest::Client::builder()
        .user_agent(concat!("Supstream/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;

    let store_volume = store.config().music.volume;
    let state = Arc::new(AppState {
        store,
        bus: EventBus::new(),
        status: StatusHub::new(),
        http,
        twitch: TwitchService::new(opts.twitch_endpoints.unwrap_or_else(twitch::Endpoints::from_env)),
        da: DaService::new(opts.da_endpoints.unwrap_or_else(donationalerts::Endpoints::from_env)),
        music: crate::music::MusicService::new(&dir, store_volume),
        analysis: Arc::new(crate::analysis::AnalysisService::new(&dir)),
    });
    crate::music::clean_cache(&state);
    crate::music::resume_pending(&state);

    let cfg = state.store.config();
    let port = opts.port.unwrap_or(cfg.server.port);
    let host = if cfg.server.allow_lan { [0, 0, 0, 0] } else { [127, 0, 0, 1] };
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((host, port))).await.with_context(|| {
            format!("порт {port} занят — возможно, Supstream уже запущен (или смените порт в config.json)")
        })?;
    let addr = listener.local_addr()?;

    let router = crate::server::router(state.clone());
    let server = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("HTTP-сервер остановился: {e}");
        }
    });

    let tasks = vec![
        server,
        tokio::spawn(twitch::supervise(state.clone())),
        tokio::spawn(donationalerts::supervise(state.clone())),
        tokio::spawn(crate::music::requests::run(state.clone())),
        {
            // Выгружаем модели анализа из памяти, если ими давно не пользовались.
            let st = state.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                loop {
                    tick.tick().await;
                    st.analysis.unload_if_idle();
                }
            })
        },
    ];

    tracing::info!("Supstream {VERSION} запущен на http://{addr}");
    Ok(Running { state, addr, tasks })
}
