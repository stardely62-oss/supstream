//! Проверка трека по тексту: находим нарушения, ставим отметки глушения
//! и выносим вердикт по настройкам стримера.
//!
//! Сейчас позиция слова оценивается внутри строки с таймкодом (доля символов),
//! поэтому отметки с запасом. Следующий шаг — выравнивание текста по звуку
//! (GigaAM/Whisper), тогда отметки станут точными до слова.

use super::filter::{Category, Filter};
use super::lyrics::Lyrics;
use super::track::{CheckReport, LyricsInfo, MusicConfig, Mute, Policy, ReportLine, Status};

/// Строка без следующей метки считается длиной не больше этого.
const MAX_LINE_SECS: f64 = 8.0;
/// Слова обычно звучат в первых ~90% интервала строки.
const SUNG_SHARE: f64 = 0.9;

pub struct Outcome {
    pub report: CheckReport,
    pub mutes: Vec<Mute>,
    pub status: Status,
}

fn severity(s: Status) -> u8 {
    match s {
        Status::Rejected => 2,
        Status::NeedsReview => 1,
        _ => 0,
    }
}

fn worst(a: Status, b: Status) -> Status {
    if severity(b) > severity(a) { b } else { a }
}

pub fn evaluate(
    lyrics: Option<&Lyrics>,
    duration: Option<f64>,
    filter: &Filter,
    cfg: &MusicConfig,
) -> Outcome {
    // Старые настройки приводим к допустимым вариантам.
    let cfg = &MusicConfig {
        hate: cfg.hate.for_hate(),
        profanity: cfg.profanity.for_profanity(),
        no_lyrics: cfg.no_lyrics.for_no_lyrics(),
        ..cfg.clone()
    };
    let mut reasons = Vec::new();
    let mut status = Status::Ready;

    let Some(lyrics) = lyrics else {
        let s = match cfg.no_lyrics {
            Policy::Allow => {
                reasons.push("Текст не найден — пропущен по настройке «без текста: пропускать»".into());
                Status::Ready
            }
            Policy::Reject => {
                reasons.push("Текст не найден — трек отклонён по настройке".into());
                Status::Rejected
            }
            Policy::Mute | Policy::Bleep | Policy::Review => {
                reasons.push("Текст не найден — проверьте трек вручную".into());
                Status::NeedsReview
            }
        };
        return Outcome {
            report: CheckReport {
                lyrics: None,
                lines: Vec::new(),
                hate: 0,
                profanity: 0,
                reasons,
                precision: "none".into(),
            },
            mutes: Vec::new(),
            status: s,
        };
    };

    let info = LyricsInfo {
        source: lyrics.source.clone(),
        synced: lyrics.synced,
        instrumental: lyrics.instrumental,
        matched_artist: lyrics.matched_artist.clone(),
        matched_title: lyrics.matched_title.clone(),
    };

    let mut lines = Vec::with_capacity(lyrics.lines.len());
    let (mut hate, mut profanity) = (0, 0);
    let mut mutes: Vec<Mute> = Vec::new();

    for (i, l) in lyrics.lines.iter().enumerate() {
        let end = l.time.map(|t| {
            let next = lyrics.lines[i + 1..].iter().find_map(|n| n.time.filter(|nt| *nt > t));
            let mut e = next.unwrap_or(t + MAX_LINE_SECS).min(t + MAX_LINE_SECS * 2.0);
            if let Some(d) = duration {
                e = e.min(d);
            }
            e.max(t + 0.5)
        });
        let hits = filter.scan(&l.text);
        let len = l.text.chars().count().max(1) as f64;
        for h in &hits {
            match h.category {
                Category::Hate => hate += 1,
                Category::Profanity => profanity += 1,
            }
            if let (Some(t0), Some(t1)) = (l.time, end) {
                let span = (t1 - t0) * SUNG_SHARE;
                let s = t0 + span * (h.start as f64 / len) - cfg.mute_padding;
                let e = t0 + span * (h.end as f64 / len) + cfg.mute_padding;
                let policy = match h.category {
                    Category::Hate => cfg.hate,
                    Category::Profanity => cfg.profanity,
                };
                mutes.push(Mute {
                    id: 0,
                    start: s.max(t0 - 0.3).max(0.0),
                    end: e.min(t1),
                    label: h.word.clone(),
                    category: Some(h.category),
                    auto: true,
                    // Оскорбления вырезаем всегда (даже если трек одобрят вручную),
                    // мат — по настройке. Отклонённый трек тоже получает отметки:
                    // если стример его одобрит, они уже будут на месте.
                    enabled: h.category == Category::Hate
                        || matches!(policy, Policy::Mute | Policy::Bleep | Policy::Reject),
                    bleep: h.category == Category::Profanity && policy != Policy::Mute,
                });
            }
        }
        lines.push(ReportLine { time: l.time, end, text: l.text.clone(), hits });
    }

    if lyrics.instrumental {
        reasons.push("Инструментальный трек".into());
    }

    let mut apply = |count: usize, policy: Policy, what: &str| {
        if count == 0 {
            return;
        }
        let s = match policy {
            Policy::Allow => {
                reasons.push(format!("{what}: {count} — пропущено по настройке"));
                Status::Ready
            }
            Policy::Mute if lyrics.synced => {
                reasons.push(format!("{what}: {count} — вырезано автоматически"));
                Status::Ready
            }
            Policy::Bleep if lyrics.synced => {
                reasons.push(format!("{what}: {count} — запикано автоматически"));
                Status::Ready
            }
            Policy::Mute | Policy::Bleep => {
                reasons.push(format!(
                    "{what}: {count}, но у текста нет таймкодов — заглушить нельзя, проверьте вручную"
                ));
                Status::NeedsReview
            }
            Policy::Review => {
                reasons.push(format!("{what}: {count} — нужна ваша проверка"));
                Status::NeedsReview
            }
            Policy::Reject => {
                reasons.push(format!("{what}: {count} — трек отклонён"));
                Status::Rejected
            }
        };
        status = worst(status, s);
    };
    apply(hate, cfg.hate, "Запретки");
    apply(profanity, cfg.profanity, "Мат");

    if hate == 0 && profanity == 0 && !lyrics.instrumental {
        reasons.push("Нарушений в тексте не найдено".into());
    }

    Outcome {
        report: CheckReport {
            lyrics: Some(info),
            lines,
            hate,
            profanity,
            reasons,
            precision: if lyrics.synced { "line_estimate".into() } else { "none".into() },
        },
        mutes: merge(mutes),
        status,
    }
}

