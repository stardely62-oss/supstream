//! Тексты песен: поиск в LRCLIB (бесплатная открытая база, есть тексты с таймкодами)
//! и разбор формата LRC.
//! API: https://lrclib.net/docs

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LyricLine {
    /// Начало строки в секундах (`None` — текст без таймкодов).
    pub time: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lyrics {
    pub source: String,
    pub synced: bool,
    pub instrumental: bool,
    pub lines: Vec<LyricLine>,
    /// Что нашли (для проверки стримером, тот ли это трек).
    pub matched_artist: String,
    pub matched_title: String,
}

/// Разбор LRC: `[01:23.45] текст`, несколько меток на строке, служебные теги `[ar:…]` пропускаются.
pub fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    let mut out = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(close) = stripped.find(']') else { break };
            if let Some(t) = parse_timestamp(&stripped[..close]) {
                times.push(t);
            }
            rest = stripped[close + 1..].trim_start();
        }
        let text = rest.trim().to_string();
        for t in times {
            out.push(LyricLine { time: Some(t), text: text.clone() });
        }
    }
    out.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn parse_timestamp(s: &str) -> Option<f64> {
    let (m, rest) = s.split_once(':')?;
    let minutes: f64 = m.trim().parse().ok()?;
    let seconds: f64 = rest.trim().replace(',', ".").parse().ok()?;
    (minutes >= 0.0 && (0.0..60.0).contains(&seconds)).then_some(minutes * 60.0 + seconds)
}

pub fn parse_plain(text: &str) -> Vec<LyricLine> {
    text.lines().map(|l| LyricLine { time: None, text: l.trim().to_string() }).collect()
}

/// Исполнитель и название из заголовка видео:
/// «Artist - Title (Official Video) [4K]» → ("Artist", "Title").
pub fn guess_artist_title(raw_title: &str, uploader: &str) -> (String, String) {
    // Убираем группы в скобках с «шумом» (official video, lyrics, prod. …).
    // «(feat. X)» и «(Remix)» — часть названия, их оставляем.
    let noise_words = [
        "official",
        "video",
        "lyric",
        "audio",
        "клип",
        "текст",
        "prod",
        "4k",
        "hd",
        "mv",
        "премьера",
        "visualizer",
    ];
    let mut t = String::with_capacity(raw_title.len());
    let mut rest = raw_title;
    while let Some(pos) = rest.find(['(', '[', '【']) {
        let open = rest[pos..].chars().next().unwrap_or('(');
        let close = match open {
            '(' => ')',
            '[' => ']',
            _ => '】',
        };
        let after = pos + open.len_utf8();
        let Some(len) = rest[after..].find(close) else { break };
        let inner = rest[after..after + len].to_lowercase();
        let noise = open != '(' || noise_words.iter().any(|n| inner.contains(n));
        t.push_str(&rest[..pos]);
        if !noise {
            t.push_str(&rest[pos..after + len + close.len_utf8()]);
        }
        rest = &rest[after + len + close.len_utf8()..];
    }
    t.push_str(rest);
    let t = t.replace(['«', '»', '"'], "");
    for sep in [" - ", " – ", " — ", " | "] {
        if let Some((a, b)) = t.split_once(sep) {
            return (a.trim().to_string(), b.trim().to_string());
        }
    }
    let uploader = uploader.trim_end_matches(" - Topic").trim().to_string();
    (uploader, t.trim().to_string())
}

#[derive(Debug, Deserialize)]
struct LrclibItem {
    #[serde(rename = "trackName", default)]
    track_name: String,
    #[serde(rename = "artistName", default)]
    artist_name: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(rename = "plainLyrics", default)]
    plain_lyrics: Option<String>,
    #[serde(rename = "syncedLyrics", default)]
    synced_lyrics: Option<String>,
}

pub struct Lrclib {
    pub base: String,
}

impl Lrclib {
    pub fn from_env() -> Self {
        Self { base: std::env::var("SUPSTREAM_LRCLIB_URL").unwrap_or_else(|_| "https://lrclib.net".into()) }
    }

