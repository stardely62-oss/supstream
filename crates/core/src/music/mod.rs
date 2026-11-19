//! Музыка: очередь заказов, проверка треков, управление плеером.
//!
//! Звук играет не приложение, а виджет-плеер в OBS (browser source) — так музыка
//! становится отдельным источником звука, и её легко убрать из записи (Twitch VOD Track).
//! Ядро — источник истины: что играет, пауза, громкость, отметки глушения.

pub mod check;
pub mod filter;
pub mod lyrics;
pub mod requests;
pub mod track;
pub mod ytdlp;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, watch};

pub use track::MusicConfig;
use track::{Mute, Requester, Status, Track, Via};

use crate::app::AppState;

const HISTORY_KEEP: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub current: Option<String>,
    pub paused: bool,
    pub position: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Тревожная кнопка: звук музыки выключен немедленно.
    pub panic: bool,
    pub volume: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    tracks: Vec<Track>,
    paused: bool,
}

struct Inner {
    tracks: Vec<Track>,
    player: PlayerState,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Progress {
    pub track_id: Option<String>,
    pub position: f64,
    pub duration: Option<f64>,
}

pub struct MusicService {
    inner: std::sync::Mutex<Inner>,
    rev: watch::Sender<u64>,
    /// Позиция воспроизведения — отдельно, чтобы не гонять всю очередь раз в секунду.
    progress: watch::Sender<Progress>,
    players: AtomicUsize,
    work: Semaphore,
    pub ytdlp: ytdlp::YtDlp,
    pub lrclib: lyrics::Lrclib,
    dir: PathBuf,
}

/// То, что видят панель и виджет.
#[derive(Debug, Clone, Serialize)]
pub struct MusicView {
    pub player: PlayerState,
    pub players_connected: usize,
    pub now: Option<Track>,
    pub queue: Vec<Track>,
    pub history: Vec<Track>,
}

impl MusicService {
    pub fn new(data_dir: &std::path::Path, volume: f64) -> Self {
        let dir = data_dir.join("music");
        let saved: Saved = std::fs::read(dir.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut tracks = saved.tracks;
        for t in &mut tracks {
            match t.status {
                // Прерванный трек вернётся в начало очереди.
                Status::Playing => t.status = Status::Ready,
                // Не успели проверить — проверим заново (см. resume_pending).
                Status::Checking => t.status = Status::Resolving,
                _ => {}
            }
            if t.audio_file.as_ref().is_some_and(|p| !p.is_file()) && t.status == Status::Ready {
                t.audio_file = None;
                t.status = Status::Resolving;
            }
        }
        Self {
            inner: std::sync::Mutex::new(Inner {
                tracks,
                player: PlayerState {
                    current: None,
                    paused: saved.paused,
                    position: 0.0,
                    duration: None,
                    panic: false,
                    volume,
                },
            }),
            rev: watch::channel(0).0,
            progress: watch::channel(Progress::default()).0,
            players: AtomicUsize::new(0),
            work: Semaphore::new(2),
            ytdlp: ytdlp::YtDlp::new(data_dir),
            lrclib: lyrics::Lrclib::from_env(),
            dir,
        }
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.rev.subscribe()
    }

    pub fn subscribe_progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }

    pub fn view(&self) -> MusicView {
        let inner = self.inner.lock().unwrap();
        let now =
            inner.player.current.as_ref().and_then(|id| inner.tracks.iter().find(|t| &t.id == id)).cloned();
        let queue = inner
            .tracks
            .iter()
            .filter(|t| t.status.is_active() && Some(&t.id) != inner.player.current.as_ref())
            .cloned()
            .collect();
        let mut history: Vec<Track> =
            inner.tracks.iter().filter(|t| t.status.is_finished()).cloned().collect();
        history.reverse();
        MusicView {
            player: inner.player.clone(),
            players_connected: self.players.load(Ordering::Relaxed),
            now,
            queue,
            history,
        }
    }

    pub fn track(&self, id: &str) -> Option<Track> {
        self.inner.lock().unwrap().tracks.iter().find(|t| t.id == id).cloned()
    }

