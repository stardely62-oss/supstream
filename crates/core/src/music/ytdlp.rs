//! yt-dlp: поиск трека, данные о нём и загрузка звука.
//!
//! Бинарник берётся (по порядку): `SUPSTREAM_YTDLP` → `<данные>/bin/` → PATH →
//! скачивается с GitHub при первом заказе. Раз в сутки обновляется (`-U`):
//! YouTube регулярно ломает загрузку, и свежий yt-dlp — главное лекарство.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::process::Command;

use super::track::TrackMeta;

const UPDATE_EVERY: Duration = Duration::from_secs(24 * 3600);

fn asset_name() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "yt-dlp_macos"
    } else {
        "yt-dlp_linux"
    }
}

fn local_name() -> &'static str {
    if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" }
}

fn command(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // Без этого на каждом запуске мелькает чёрное окно консоли.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

pub struct YtDlp {
    bin_dir: PathBuf,
    lock: tokio::sync::Mutex<Option<PathBuf>>,
}

impl YtDlp {
    pub fn new(data_dir: &Path) -> Self {
        Self { bin_dir: data_dir.join("bin"), lock: tokio::sync::Mutex::new(None) }
    }

    /// Путь к yt-dlp; при необходимости скачивает его.
    pub async fn path(&self, http: &reqwest::Client) -> Result<PathBuf> {
        let mut cached = self.lock.lock().await;
        if let Some(p) = cached.as_ref() {
            return Ok(p.clone());
        }
        let found = if let Ok(p) = std::env::var("SUPSTREAM_YTDLP") {
            PathBuf::from(p)
        } else {
            let own = self.bin_dir.join(local_name());
            if own.is_file() {
                self.maybe_update(&own).await;
                own
            } else if let Some(p) = find_in_path(local_name()) {
                p
            } else {
                self.download(http, &own).await?;
                own
            }
        };
        *cached = Some(found.clone());
        Ok(found)
    }

    async fn download(&self, http: &reqwest::Client, dest: &Path) -> Result<()> {
        tracing::info!("скачиваю yt-dlp…");
        tokio::fs::create_dir_all(&self.bin_dir).await?;
        let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{}", asset_name());
        let bytes = http
            .get(&url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .context("не удалось скачать yt-dlp: GitHub недоступен")?
            .error_for_status()
            .context("не удалось скачать yt-dlp")?
            .bytes()
            .await?;
        let tmp = dest.with_extension("download");
        tokio::fs::write(&tmp, &bytes).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).await?;
        }
        tokio::fs::rename(&tmp, dest).await?;
        tracing::info!("yt-dlp установлен: {}", dest.display());
        Ok(())
    }

    async fn maybe_update(&self, path: &Path) {
        let stale = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > UPDATE_EVERY)
            .unwrap_or(false);
        if !stale {
            return;
        }
        let path = path.to_path_buf();
        tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(180), command(&path).arg("-U").output()).await {
                Ok(Ok(out)) => {
                    tracing::info!("yt-dlp -U: {}", String::from_utf8_lossy(&out.stdout).trim());
                    // Отмечаем проверку, даже если версия уже свежая.
                    let _ = filetime_touch(&path);
                }
                _ => tracing::warn!("не удалось обновить yt-dlp"),
            }
        });
    }

    /// Данные о треке по ссылке или поисковому запросу.
    pub async fn resolve(&self, http: &reqwest::Client, query: &str) -> Result<TrackMeta> {
        let target = if is_url(query) { query.to_string() } else { format!("ytsearch1:{query}") };
        parse_meta(&self.json(http, &target, &["--no-playlist"]).await?)
    }

    /// Поиск только внутри канала (безопасный режим): `youtube.com/channel/UC…/search?query=…`.
    pub async fn search_in_channel(
        &self,
        http: &reqwest::Client,
        channel_id: &str,
        query: &str,
    ) -> Result<TrackMeta> {
        let mut url = url::Url::parse(&format!("https://www.youtube.com/channel/{channel_id}/search"))?;
        url.query_pairs_mut().append_pair("query", query);
        parse_meta(&self.json(http, url.as_str(), &["--playlist-items", "1"]).await?)
    }

    /// Канал по ссылке на него или на любое его видео.
    pub async fn resolve_channel(
        &self,
        http: &reqwest::Client,
        link: &str,
    ) -> Result<super::track::SafeChannel> {
        if !is_url(link) {
            bail!("нужна ссылка на канал или видео");
        }
        let v = self.json(http, link, &["--playlist-items", "1"]).await?;
        parse_channel(&v, link)
    }

    async fn json(&self, http: &reqwest::Client, target: &str, extra: &[&str]) -> Result<Value> {
        let bin = self.path(http).await?;
        let out = tokio::time::timeout(
            Duration::from_secs(60),
            command(&bin).arg("-J").arg("--no-warnings").args(extra).arg("--").arg(target).output(),
        )
        .await
        .context("поиск трека занял слишком много времени")?
        .context("не удалось запустить yt-dlp")?;
        if !out.status.success() {
            bail!("{}", short_error(&out.stderr));
        }
        serde_json::from_slice(&out.stdout).context("yt-dlp: непонятный ответ")
    }

    /// Скачивает звук в `dir`, возвращает путь к файлу.
    pub async fn download_audio(
        &self,
        http: &reqwest::Client,
        meta: &TrackMeta,
        dir: &Path,
    ) -> Result<PathBuf> {
        let bin = self.path(http).await?;
        tokio::fs::create_dir_all(dir).await?;
        let template = dir.join(format!("{}.%(ext)s", safe_id(&meta.video_id)));
        let out = tokio::time::timeout(
            Duration::from_secs(300),
            command(&bin)
                .args([
                    "--no-playlist",
                    "--no-warnings",
                    "--no-progress",
                    // Opus/WebM гарантированно играет в браузере OBS (там нет AAC/H.264).
                    "-f",
                    "bestaudio[acodec=opus]/bestaudio[ext=webm]/bestaudio",
                    "--print",
                    "after_move:filepath",
                    "-o",
                ])
                .arg(&template)
                .arg("--")
                .arg(&meta.url)
                .output(),
        )
        .await
        .context("загрузка трека заняла слишком много времени")?
        .context("не удалось запустить yt-dlp")?;
        if !out.status.success() {
            bail!("{}", short_error(&out.stderr));
        }
        let path = String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or_default().trim().to_string();
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("yt-dlp не сообщил, куда сохранил файл");
        }
        Ok(path)
    }
}