    /// Ищет текст: сначала по исполнителю+названию, потом по общей строке.
    /// Из кандидатов берём ближайший по длительности (±7 с), предпочитая текст с таймкодами.
    pub async fn find(
        &self,
        http: &reqwest::Client,
        artist: &str,
        title: &str,
        duration: Option<f64>,
    ) -> Result<Option<Lyrics>> {
        let mut queries: Vec<Vec<(&str, String)>> = Vec::new();
        if !artist.is_empty() {
            queries.push(vec![("track_name", title.to_string()), ("artist_name", artist.to_string())]);
        }
        queries.push(vec![("q", format!("{artist} {title}").trim().to_string())]);

        for q in queries {
            let items: Vec<LrclibItem> = http
                .get(format!("{}/api/search", self.base))
                .query(&q)
                .send()
                .await
                .context("LRCLIB недоступен")?
                .error_for_status()
                .context("LRCLIB вернул ошибку")?
                .json()
                .await
                .context("LRCLIB: неожиданный ответ")?;
            if let Some(best) = pick_best(items, duration) {
                return Ok(Some(best));
            }
        }
        Ok(None)
    }
}

fn pick_best(items: Vec<LrclibItem>, duration: Option<f64>) -> Option<Lyrics> {
    let mut scored: Vec<(f64, LrclibItem)> = items
        .into_iter()
        .filter(|i| i.instrumental || i.plain_lyrics.is_some() || i.synced_lyrics.is_some())
        .filter_map(|i| {
            let diff = match (duration, i.duration) {
                (Some(a), Some(b)) => (a - b).abs(),
                _ => 0.0,
            };
            // Другая длительность — скорее всего другая версия (ремикс, live) с другим текстом.
            if diff > 7.0 {
                return None;
            }
            let synced_bonus =
                if i.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty()) { 0.0 } else { 10.0 };
            Some((diff + synced_bonus, i))
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let (_, best) = scored.into_iter().next()?;
    let synced = best.synced_lyrics.as_deref().map(parse_lrc).filter(|l| !l.is_empty());
    let (is_synced, lines) = match synced {
        Some(lines) => (true, lines),
        None => (false, best.plain_lyrics.as_deref().map(parse_plain).unwrap_or_default()),
    };
    Some(Lyrics {
        source: "lrclib".into(),
        synced: is_synced,
        instrumental: best.instrumental,
        lines,
        matched_artist: best.artist_name,
        matched_title: best.track_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_with_multiple_stamps_and_tags() {
        let lrc =
            "[ar:Artist]\n[ti:Song]\n[00:12.50] Первая строка\n[00:05.00][01:10.00]Припев\n[00:20.00]\n";
        let lines = parse_lrc(lrc);
        assert_eq!(
            lines,
            vec![
                LyricLine { time: Some(5.0), text: "Припев".into() },
                LyricLine { time: Some(12.5), text: "Первая строка".into() },
                LyricLine { time: Some(20.0), text: "".into() },
                LyricLine { time: Some(70.0), text: "Припев".into() },
            ]
        );
    }

    #[test]
    fn guesses_artist_and_title() {
        let cases = [
            ("Linkin Park - Numb (Official Music Video) [4K UPGRADE]", "Linkin Park", "Numb"),
            ("Кино — Группа крови (Official Audio)", "Кино", "Группа крови"),
            ("Artist - Song (feat. Other) [Lyrics]", "Artist", "Song (feat. Other)"),
            ("Some Song", "Singer - Topic", "Some Song"),
            ("A - B (feat. C) (Official Video)", "A", "B (feat. C)"),
        ];
        for (raw, artist, title) in cases {
            let uploader = if raw == "Some Song" { "Singer - Topic" } else { "" };
            let (a, t) = guess_artist_title(raw, uploader);
            assert_eq!((a.as_str(), t.as_str()), (if raw == "Some Song" { "Singer" } else { artist }, title));
        }
    }

    #[test]
    fn picks_closest_synced_version() {
        let items = vec![
            LrclibItem {
                track_name: "Song (Live)".into(),
                artist_name: "A".into(),
                duration: Some(260.0),
                instrumental: false,
                plain_lyrics: Some("live".into()),
                synced_lyrics: Some("[00:01.00]live".into()),
            },
            LrclibItem {
                track_name: "Song".into(),
                artist_name: "A".into(),
                duration: Some(201.0),
                instrumental: false,
                plain_lyrics: Some("plain only".into()),
                synced_lyrics: None,
            },
            LrclibItem {
                track_name: "Song".into(),
                artist_name: "A".into(),
                duration: Some(203.0),
                instrumental: false,
                plain_lyrics: Some("x".into()),
                synced_lyrics: Some("[00:01.00]studio".into()),
            },
        ];
        let best = pick_best(items, Some(200.0)).unwrap();
        assert!(best.synced);
        assert_eq!(best.lines[0].text, "studio");
    }
}