    /// Изменить состояние под блокировкой, сохранить и оповестить подписчиков.
    fn mutate<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let r;
        let snapshot;
        {
            let mut inner = self.inner.lock().unwrap();
            r = f(&mut inner);
            // Держим ограниченную историю.
            let finished: Vec<usize> = inner
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.status.is_finished())
                .map(|(i, _)| i)
                .collect();
            if finished.len() > HISTORY_KEEP {
                let drop: Vec<usize> = finished[..finished.len() - HISTORY_KEEP].to_vec();
                let mut i = 0;
                inner.tracks.retain(|_| {
                    let keep = !drop.contains(&i);
                    i += 1;
                    keep
                });
            }
            snapshot = Saved { tracks: inner.tracks.clone(), paused: inner.player.paused };
        }
        if let Err(e) = save(&self.dir, &snapshot) {
            tracing::warn!("музыка: не удалось сохранить очередь: {e:#}");
        }
        self.rev.send_modify(|r| *r += 1);
        r
    }

    fn update_track(&self, id: &str, f: impl FnOnce(&mut Track)) -> bool {
        self.mutate(|inner| match inner.tracks.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                f(t);
                true
            }
            None => false,
        })
    }

    pub fn player_connected(&self, delta: isize) {
        if delta > 0 {
            self.players.fetch_add(1, Ordering::Relaxed);
        } else {
            self.players.fetch_sub(1, Ordering::Relaxed);
        }
        self.rev.send_modify(|r| *r += 1);
    }
}

fn save(dir: &std::path::Path, saved: &Saved) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("state.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(saved)?)?;
    std::fs::rename(tmp, dir.join("state.json"))?;
    Ok(())
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..10].to_string()
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

/// Новый заказ. Проверка и загрузка идут в фоне.
pub fn request(state: &Arc<AppState>, query: &str, requester: Requester) -> Result<Track> {
    let cfg = state.store.config().music;
    let query = query.trim();
    if !cfg.enabled {
        bail!(Refused("заказ музыки выключен".into()));
    }
    if query.is_empty() {
        bail!(Refused("укажите ссылку или название трека".into()));
    }
    if query.chars().count() > 300 {
        bail!(Refused("слишком длинный запрос".into()));
    }
    let music = &state.music;
    if requester.via != Via::Panel
        && let Some(uid) = &requester.user_id
    {
        let active = music
            .inner
            .lock()
            .unwrap()
            .tracks
            .iter()
            .filter(|t| t.status.is_active() && t.requested_by.user_id.as_ref() == Some(uid))
            .count();
        if active >= cfg.per_user_limit as usize {
            bail!(Refused(format!("у вас уже {active} трек(а) в очереди — дождитесь их")));
        }
    }
    let track = Track {
        id: short_id(),
        query: query.to_string(),
        requested_by: requester,
        requested_at: chrono::Utc::now(),
        status: Status::Resolving,
        error: None,
        meta: None,
        audio_file: None,
        check: None,
        mutes: Vec::new(),
        approved: false,
    };
    music.mutate(|inner| inner.tracks.push(track.clone()));
    spawn_process(state, track.id.clone());
    Ok(track)
}

/// Проверить треки, прерванные прошлым выключением приложения.
pub fn resume_pending(state: &Arc<AppState>) {
    let ids: Vec<String> = state
        .music
        .inner
        .lock()
        .unwrap()
        .tracks
        .iter()
        .filter(|t| t.status == Status::Resolving)
        .map(|t| t.id.clone())
        .collect();
    for id in ids {
        spawn_process(state, id);
    }
    maybe_start(state);
}

fn spawn_process(state: &Arc<AppState>, id: String) {
    let state = state.clone();
    tokio::spawn(async move {
        let result = process(&state, &id).await;
        if let Err(e) = result {
            let msg = format!("{e:#}");
            tracing::warn!("музыка: трек {id}: {msg}");
            let refused = e.downcast_ref::<Refused>().is_some();
            state.music.update_track(&id, |t| {
                t.status = if refused { Status::Rejected } else { Status::Failed };
                t.error = Some(msg.clone());
            });
            requests::notify_result(&state, &id);
        }
        maybe_start(&state);
    });
}

