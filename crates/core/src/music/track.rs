//! Трек, его статусы, отметки глушения и настройки музыки.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::filter::{Category, Hit, Rule};

/// Что делать, если в треке найдено нарушение (или не найден текст).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// Пропустить как есть.
    Allow,
    /// Заглушить проблемные места (нужен текст с таймкодами).
    Mute,
    /// Отправить на ручную проверку стримеру.
    Review,
    /// Отклонить трек.
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    Everyone,
    Subscriber,
    Vip,
    Moderator,
}

/// Откуда можно брать музыку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    /// Любой трек. Риск жалоб правообладателей (DMCA) — на совести стримера.
    #[default]
    Any,
    /// Только разрешённые каналы и своя папка с музыкой.
    Safe,
}

/// Разрешённый в безопасном режиме канал (например, StreamBeats или NoCopyrightSounds).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafeChannel {
    /// ID канала YouTube (UC…) — уникален, в отличие от названия.
    pub channel_id: String,
    pub name: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicConfig {
    pub enabled: bool,
    pub max_duration_secs: u32,
    /// Сколько треков один зритель может держать в очереди одновременно.
    pub per_user_limit: u32,
    pub hate: Policy,
    pub profanity: Policy,
    pub no_lyrics: Policy,
    /// Запас вокруг заглушаемого слова, сек. Пока позиция слова оценивается
    /// по строке — запас большой; с выравниванием по звуку станет меньше.
    pub mute_padding: f64,
    pub chat_enabled: bool,
    pub chat_command: String,
    pub chat_min_role: ChatRole,
    /// Отвечать в чат «трек добавлен / отклонён».
    pub chat_replies: bool,
    /// Название награды за баллы, которая заказывает трек (пусто — выключено).
    pub reward_title: String,
    /// Заказ донатом от этой суммы (0 — выключено). Текст доната = запрос.
    pub donation_min_amount: f64,
    pub volume: f64,
    pub custom_rules: Vec<Rule>,
    pub allow_words: Vec<String>,
    pub source_mode: SourceMode,
    pub safe_channels: Vec<SafeChannel>,
    /// Папки со своей музыкой (на которую есть права). Ищутся первыми в любом режиме.
    pub local_dirs: Vec<String>,
    /// Награда, созданная приложением: только для неё Twitch даёт вернуть баллы.
    pub reward_id: String,
    pub reward_cost: u32,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_duration_secs: 420,
            per_user_limit: 2,
            hate: Policy::Reject,
            profanity: Policy::Mute,
            no_lyrics: Policy::Review,
            mute_padding: 0.7,
            chat_enabled: true,
            chat_command: "!sr".into(),
            chat_min_role: ChatRole::Everyone,
            chat_replies: true,
            reward_title: String::new(),
            donation_min_amount: 0.0,
            volume: 0.5,
            custom_rules: Vec::new(),
            allow_words: Vec::new(),
            source_mode: SourceMode::Any,
            safe_channels: Vec::new(),
            local_dirs: Vec::new(),
            reward_id: String::new(),
            reward_cost: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Panel,
    Chat,
    Reward,
    Donation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requester {
    pub name: String,
    pub via: Via,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Для награды: (reward_id, redemption_id) — чтобы вернуть баллы при отказе.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redemption: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ищем трек и его данные.
    Resolving,
    /// Скачиваем звук и проверяем текст.
    Checking,
    Ready,
    NeedsReview,
    Rejected,
    Failed,
    Playing,
    Played,
    Skipped,
}

impl Status {
    /// Трек ещё в очереди (не доигран, не отклонён).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Resolving | Self::Checking | Self::Ready | Self::NeedsReview | Self::Playing)
    }

    pub fn is_finished(self) -> bool {
        !self.is_active()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMeta {
    pub video_id: String,
    pub url: String,
    pub title: String,
    pub artist: String,
    pub track_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub extractor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub channel: String,
    /// Трек из своей папки, а не из интернета.
    #[serde(default)]
    pub local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mute {
    pub id: u32,
    pub start: f64,
    pub end: f64,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    /// Создано проверкой (а не стримером вручную).
    pub auto: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsInfo {
    pub source: String,
    pub synced: bool,
    pub instrumental: bool,
    pub matched_artist: String,
    pub matched_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lyrics: Option<LyricsInfo>,
    pub lines: Vec<ReportLine>,
    pub hate: usize,
    pub profanity: usize,
    /// Почему такой итог — показываем стримеру.
    pub reasons: Vec<String>,
    /// Точность отметок глушения: `line_estimate` — позиция слова оценена по строке.
    pub precision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub query: String,
    pub requested_by: Requester,
    pub requested_at: DateTime<Utc>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<TrackMeta>,
    #[serde(default, skip_serializing)]
    pub audio_file: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckReport>,
    #[serde(default)]
    pub mutes: Vec<Mute>,
    /// Стример вручную одобрил трек.
    #[serde(default)]
    pub approved: bool,
    /// Активация награды уже подтверждена или отменена на Twitch.
    #[serde(default)]
    pub settled: bool,
}

impl Track {
    pub fn display_title(&self) -> String {
        match &self.meta {
            Some(m) if !m.artist.is_empty() => format!("{} — {}", m.artist, m.track_title),
            Some(m) => m.title.clone(),
            None => self.query.clone(),
        }
    }
}