/// Сливает пересекающиеся отметки, чтобы звук не «моргал».
fn merge(mut mutes: Vec<Mute>) -> Vec<Mute> {
    mutes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Mute> = Vec::with_capacity(mutes.len());
    for m in mutes {
        if let Some(last) = out.last_mut()
            && m.start <= last.end + 0.15
            && m.enabled == last.enabled
        {
            last.end = last.end.max(m.end);
            // Если рядом и пик, и вырезание — вырезаем (строже).
            last.bleep = last.bleep && m.bleep;
            if !last.label.split(", ").any(|w| w == m.label) {
                last.label = format!("{}, {}", last.label, m.label);
            }
            last.category = last.category.max(m.category);
            continue;
        }
        out.push(m);
    }
    for (i, m) in out.iter_mut().enumerate() {
        m.id = i as u32 + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::lyrics::{LyricLine, Lyrics};

    fn lyrics(synced: bool, lines: &[(f64, &str)]) -> Lyrics {
        Lyrics {
            source: "test".into(),
            synced,
            instrumental: false,
            lines: lines
                .iter()
                .map(|(t, s)| LyricLine { time: synced.then_some(*t), text: s.to_string() })
                .collect(),
            matched_artist: "A".into(),
            matched_title: "T".into(),
        }
    }

    fn filter() -> Filter {
        Filter::with_custom(&[], &[])
    }

    #[test]
    fn clean_track_is_ready() {
        let l = lyrics(true, &[(1.0, "Солнце светит"), (5.0, "Птицы поют")]);
        let o = evaluate(Some(&l), Some(180.0), &filter(), &MusicConfig::default());
        assert_eq!(o.status, Status::Ready);
        assert!(o.mutes.is_empty());
    }

    #[test]
    fn profanity_is_bleeped_by_default_and_muted_if_chosen() {
        let l = lyrics(true, &[(10.0, "я иду домой, бля, опять"), (14.0, "дальше чисто")]);
        let o = evaluate(Some(&l), Some(180.0), &filter(), &MusicConfig::default());
        assert!(o.mutes[0].bleep, "мат по умолчанию запикивается");
        let cfg = MusicConfig { profanity: Policy::Mute, ..Default::default() };
        let o = evaluate(Some(&l), Some(180.0), &filter(), &cfg);
        assert!(!o.mutes[0].bleep && o.mutes[0].enabled);
        let cfg = MusicConfig { profanity: Policy::Reject, ..Default::default() };
        assert_eq!(evaluate(Some(&l), Some(180.0), &filter(), &cfg).status, Status::Rejected);
    }

    #[test]
    fn hate_is_cut_never_bleeped_and_old_settings_are_mapped() {
        let l = lyrics(true, &[(3.0, "ты пидор, сука"), (6.0, "ok")]);
        let cfg = MusicConfig { hate: Policy::Review, ..Default::default() };
        let o = evaluate(Some(&l), Some(100.0), &filter(), &cfg);
        assert_eq!(o.status, Status::Rejected, "старое «на проверку» для оскорблений → отклонять");
        let cfg = MusicConfig { hate: Policy::Bleep, ..Default::default() };
        let o = evaluate(Some(&l), Some(100.0), &filter(), &cfg);
        assert_eq!(o.status, Status::Ready);
        assert_eq!(o.mutes.len(), 1, "пик и вырезание рядом сливаются: {:?}", o.mutes);
        assert!(!o.mutes[0].bleep, "оскорбления вырезаются, а не запикиваются");
    }

    #[test]
    fn profanity_is_muted_inside_its_line() {
        let l = lyrics(true, &[(10.0, "я иду домой, бля, опять"), (14.0, "дальше чисто")]);
        let o = evaluate(Some(&l), Some(180.0), &filter(), &MusicConfig::default());
        assert_eq!(o.status, Status::Ready);
        assert_eq!(o.mutes.len(), 1);
        let m = &o.mutes[0];
        assert!(m.enabled && m.auto);
        assert!(m.start >= 9.7 && m.end <= 14.0, "{m:?}");
        // Слово примерно в середине строки — отметка не в самом начале.
        assert!(m.start > 10.5, "{m:?}");
        assert_eq!(o.report.profanity, 1);
    }

    #[test]
    fn hate_rejects_by_default_but_mute_is_prepared() {
        let l = lyrics(true, &[(3.0, "ты пидор"), (6.0, "ok")]);
        let o = evaluate(Some(&l), Some(100.0), &filter(), &MusicConfig::default());
        assert_eq!(o.status, Status::Rejected);
        assert_eq!(o.mutes.len(), 1);
        assert!(o.mutes[0].enabled);
        assert_eq!(o.mutes[0].category, Some(Category::Hate));
    }

    #[test]
    fn unsynced_lyrics_with_mat_go_to_review() {
        let l = lyrics(false, &[(0.0, "сука")]);
        let o = evaluate(Some(&l), None, &filter(), &MusicConfig::default());
        assert_eq!(o.status, Status::NeedsReview);
        assert!(o.mutes.is_empty());
    }

    #[test]
    fn no_lyrics_follows_policy() {
        let mut cfg = MusicConfig::default();
        assert_eq!(evaluate(None, None, &filter(), &cfg).status, Status::NeedsReview);
        cfg.no_lyrics = Policy::Reject;
        assert_eq!(evaluate(None, None, &filter(), &cfg).status, Status::Rejected);
        cfg.no_lyrics = Policy::Allow;
        assert_eq!(evaluate(None, None, &filter(), &cfg).status, Status::Ready);
    }

    #[test]
    fn overlapping_mutes_are_merged() {
        let l = lyrics(true, &[(20.0, "бля бля сука"), (23.0, "")]);
        let o = evaluate(Some(&l), None, &filter(), &MusicConfig::default());
        assert_eq!(o.mutes.len(), 1, "{:?}", o.mutes);
        assert!(o.mutes[0].label.contains("сука"));
        assert_eq!(o.mutes[0].id, 1);
    }

    #[test]
    fn profanity_allowed_leaves_mutes_disabled() {
        let cfg = MusicConfig { profanity: Policy::Allow, ..Default::default() };
        let l = lyrics(true, &[(1.0, "нахуй"), (4.0, "")]);
        let o = evaluate(Some(&l), None, &filter(), &cfg);
        assert_eq!(o.status, Status::Ready);
        assert_eq!(o.mutes.len(), 1);
        assert!(!o.mutes[0].enabled, "отметка подготовлена, но выключена");
    }
}
