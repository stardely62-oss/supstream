//! Своя музыка из папок на ПК (купленная, по подписке, своя). Ищется по названию файла:
//! «Artist - Title.mp3». Такие треки не скачиваются и разрешены в безопасном режиме.

use std::path::{Path, PathBuf};

use super::track::TrackMeta;

const AUDIO_EXT: &[&str] = &["mp3", "ogg", "opus", "webm", "flac", "wav", "m4a"];
const MAX_DEPTH: usize = 4;
const MAX_FILES: usize = 20_000;

#[derive(Debug, Clone)]
pub struct LocalTrack {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, depth + 1, out);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| AUDIO_EXT.contains(&x.to_lowercase().as_str()))
        {
            out.push(p);
        }
    }
}

fn describe(path: &Path) -> LocalTrack {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    // «01. Artist - Title» → убираем номер трека.
    let stem = stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ' || c == '_');
    let (artist, title) = match stem.split_once(" - ") {
        Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
        None => (String::new(), stem.trim().to_string()),
    };
    LocalTrack { path: path.to_path_buf(), artist, title }
}

/// Лучший файл, в названии которого есть все слова запроса.
pub fn find(dirs: &[String], query: &str) -> Option<LocalTrack> {
    let q = normalize(query);
    if q.is_empty() || super::ytdlp::is_url(query) {
        return None;
    }
    let words: Vec<&str> = q.split(' ').collect();
    let mut files = Vec::new();
    for d in dirs.iter().filter(|d| !d.trim().is_empty()) {
        walk(Path::new(d.trim()), 0, &mut files);
    }
    files
        .iter()
        .map(|p| describe(p))
        .filter_map(|t| {
            let hay = normalize(&format!("{} {}", t.artist, t.title));
            let hay_words: Vec<&str> = hay.split(' ').collect();
            // Слово запроса должно совпасть с началом слова в названии: «numb» ≠ «numbers»? — допускаем,
            // но точные совпадения слов ценим выше.
            if !words.iter().all(|w| hay_words.iter().any(|h| h.starts_with(w))) {
                return None;
            }
            let exact = words.iter().filter(|w| hay_words.contains(w)).count();
            Some((exact, usize::MAX - hay.len(), t))
        })
        .max_by_key(|(exact, short, _)| (*exact, *short))
        .map(|(_, _, t)| t)
}

/// Входит ли путь в одну из папок библиотеки (для раздачи файла плееру).
pub fn contains(dirs: &[String], path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else { return false };
    dirs.iter()
        .filter(|d| !d.trim().is_empty())
        .filter_map(|d| Path::new(d.trim()).canonicalize().ok())
        .any(|d| path.starts_with(d))
}

impl LocalTrack {
    pub fn meta(&self) -> TrackMeta {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.path.hash(&mut h);
        TrackMeta {
            video_id: format!("local-{:016x}", h.finish()),
            url: String::new(),
            title: if self.artist.is_empty() {
                self.title.clone()
            } else {
                format!("{} - {}", self.artist, self.title)
            },
            artist: self.artist.clone(),
            track_title: self.title.clone(),
            duration: None,
            thumbnail: None,
            extractor: "local".into(),
            channel_id: None,
            channel: "Моя музыка".into(),
            local: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_words_in_filename() {
        let dir = std::env::temp_dir().join(format!("supstream-lib-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Альбом")).unwrap();
        for f in [
            "01. Кино - Группа крови.mp3",
            "Кино - Группа крови (Live).mp3",
            "Artist - Numbers.ogg",
            "readme.txt",
        ] {
            std::fs::write(
                dir.join(if f.starts_with("01") { format!("Альбом/{f}") } else { f.into() }),
                b"x",
            )
            .unwrap();
        }
        let dirs = vec![dir.display().to_string()];
        let t = find(&dirs, "кино группа крови").unwrap();
        assert_eq!(t.artist, "Кино");
        assert_eq!(t.title, "Группа крови", "короткое название без (Live) — в приоритете");
        assert!(t.path.ends_with("01. Кино - Группа крови.mp3"));
        assert_eq!(find(&dirs, "numb").unwrap().title, "Numbers");
        assert!(find(&dirs, "readme").is_none(), "не аудио");
        assert!(find(&dirs, "несуществующий трек").is_none());
        assert!(find(&dirs, "https://youtu.be/x").is_none());
        assert!(contains(&dirs, &t.path));
        assert!(!contains(&dirs, Path::new("/etc/passwd")));
        let m = t.meta();
        assert!(m.local && m.video_id.starts_with("local-"));
        std::fs::remove_dir_all(dir).ok();
    }
}
