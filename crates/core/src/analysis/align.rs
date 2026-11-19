//! Выравнивание известного текста по звуку моделью MMS (wav2vec2 + CTC, 1100+ языков).
//!
//! Модель выдаёт для каждого кадра (20 мс) вероятности латинских букв. Текст транслитерируется
//! в латиницу, и алгоритм Витерби находит, в каких кадрах звучит каждая буква — а значит,
//! где начинается и заканчивается каждое слово.

use std::collections::HashMap;

pub const SAMPLE_RATE: u32 = 16_000;
/// Длительность одного кадра модели.
pub const FRAME_SECS: f64 = 0.02;
const BLANK: usize = 0;
/// «Джокер»: совпадает с любой речью. Ставится по краям строки, чтобы чужие слова
/// из запаса окна (конец прошлой строки, начало следующей) не растягивали крайние слова.
const STAR: usize = usize::MAX;

/// Кириллица → латиница (как uroman, которым обучали модель); латиница остаётся как есть.
pub fn romanize(word: &str) -> String {
    let mut out = String::new();
    for c in word.to_lowercase().chars() {
        let r = match c {
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' => "g",
            'д' => "d",
            'е' | 'ё' | 'э' => "e",
            'ж' => "zh",
            'з' => "z",
            'и' | 'і' => "i",
            'й' | 'ы' => "y",
            'к' => "k",
            'л' => "l",
            'м' => "m",
            'н' => "n",
            'о' => "o",
            'п' => "p",
            'р' => "r",
            'с' => "s",
            'т' => "t",
            'у' => "u",
            'ф' => "f",
            'х' => "kh",
            'ц' => "ts",
            'ч' => "ch",
            'ш' => "sh",
            'щ' => "shch",
            'ю' => "yu",
            'я' => "ya",
            'ї' => "yi",
            'є' => "ye",
            'ґ' => "g",
            'ъ' | 'ь' => "",
            c if c.is_ascii_lowercase() || c == '\'' => {
                out.push(c);
                continue;
            }
            // Диакритика в латинице: é → e и т.п.
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => "a",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => "o",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'ñ' => "n",
            'ç' => "c",
            _ => "",
        };
        out.push_str(r);
    }
    out
}

pub struct Vocab(HashMap<char, usize>);

impl Vocab {
    /// `vocab.json` модели: `{ "a": 4, "<blank>": 0, ... }`.
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        let map: HashMap<String, usize> = serde_json::from_str(json)?;
        Ok(Self(
            map.into_iter()
                .filter_map(|(k, v)| {
                    let mut ch = k.chars();
                    match (ch.next(), ch.next()) {
                        (Some(c), None) => Some((c, v)),
                        _ => None,
                    }
                })
                .collect(),
        ))
    }

    fn tokens(&self, word: &str) -> Vec<usize> {
        romanize(word).chars().filter_map(|c| self.0.get(&c).copied()).collect()
    }
}

/// Лог-вероятности `[кадры][токены]` → log_softmax по токенам.
pub fn log_softmax(logits: &[f32], n_tokens: usize) -> Vec<Vec<f32>> {
    logits
        .chunks(n_tokens)
        .map(|row| {
            let m = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let lse = row.iter().map(|x| (x - m).exp()).sum::<f32>().ln() + m;
            row.iter().map(|x| x - lse).collect()
        })
        .collect()
}

