//! Анализ звука: точные отметки глушения.
//!
//! Для каждой строки с нарушением: вырезаем кусок трека → выделяем вокал (MDX) → выравниваем
//! текст строки по голосу (MMS CTC) → получаем время каждого слова с точностью ~50 мс.
//! Анализируются только строки с нарушениями, поэтому на трек уходит секунды, а не минуты.
//!
//! Модели загружаются при первом использовании и выгружаются после 3 минут простоя,
//! чтобы не держать ~1 ГБ памяти во время стрима без заказов.

pub mod align;
pub mod assets;
pub mod audio;
pub mod separate;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use serde::Serialize;
use tokio::sync::{Semaphore, watch};

use crate::app::AppState;

const IDLE_UNLOAD: Duration = Duration::from_secs(180);
/// Сколько звука брать вокруг строки: строки в LRC бывают неточными на ~0.5–1 с.
const WINDOW_PAD: f64 = 1.0;
/// Запас вокруг найденного слова. Модель ставит начало точно, а конец — чуть раньше
/// настоящего (замерено на тестовом треке: начало ±60 мс, конец −30…−225 мс).
pub const PAD_BEFORE: f64 = 0.15;
pub const PAD_AFTER: f64 = 0.35;
/// Насколько кусок слова может начаться позже конца строки по LRC.
const LINE_END_SLACK: f64 = 0.3;
/// Ниже этой уверенности слово считаем ненайденным и глушим по оценке строки.
const MIN_CONFIDENCE: f32 = -4.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InstallState {
    Idle,
    Downloading { asset: String, done: u64, total: u64 },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisStatus {
    pub components: Vec<assets::AssetStatus>,
    pub total_size: u64,
    pub installed: bool,
    /// ONNX Runtime загрузился (или ещё не пробовали — `None`).
    pub runtime_error: Option<String>,
    pub install: InstallState,
    pub loaded: bool,
}

struct Engine {
    vocals: Session,
    aligner: Session,
    vocab: align::Vocab,
    stft: separate::Stft,
}

pub struct AnalysisService {
    dir: PathBuf,
    install: Mutex<InstallState>,
    rev: watch::Sender<u64>,
    engine: Mutex<Option<(Engine, Instant)>>,
    env: OnceLock<std::result::Result<ort::environment::Environment, String>>,
    run: Semaphore,
}

/// Строка с нарушениями для уточнения.
#[derive(Debug, Clone)]
pub struct LineRequest {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Нарушения: диапазоны символов в строке.
    pub hits: Vec<(usize, usize)>,
}

/// Найденное время нарушения: ключ — (номер строки, начало нарушения в символах).
pub type HitTimes = HashMap<(usize, usize), (f64, f64)>;

impl AnalysisService {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("models"),
            install: Mutex::new(InstallState::Idle),
            rev: watch::channel(0).0,
            engine: Mutex::new(None),
            env: OnceLock::new(),
            run: Semaphore::new(1),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.rev.subscribe()
    }

    fn bump(&self) {
        self.rev.send_modify(|r| *r += 1);
    }

    pub fn installed(&self) -> bool {
        assets::all().iter().all(|a| assets::installed(&self.dir, a))
    }

    pub fn status(&self) -> AnalysisStatus {
        let all = assets::all();
        AnalysisStatus {
            components: all
                .iter()
                .map(|a| assets::AssetStatus {
                    id: a.id,
                    label: a.label,
                    size: a.size,
                    installed: assets::installed(&self.dir, a),
                })
                .collect(),
            total_size: all.iter().map(|a| a.size).sum(),
            installed: self.installed(),
            runtime_error: self.env.get().and_then(|r| r.as_ref().err().cloned()),
            install: self.install.lock().unwrap().clone(),
            loaded: self.engine.lock().unwrap().is_some(),
        }
    }

    fn runtime_path(&self) -> PathBuf {
        if let Ok(p) = std::env::var("SUPSTREAM_ORT_DYLIB") {
            return PathBuf::from(p);
        }
        if cfg!(windows) {
            assets::path(&self.dir, &assets::RUNTIME_WINDOWS)
        } else if cfg!(target_os = "macos") {
            PathBuf::from("libonnxruntime.dylib")
        } else {
            PathBuf::from("libonnxruntime.so")
        }
    }

    fn environment(&self) -> Result<&ort::environment::Environment> {
        let r = self.env.get_or_init(|| {
            let path = self.runtime_path();
            ort::init_from(&path)
                .map_err(|e| format!("не удалось загрузить ONNX Runtime ({}): {e}", path.display()))?
                .with_name("supstream")
                .build()
                .map_err(|e| format!("ONNX Runtime: {e}"))
        });
        r.as_ref().map_err(|e| anyhow!("{e}"))
    }

    fn load(&self, threads: usize) -> Result<Engine> {
        let env = self.environment()?;
        let open = |a: &assets::Asset| -> Result<Session> {
            Session::builder(env)
                .map_err(|e| anyhow!("{e}"))?
                .with_intra_threads(threads)
                .map_err(|e| anyhow!("{e}"))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| anyhow!("{e}"))?
                .commit_from_file(assets::path(&self.dir, a))
                .map_err(|e| anyhow!("{}: {e}", a.label))
        };
        let vocab = std::fs::read_to_string(assets::path(&self.dir, &assets::ALIGNER_VOCAB))?;
        Ok(Engine {
            vocals: open(&assets::VOCALS)?,
            aligner: open(&assets::ALIGNER)?,
            vocab: align::Vocab::from_json(&vocab)?,
            stft: separate::Stft::new(),
        })
    }

    /// Выгрузить модели, если ими давно не пользовались.
    pub fn unload_if_idle(&self) {
        let mut e = self.engine.lock().unwrap();
        if e.as_ref().is_some_and(|(_, at)| at.elapsed() > IDLE_UNLOAD) {
            *e = None;
            drop(e);
            tracing::info!("анализ звука: модели выгружены из памяти");
            self.bump();
        }
    }

    /// Уточнить время нарушений в строках. Работает в отдельном потоке, по одному треку за раз.
    pub async fn refine(
        self: &Arc<Self>,
        audio: PathBuf,
        lines: Vec<LineRequest>,
        threads: usize,
    ) -> Result<HitTimes> {
        if !self.installed() {
            bail!("компоненты анализа звука не установлены");
        }
        let _permit = self.run.acquire().await?;
        let me = self.clone();
        let started = Instant::now();
        let res = tokio::task::spawn_blocking(move || me.refine_blocking(&audio, &lines, threads)).await??;
        tracing::info!("анализ звука: {} нарушений за {:.1} с", res.len(), started.elapsed().as_secs_f64());
        Ok(res)
    }

    fn refine_blocking(&self, audio_path: &Path, lines: &[LineRequest], threads: usize) -> Result<HitTimes> {
        let mut guard = self.engine.lock().unwrap();
        if guard.is_none() {
            tracing::info!("анализ звука: загружаю модели…");
            *guard = Some((self.load(threads)?, Instant::now()));
            self.bump();
        }
        let (engine, used) = guard.as_mut().expect("модели загружены");
        *used = Instant::now();

        let pcm = audio::decode(audio_path)?;
        let stereo = pcm.to_stereo(separate::SAMPLE_RATE);
        let total = stereo[0].len();
        let sr = separate::SAMPLE_RATE as f64;
        let mut out = HitTimes::new();

        // Окна строк; пересекающиеся объединяем, чтобы выделять вокал один раз.
        let windows: Vec<(usize, usize)> = lines
            .iter()
            .map(|l| {
                let w0 = ((l.start - WINDOW_PAD).max(0.0) * sr) as usize;
                let w1 = (((l.end + WINDOW_PAD) * sr) as usize).min(total);
                (w0.min(w1), w1)
            })
            .collect();
        let groups = merge_windows(&windows);
        let mut vocals_cache: Vec<Option<[Vec<f32>; 2]>> = vec![None; groups.len()];

        for (li, line) in lines.iter().enumerate() {
            let (w0, w1) = windows[li];
            if w1 <= w0 + 1600 {
                continue;
            }
            let gi = groups.iter().position(|&(g0, g1)| g0 <= w0 && w1 <= g1).expect("окно входит в группу");
            let (g0, g1) = groups[gi];
            if vocals_cache[gi].is_none() {
                let window: [Vec<f32>; 2] = std::array::from_fn(|c| stereo[c][g0..g1].to_vec());
                vocals_cache[gi] =
                    Some(separate::separate(&engine.stft, &window, |x| run_vocals(&mut engine.vocals, x))?);
            }
            let group_vocals = vocals_cache[gi].as_ref().expect("вокал выделен");
            let vocals: [Vec<f32>; 2] = std::array::from_fn(|c| group_vocals[c][w0 - g0..w1 - g0].to_vec());
            let voice16 = audio::resample(&audio::mono(&vocals), separate::SAMPLE_RATE, align::SAMPLE_RATE);
            let emissions = run_aligner(&mut engine.aligner, &align::normalize(&voice16))?;

            // Слова строки с их позициями в символах.
            let chars: Vec<char> = line.text.chars().collect();
            let mut words: Vec<(usize, usize, String)> = Vec::new();
            let mut i = 0;
            while i < chars.len() {
                if chars[i].is_whitespace() {
                    i += 1;
                    continue;
                }
                let s = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                words.push((s, i, chars[s..i].iter().collect()));
            }
            let word_refs: Vec<&str> = words.iter().map(|w| w.2.as_str()).collect();
            let spans = align::align_words(&emissions, &word_refs, &engine.vocab);

            let offset = w0 as f64 / sr;
            for &(hs, he) in &line.hits {
                let found: Vec<&align::WordSpan> = words
                    .iter()
                    .zip(&spans)
                    .filter(|((ws, we, _), _)| *ws < he && hs < *we)
                    .filter_map(|(_, sp)| sp.as_ref())
                    .collect();
                if found.is_empty() {
                    continue;
                }
                // Куски слова, начавшиеся уже после конца строки (по LRC), — это чужие слова,
                // в которых модель «нашла» нерасслышанную букву. Первый кусок оставляем всегда.
                let line_end = line.end - offset + LINE_END_SLACK;
                let pieces: Vec<(f64, f64)> = found
                    .iter()
                    .flat_map(|w| {
                        w.pieces
                            .iter()
                            .enumerate()
                            .filter(|(i, p)| *i == 0 || p.0 <= line_end)
                            .map(|(_, p)| *p)
                    })
                    .collect();
                let start = pieces.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
                let end = pieces.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
                let conf = found.iter().map(|s| s.confidence).fold(f32::INFINITY, f32::min);
                let dur = end - start;
                if conf < MIN_CONFIDENCE || !(0.04..=3.0).contains(&dur) {
                    tracing::debug!(
                        "слово в строке {} найдено неуверенно ({conf:.2}, {dur:.2} с)",
                        line.index
                    );
                    continue;
                }
                out.insert((line.index, hs), (offset + start, offset + end));
            }
        }
        Ok(out)
    }
}