fn filetime_touch(path: &Path) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.set_modified(std::time::SystemTime::now())
}

pub fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

pub fn parse_channel(v: &Value, link: &str) -> Result<super::track::SafeChannel> {
    let first = v.get("entries").and_then(Value::as_array).and_then(|e| e.first());
    let pick = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .or_else(|| first.and_then(|f| f.get(k)).and_then(Value::as_str))
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    let channel_id = pick("channel_id").context("не удалось определить канал по ссылке")?;
    let name = pick("channel").or_else(|| pick("uploader")).unwrap_or_else(|| channel_id.clone());
    Ok(super::track::SafeChannel { channel_id, name, url: link.to_string() })
}

fn safe_id(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn short_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line =
        text.lines().rev().find(|l| l.contains("ERROR")).or_else(|| text.lines().last()).unwrap_or("").trim();
    let line = line.trim_start_matches("ERROR:").trim();
    if line.is_empty() {
        "yt-dlp завершился с ошибкой".into()
    } else {
        format!("yt-dlp: {line}")
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

pub fn parse_meta(v: &Value) -> Result<TrackMeta> {
    let v = if v.get("_type").and_then(Value::as_str) == Some("playlist") {
        v.get("entries").and_then(Value::as_array).and_then(|e| e.first()).context("ничего не найдено")?
    } else {
        v
    };
    if v.get("is_live").and_then(Value::as_bool) == Some(true) {
        bail!("это прямая трансляция, а не трек");
    }
    let title = s(v, "title");
    let (artist, track_title) = match (
        v.get("artist").or(v.get("creator")).and_then(Value::as_str),
        v.get("track").and_then(Value::as_str),
    ) {
        (Some(a), Some(t)) if !a.is_empty() && !t.is_empty() => {
            // «Artist1, Artist2» → берём первого: так точнее ищется текст.
            (a.split(", ").next().unwrap_or(a).to_string(), t.to_string())
        }
        _ => super::lyrics::guess_artist_title(&title, &s(v, "uploader")),
    };
    Ok(TrackMeta {
        video_id: s(v, "id"),
        url: v
            .get("webpage_url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| s(v, "original_url")),
        title,
        artist,
        track_title,
        duration: v.get("duration").and_then(Value::as_f64),
        thumbnail: v.get("thumbnail").and_then(Value::as_str).map(str::to_string),
        extractor: s(v, "extractor_key"),
        channel_id: v.get("channel_id").and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string),
        channel: v
            .get("channel")
            .or(v.get("uploader"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        local: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_search_result_and_prefers_music_metadata() {
        let v = json!({
            "_type": "playlist",
            "entries": [{
                "id": "kXYiU_JCYtU", "title": "Numb [Official Music Video] - Linkin Park",
                "webpage_url": "https://www.youtube.com/watch?v=kXYiU_JCYtU",
                "duration": 187.0, "uploader": "Linkin Park", "extractor_key": "Youtube",
                "artist": "Linkin Park", "track": "Numb"
            }]
        });
        let m = parse_meta(&v).unwrap();
        assert_eq!(m.artist, "Linkin Park");
        assert_eq!(m.track_title, "Numb");
        assert_eq!(m.duration, Some(187.0));
    }

    #[test]
    fn falls_back_to_title_parsing_and_rejects_live() {
        let v = json!({ "id": "x", "title": "Кино - Звезда по имени Солнце (Official Video)", "uploader": "kino", "webpage_url": "u" });
        let m = parse_meta(&v).unwrap();
        assert_eq!((m.artist.as_str(), m.track_title.as_str()), ("Кино", "Звезда по имени Солнце"));
        assert!(parse_meta(&json!({ "id": "x", "is_live": true })).is_err());
        assert!(parse_meta(&json!({ "_type": "playlist", "entries": [] })).is_err());
    }

    #[test]
    fn channel_from_video_or_channel_page() {
        let video = json!({ "id": "v", "channel_id": "UC123", "channel": "NoCopyrightSounds" });
        let c = parse_channel(&video, "https://youtu.be/v").unwrap();
        assert_eq!((c.channel_id.as_str(), c.name.as_str()), ("UC123", "NoCopyrightSounds"));
        let page =
            json!({ "_type": "playlist", "entries": [{ "channel_id": "UC9", "uploader": "StreamBeats" }] });
        assert_eq!(parse_channel(&page, "x").unwrap().name, "StreamBeats");
        assert!(parse_channel(&json!({ "id": "x" }), "x").is_err());
    }

    #[test]
    fn error_is_short() {
        let err = b"[youtube] abc: Downloading webpage\nERROR: [youtube] abc: Video unavailable\n";
        assert_eq!(short_error(err), "yt-dlp: [youtube] abc: Video unavailable");
    }
}