/// Нормализация входа wav2vec2: нулевое среднее, единичная дисперсия.
pub fn normalize(audio: &[f32]) -> Vec<f32> {
    let n = audio.len().max(1) as f32;
    let mean = audio.iter().sum::<f32>() / n;
    let var = audio.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    let sd = (var + 1e-7).sqrt();
    audio.iter().map(|x| (x - mean) / sd).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct WordSpan {
    pub start: f64,
    pub end: f64,
    /// Средняя лог-вероятность букв слова — насколько модель «уверена».
    pub confidence: f32,
    /// Группы букв, разделённые паузами длиннее `PIECE_GAP`. Если модель не расслышала букву,
    /// она может «найти» её в чужом слове далеко впереди — такой кусок потом отбрасывается.
    pub pieces: Vec<(f64, f64)>,
}

/// Пауза между буквами одного слова, после которой начинается новый кусок.
pub const PIECE_GAP: f64 = 0.4;

/// CTC-Витерби: для каждого слова — интервал в секундах (от начала `emissions`).
/// `None` у слова — в нём нет букв (числа, эмодзи) или выравнивание невозможно.
pub fn align_words(emissions: &[Vec<f32>], words: &[&str], vocab: &Vocab) -> Vec<Option<WordSpan>> {
    let mut toks = vec![STAR];
    let mut owner = vec![usize::MAX];
    for (wi, w) in words.iter().enumerate() {
        for t in vocab.tokens(w) {
            toks.push(t);
            owner.push(wi);
        }
    }
    toks.push(STAR);
    owner.push(usize::MAX);
    let t_len = emissions.len();
    let s_len = 2 * toks.len() + 1;
    if toks.len() == 2 || t_len < toks.len() {
        return vec![None; words.len()];
    }
    let state = |s: usize| if s.is_multiple_of(2) { BLANK } else { toks[s / 2] };
    // Вероятность «джокера» — лучшая из букв в кадре минус штраф: при равенстве
    // побеждает настоящая буква, а чужая речь (где наших букв нет) уходит в джокер.
    const STAR_PENALTY: f32 = 1.0;
    let star: Vec<f32> = emissions
        .iter()
        .map(|e| e.iter().skip(1).cloned().fold(f32::NEG_INFINITY, f32::max) - STAR_PENALTY)
        .collect();
    let emit = |t: usize, tok: usize| if tok == STAR { star[t] } else { emissions[t][tok] };
    const NEG: f32 = -1e30;
    let mut prev = vec![NEG; s_len];
    let mut cur = vec![NEG; s_len];
    let mut back = vec![0u8; t_len * s_len];
    // Джокеры необязательны: начать можно и сразу с первой буквы (состояния 2–3),
    // закончить — на последней букве (s_len-4, s_len-3).
    for (s0, p) in prev.iter_mut().enumerate().take(4) {
        *p = emit(0, state(s0));
    }
    for t in 1..t_len {
        // Из-за порядка «пусто/буква» состояние s достижимо только из s, s-1, s-2.
        let lo = (2 * toks.len()).saturating_sub(2 * (t_len - t)).saturating_sub(5);
        let hi = (2 * t + 4).min(s_len);
        for s in 0..s_len {
            if s < lo || s >= hi {
                cur[s] = NEG;
                continue;
            }
            let mut best = prev[s];
            let mut b = 0u8;
            if s >= 1 && prev[s - 1] > best {
                best = prev[s - 1];
                b = 1;
            }
            if s >= 2 && s % 2 == 1 && state(s) != state(s - 2) && prev[s - 2] > best {
                best = prev[s - 2];
                b = 2;
            }
            cur[s] = best + emit(t, state(s));
            back[t * s_len + s] = b;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let mut s = (s_len - 4..s_len).max_by(|&a, &b| prev[a].total_cmp(&prev[b])).expect("есть состояния");
    if prev[s] <= NEG / 2.0 {
        return vec![None; words.len()];
    }
    let mut frames: Vec<Vec<(usize, f32)>> = vec![Vec::new(); words.len()];
    for t in (0..t_len).rev() {
        if s % 2 == 1 && owner[s / 2] != usize::MAX {
            frames[owner[s / 2]].push((t, emit(t, state(s))));
        }
        if t > 0 {
            s -= back[t * s_len + s] as usize;
        }
    }
    frames
        .into_iter()
        .map(|mut f| {
            if f.is_empty() {
                return None;
            }
            f.sort_by_key(|x| x.0);
            let gap = (PIECE_GAP / FRAME_SECS) as usize;
            let mut pieces: Vec<(f64, f64)> = Vec::new();
            let (mut a, mut b) = (f[0].0, f[0].0);
            for &(t, _) in &f[1..] {
                if t > b + gap {
                    pieces.push((a as f64 * FRAME_SECS, (b + 1) as f64 * FRAME_SECS));
                    a = t;
                }
                b = t;
            }
            pieces.push((a as f64 * FRAME_SECS, (b + 1) as f64 * FRAME_SECS));
            Some(WordSpan {
                start: pieces[0].0,
                end: pieces[pieces.len() - 1].1,
                confidence: f.iter().map(|x| x.1).sum::<f32>() / f.len() as f32,
                pieces,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        Vocab::from_json(r#"{"<blank>":0,"<pad>":1,"a":2,"b":3,"k":4,"h":5,"u":6,"y":7}"#).unwrap()
    }

    #[test]
    fn romanizes_russian_and_keeps_latin() {
        assert_eq!(romanize("Нахуй"), "nakhuy");
        assert_eq!(romanize("щёки"), "shcheki");
        assert_eq!(romanize("подъезд"), "podezd");
        assert_eq!(romanize("Don't"), "don't");
        assert_eq!(romanize("café 2024!"), "cafe");
    }

    /// Синтетические «эмиссии»: буквы звучат в заданных кадрах, в остальных — пусто.
    #[test]
    fn viterbi_finds_word_boundaries() {
        let v = vocab();
        let n_tok = 8;
        let mut em = vec![vec![-10.0f32; n_tok]; 60];
        for row in em.iter_mut() {
            row[0] = -0.05; // по умолчанию — «пусто»
        }
        let mut put = |frames: std::ops::Range<usize>, tok: usize| {
            for t in frames {
                em[t][tok] = -0.05;
                em[t][0] = -5.0;
            }
        };
        // «ab» в кадрах 5..9, «ku» в кадрах 30..36.
        put(5..7, 2);
        put(7..9, 3);
        put(30..33, 4);
        put(33..36, 6);
        let spans = align_words(&em, &["ab", "ку"], &v);
        let a = spans[0].clone().unwrap();
        let b = spans[1].clone().unwrap();
        assert!((a.start - 0.10).abs() < 1e-9 && (a.end - 0.18).abs() < 1e-9, "{a:?}");
        assert!((b.start - 0.60).abs() < 1e-9 && (b.end - 0.72).abs() < 1e-9, "{b:?}");
        assert!(a.confidence > -1.0);
    }

    #[test]
    fn handles_repeated_letters_and_wordless_tokens() {
        let v = vocab();
        let mut em = vec![vec![-10.0f32; 8]; 20];
        for row in em.iter_mut() {
            row[0] = -0.05;
        }
        // «aa» требует пустой кадр между одинаковыми буквами.
        for (t, tok) in [(3, 2), (4, 0), (5, 2)] {
            em[t][tok] = -0.05;
            if tok != 0 {
                em[t][0] = -5.0;
            }
        }
        let spans = align_words(&em, &["aa", "2024"], &v);
        assert!(align_words(&em, &["2024"], &v)[0].is_none());
        let s = spans[0].clone().unwrap();
        assert!((s.start - 0.06).abs() < 1e-9 && (s.end - 0.12).abs() < 1e-9, "{s:?}");
        assert!(spans[1].is_none(), "в «2024» нет букв");
    }

    /// Чужая речь после строки уходит в «джокер», а не растягивает последнее слово.
    #[test]
    fn trailing_speech_does_not_stretch_last_word() {
        let v = vocab();
        let mut em = vec![vec![-10.0f32; 8]; 50];
        for row in em.iter_mut() {
            row[0] = -0.05;
        }
        let mut put = |frames: std::ops::Range<usize>, tok: usize| {
            for t in frames {
                em[t][tok] = -0.05;
                em[t][0] = -5.0;
            }
        };
        put(5..8, 2); // «a» — наше слово
        put(8..10, 3); // «b»
        put(20..40, 5); // «h»… — следующая строка, не наша
        let s = align_words(&em, &["ab"], &v)[0].clone().unwrap();
        assert!((s.end - 0.20).abs() < 1e-9, "конец слова не должен уехать в чужую речь: {s:?}");
    }

    /// Буква, которую модель «нашла» далеко после остальных, — отдельный кусок.
    #[test]
    fn far_letter_becomes_separate_piece() {
        let v = vocab();
        let mut em = vec![vec![-10.0f32; 8]; 100];
        for row in em.iter_mut() {
            row[0] = -0.05;
        }
        for (frames, tok) in [(5..7, 2), (80..82, 3)] {
            for t in frames {
                em[t][tok] = -0.05;
                em[t][0] = -5.0;
            }
        }
        let s = align_words(&em, &["ab"], &v)[0].clone().unwrap();
        assert_eq!(s.pieces.len(), 2, "{s:?}");
        assert!((s.pieces[0].1 - 0.14).abs() < 1e-9);
    }

    #[test]
    fn too_short_audio_gives_none() {
        let em = vec![vec![0.0f32; 8]; 2];
        assert!(align_words(&em, &["abba"], &vocab()).iter().all(Option::is_none));
    }

    #[test]
    fn log_softmax_normalizes() {
        let r = log_softmax(&[1.0, 2.0, 3.0], 3);
        let sum: f32 = r[0].iter().map(|x| x.exp()).sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }
}