/// Объединяет пересекающиеся окна `(начало, конец)` в группы.
fn merge_windows(windows: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut sorted: Vec<(usize, usize)> = windows.to_vec();
    sorted.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in sorted {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn run_vocals(session: &mut Session, input: Vec<f32>) -> Result<Vec<f32>> {
    let shape = [1usize, 4, separate::DIM_F, separate::DIM_T];
    let tensor = Tensor::from_array((shape, input)).map_err(|e| anyhow!("{e}"))?;
    let outputs =
        session.run(ort::inputs!["input" => tensor]).map_err(|e| anyhow!("выделение вокала: {e}"))?;
    let (_, data) = outputs["output"].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
    Ok(data.to_vec())
}

fn run_aligner(session: &mut Session, audio: &[f32]) -> Result<Vec<Vec<f32>>> {
    let tensor = Tensor::from_array(([1usize, audio.len()], audio.to_vec())).map_err(|e| anyhow!("{e}"))?;
    let outputs =
        session.run(ort::inputs!["input_values" => tensor]).map_err(|e| anyhow!("выравнивание: {e}"))?;
    let (shape, data) = outputs["logits"].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
    let n_tokens = *shape.last().context("пустой ответ модели")? as usize;
    Ok(align::log_softmax(data, n_tokens))
}

/// Скачать недостающие компоненты в фоне; прогресс — через `status()`.
pub fn install(state: &Arc<AppState>) -> Result<()> {
    let svc = &state.analysis;
    {
        let mut st = svc.install.lock().unwrap();
        if matches!(*st, InstallState::Downloading { .. }) {
            bail!("загрузка уже идёт");
        }
        *st = InstallState::Downloading { asset: String::new(), done: 0, total: 0 };
    }
    svc.bump();
    let state = state.clone();
    tokio::spawn(async move {
        let svc = &state.analysis;
        let mut result = Ok(());
        for a in assets::all() {
            if assets::installed(&svc.dir, &a) {
                continue;
            }
            tracing::info!("анализ звука: скачиваю {}", a.label);
            let mut last = Instant::now();
            let r = assets::download(&state.http, &svc.dir, &a, |done| {
                // Не чаще 4 раз в секунду, чтобы не заваливать панель сообщениями.
                if last.elapsed() > Duration::from_millis(250) || done == a.size {
                    last = Instant::now();
                    *svc.install.lock().unwrap() =
                        InstallState::Downloading { asset: a.label.to_string(), done, total: a.size };
                    svc.bump();
                }
            })
            .await;
            if let Err(e) = r {
                result = Err(e);
                break;
            }
        }
        *svc.install.lock().unwrap() = match result {
            Ok(()) => {
                tracing::info!("анализ звука: все компоненты установлены");
                InstallState::Idle
            }
            Err(e) => InstallState::Failed { message: format!("{e:#}") },
        };
        svc.bump();
    });
    Ok(())
}

/// Потоки для анализа: половина ядер, чтобы не просаживать игру.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map(|n| (n.get() / 2).max(1)).unwrap_or(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_windows_are_merged() {
        assert_eq!(merge_windows(&[(10, 20), (0, 5), (15, 30), (40, 50)]), vec![(0, 5), (10, 30), (40, 50)]);
    }
}