async fn process(state: &Arc<AppState>, id: &str) -> Result<()> {
    let _permit = state.music.work.acquire().await?;
    let music = &state.music;
    let Some(track) = music.track(id) else { return Ok(()) };
    let cfg = state.store.config().music;

    let meta = music.ytdlp.resolve(&state.http, &track.query).await?;
    if let Some(d) = meta.duration
        && d > cfg.max_duration_secs as f64
    {
        bail!(Refused(format!(
            "трек длиннее {} мин ({} мин)",
            cfg.max_duration_secs / 60,
            (d / 60.0).round()
        )));
    }
    let dup = music.inner.lock().unwrap().tracks.iter().any(|t| {
        t.id != id && t.status.is_active() && t.meta.as_ref().is_some_and(|m| m.video_id == meta.video_id)
    });
    if dup {
        bail!(Refused("этот трек уже в очереди".into()));
    }
    music.update_track(id, |t| {
        t.meta = Some(meta.clone());
        t.status = Status::Checking;
    });

    // Загрузка и поиск текста параллельно.
    let cache = music.cache_dir();
    let (audio, lyrics) = tokio::join!(
        music.ytdlp.download_audio(&state.http, &meta, &cache),
        music.lrclib.find(&state.http, &meta.artist, &meta.track_title, meta.duration),
    );
    let audio = audio?;
    let lyrics = lyrics.unwrap_or_else(|e| {
        tracing::warn!("музыка: тексты недоступны: {e:#}");
        None
    });

    let filter = filter::Filter::with_custom(&cfg.custom_rules, &cfg.allow_words);
    let outcome = check::evaluate(lyrics.as_ref(), meta.duration, &filter, &cfg);
    music.update_track(id, |t| {
        t.audio_file = Some(audio);
        t.check = Some(outcome.report);
        t.mutes = outcome.mutes;
        // Пока проверяли, стример мог уже одобрить или убрать трек.
        if t.status == Status::Checking {
            t.status = if t.approved && outcome.status == Status::NeedsReview {
                Status::Ready
            } else {
                outcome.status
            };
        }
    });
    requests::notify_result(state, id);
    Ok(())
}

/// Если ничего не играет и не пауза — запускаем следующий готовый трек.
pub fn maybe_start(state: &Arc<AppState>) {
    state.music.mutate(|inner| {
        if inner.player.current.is_some() || inner.player.paused {
            return;
        }
        if let Some(t) = inner.tracks.iter_mut().find(|t| t.status == Status::Ready) {
            t.status = Status::Playing;
            inner.player.current = Some(t.id.clone());
            inner.player.position = 0.0;
            inner.player.duration = t.meta.as_ref().and_then(|m| m.duration);
        }
    });
}

fn finish_current(inner: &mut Inner, status: Status) {
    if let Some(id) = inner.player.current.take()
        && let Some(t) = inner.tracks.iter_mut().find(|t| t.id == id)
    {
        t.status = status;
    }
    inner.player.position = 0.0;
    inner.player.duration = None;
}

#[derive(Debug, Deserialize)]
pub struct PlayerReport {
    pub track_id: String,
    #[serde(default)]
    pub position: Option<f64>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Отчёт виджета-плеера. Если плееров несколько (OBS + предпросмотр),
/// «трек закончился» засчитывается один раз — по совпадению id.
pub fn player_report(state: &Arc<AppState>, r: PlayerReport) {
    let music = &state.music;
    if r.error.is_none() && !r.ended {
        // Обычный отчёт о позиции: без сохранения на диск и без рассылки очереди.
        let mut inner = music.inner.lock().unwrap();
        if inner.player.current.as_deref() != Some(r.track_id.as_str()) {
            return;
        }
        if let Some(p) = r.position {
            inner.player.position = p;
        }
        if r.duration.is_some() {
            inner.player.duration = r.duration;
        }
        let progress = Progress {
            track_id: inner.player.current.clone(),
            position: inner.player.position,
            duration: inner.player.duration,
        };
        drop(inner);
        music.progress.send_if_modified(|p| {
            let changed = *p != progress;
            *p = progress;
            changed
        });
        return;
    }
    let advanced = music.mutate(|inner| {
        if inner.player.current.as_deref() != Some(r.track_id.as_str()) {
            return false;
        }
        if let Some(err) = &r.error {
            tracing::warn!("плеер: ошибка воспроизведения: {err}");
            if let Some(t) = inner.tracks.iter_mut().find(|t| t.id == r.track_id) {
                t.error = Some(format!("плеер: {err}"));
            }
            finish_current(inner, Status::Failed);
        } else {
            finish_current(inner, Status::Played);
        }
        true
    });
    if advanced {
        maybe_start(state);
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlayerCommand {
    Play,
    Pause,
    Skip,
    Panic { on: bool },
    Volume { value: f64 },
}

pub fn player_command(state: &Arc<AppState>, cmd: PlayerCommand) -> Result<()> {
    match cmd {
        PlayerCommand::Play => state.music.mutate(|i| {
            i.player.paused = false;
            i.player.panic = false;
        }),
        PlayerCommand::Pause => state.music.mutate(|i| i.player.paused = true),
        PlayerCommand::Skip => state.music.mutate(|i| finish_current(i, Status::Skipped)),
        PlayerCommand::Panic { on } => state.music.mutate(|i| {
            i.player.panic = on;
            if on {
                i.player.paused = true;
            }
        }),
        PlayerCommand::Volume { value } => {
            let v = value.clamp(0.0, 1.0);
            state.music.mutate(|i| i.player.volume = v);
            state.store.update_config(|c| c.music.volume = v)?;
        }
    }
    maybe_start(state);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TrackCommand {
    /// Пропустить в эфир (с текущими отметками глушения).
    Approve,
    Reject,
    Remove,
    /// Поставить на позицию в очереди (0 — следующим).
    Move {
        to: usize,
    },
    /// Включить/выключить отметку глушения.
    SetMute {
        mute_id: u32,
        enabled: bool,
    },
    /// Добавить свою отметку (например, заглушить всю строку).
    AddMute {
        start: f64,
        end: f64,
        label: String,
    },
    RemoveMute {
        mute_id: u32,
    },
    /// Проверить заново (например, после правки списка слов).
    Recheck,
}

pub fn track_command(state: &Arc<AppState>, id: &str, cmd: TrackCommand) -> Result<()> {
    let music = &state.music;
    let Some(track) = music.track(id) else { bail!(Refused("трек не найден".into())) };
    match cmd {
        TrackCommand::Approve => {
            if track.status == Status::Resolving || track.status == Status::Checking {
                // Одобрим заранее — после проверки трек сразу станет готовым.
                music.update_track(id, |t| t.approved = true);
            } else if track.audio_file.is_none() {
                bail!(Refused("звук трека не загружен — одобрить нельзя".into()));
            } else {
                music.update_track(id, |t| {
                    t.approved = true;
                    if matches!(t.status, Status::NeedsReview | Status::Rejected) {
                        t.status = Status::Ready;
                    }
                });
            }
        }
        TrackCommand::Reject => {
            music.mutate(|inner| {
                if inner.player.current.as_deref() == Some(id) {
                    finish_current(inner, Status::Rejected);
                } else if let Some(t) = inner.tracks.iter_mut().find(|t| t.id == id) {
                    t.status = Status::Rejected;
                }
            });
        }
        TrackCommand::Remove => {
            music.mutate(|inner| {
                if inner.player.current.as_deref() == Some(id) {
                    finish_current(inner, Status::Skipped);
                }
                inner.tracks.retain(|t| t.id != id);
            });
        }
        TrackCommand::Move { to } => {
            music.mutate(|inner| {
                let Some(from) = inner.tracks.iter().position(|t| t.id == id) else { return };
                let t = inner.tracks.remove(from);
                // `to` — позиция среди активных треков (кроме играющего).
                let current = inner.player.current.clone();
                let active: Vec<usize> = inner
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.status.is_active() && Some(&x.id) != current.as_ref())
                    .map(|(i, _)| i)
                    .collect();
                let idx = active.get(to).copied().unwrap_or(inner.tracks.len());
                inner.tracks.insert(idx, t);
            });
        }
        TrackCommand::SetMute { mute_id, enabled } => {
            music.update_track(id, |t| {
                if let Some(m) = t.mutes.iter_mut().find(|m| m.id == mute_id) {
                    m.enabled = enabled;
                }
            });
        }
        TrackCommand::AddMute { start, end, label } => {
            if !(start.is_finite() && end.is_finite() && end > start && start >= 0.0) {
                bail!(Refused("неверный интервал".into()));
            }
            music.update_track(id, |t| {
                let next = t.mutes.iter().map(|m| m.id).max().unwrap_or(0) + 1;
                t.mutes.push(Mute {
                    id: next,
                    start,
                    end,
                    label,
                    category: None,
                    auto: false,
                    enabled: true,
                });
                t.mutes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
            });
        }
        TrackCommand::RemoveMute { mute_id } => {
            music.update_track(id, |t| t.mutes.retain(|m| m.id != mute_id || m.auto));
        }
        TrackCommand::Recheck => {
            if !matches!(
                track.status,
                Status::Ready | Status::NeedsReview | Status::Rejected | Status::Failed
            ) {
                bail!(Refused("трек сейчас нельзя проверить заново".into()));
            }
            music.update_track(id, |t| {
                t.status = Status::Resolving;
                t.error = None;
                t.approved = false;
            });
            spawn_process(state, id.to_string());
        }
    }
    maybe_start(state);
    Ok(())
}

/// Удаляет из кеша файлы, на которые не ссылается ни один трек.
pub fn clean_cache(state: &AppState) {
    let used: Vec<PathBuf> =
        state.music.inner.lock().unwrap().tracks.iter().filter_map(|t| t.audio_file.clone()).collect();
    let Ok(dir) = std::fs::read_dir(state.music.cache_dir()) else { return };
    for entry in dir.flatten() {
        let p = entry.path();
        if p.is_file() && !used.contains(&p) {
            let _ = std::fs::remove_file(&p);
        }
    }
}
