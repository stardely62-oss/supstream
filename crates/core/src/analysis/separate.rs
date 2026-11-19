//! Выделение вокала моделью MDX-Net (Kim_Vocal_2).
//!
//! Модель работает со спектрограммой: STFT → модель → обратное STFT. Звук режется на куски
//! по 255 кадров с перекрытием (`TRIM` с каждой стороны), чтобы на стыках не было артефактов.
//! Параметры — как у Kim_Vocal_2 в UVR: n_fft 7680, hop 1024, 3072 частотных полосы.

use std::sync::Arc;

use anyhow::Result;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex32;

pub const SAMPLE_RATE: u32 = 44_100;
pub const N_FFT: usize = 7680;
pub const HOP: usize = 1024;
pub const DIM_F: usize = 3072;
pub const DIM_T: usize = 256;
pub const CHUNK: usize = HOP * (DIM_T - 1);
pub const TRIM: usize = N_FFT / 2;
pub const GEN: usize = CHUNK - 2 * TRIM;
const BINS: usize = N_FFT / 2 + 1;

pub struct Stft {
    window: Vec<f32>,
    fwd: Arc<dyn realfft::RealToComplex<f32>>,
    inv: Arc<dyn realfft::ComplexToReal<f32>>,
}

impl Default for Stft {
    fn default() -> Self {
        Self::new()
    }
}

impl Stft {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        // Периодическое окно Ханна (как torch.hann_window).
        let window = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
            .collect();
        Self { window, fwd: planner.plan_fft_forward(N_FFT), inv: planner.plan_fft_inverse(N_FFT) }
    }

    /// Кусок длиной `CHUNK` (2 канала) → вход модели `[4, DIM_F, DIM_T]`
    /// (Re левого, Re правого, Im левого, Im правого).
    pub fn forward(&self, chunk: &[Vec<f32>; 2]) -> Vec<f32> {
        let mut out = vec![0f32; 4 * DIM_F * DIM_T];
        let mut frame = self.fwd.make_input_vec();
        let mut spec = self.fwd.make_output_vec();
        for (c, x) in chunk.iter().enumerate() {
            let padded = reflect_pad(x, N_FFT / 2);
            for t in 0..DIM_T {
                let off = t * HOP;
                for i in 0..N_FFT {
                    frame[i] = padded[off + i] * self.window[i];
                }
                self.fwd.process(&mut frame, &mut spec).expect("размеры БПФ");
                for f in 0..DIM_F {
                    out[(c * DIM_F + f) * DIM_T + t] = spec[f].re;
                    out[((c + 2) * DIM_F + f) * DIM_T + t] = spec[f].im;
                }
            }
        }
        out
    }

    /// Выход модели `[4, DIM_F, DIM_T]` → звук длиной `CHUNK` (2 канала).
    pub fn inverse(&self, y: &[f32]) -> [Vec<f32>; 2] {
        let total = N_FFT + HOP * (DIM_T - 1);
        let mut wsum = vec![0f32; total];
        for t in 0..DIM_T {
            for i in 0..N_FFT {
                wsum[t * HOP + i] += self.window[i] * self.window[i];
            }
        }
        let mut spec = vec![Complex32::new(0.0, 0.0); BINS];
        let mut frame = self.inv.make_output_vec();
        let mut res: [Vec<f32>; 2] = [vec![], vec![]];
        for (c, out_ch) in res.iter_mut().enumerate() {
            let mut acc = vec![0f32; total];
            for t in 0..DIM_T {
                for (f, s) in spec.iter_mut().enumerate() {
                    *s = if f < DIM_F {
                        Complex32::new(y[(c * DIM_F + f) * DIM_T + t], y[((c + 2) * DIM_F + f) * DIM_T + t])
                    } else {
                        Complex32::new(0.0, 0.0)
                    };
                }
                // Мнимые части крайних полос должны быть нулевыми, иначе realfft вернёт ошибку.
                spec[0].im = 0.0;
                spec[BINS - 1].im = 0.0;
                self.inv.process(&mut spec, &mut frame).expect("размеры БПФ");
                let off = t * HOP;
                for i in 0..N_FFT {
                    acc[off + i] += frame[i] / N_FFT as f32 * self.window[i];
                }
            }
            *out_ch = (0..CHUNK)
                .map(|i| {
                    let j = i + N_FFT / 2;
                    acc[j] / wsum[j].max(1e-8)
                })
                .collect();
        }
        res
    }
}

fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    let mut out = Vec::with_capacity(n + 2 * pad);
    for i in (1..=pad).rev() {
        out.push(x[i.min(n - 1)]);
    }
    out.extend_from_slice(x);
    for i in 0..pad {
        out.push(x[n.saturating_sub(2 + i)]);
    }
    out
}

/// Пропускает стерео-звук через модель кусками и склеивает результат.
/// `model` получает вход `[4, DIM_F, DIM_T]` и возвращает выход той же формы.
pub fn separate(
    stft: &Stft,
    audio: &[Vec<f32>; 2],
    mut model: impl FnMut(Vec<f32>) -> Result<Vec<f32>>,
) -> Result<[Vec<f32>; 2]> {
    let n = audio[0].len();
    let pad = GEN - n % GEN;
    let padded: [Vec<f32>; 2] = std::array::from_fn(|c| {
        let mut v = vec![0f32; TRIM];
        v.extend_from_slice(&audio[c]);
        v.resize(TRIM + n + pad + TRIM, 0.0);
        v
    });
    let mut out: [Vec<f32>; 2] = [vec![0f32; n + pad], vec![0f32; n + pad]];
    let mut start = 0;
    while start < n + pad {
        let chunk: [Vec<f32>; 2] = std::array::from_fn(|c| padded[c][start..start + CHUNK].to_vec());
        let y = model(stft.forward(&chunk))?;
        let wav = stft.inverse(&y);
        for c in 0..2 {
            out[c][start..start + GEN].copy_from_slice(&wav[c][TRIM..TRIM + GEN]);
        }
        start += GEN;
    }
    for ch in &mut out {
        ch.truncate(n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Если «модель» возвращает вход как есть, на выходе должен получиться исходный звук
    /// (без полос выше DIM_F — это ~17.6 кГц, их тест не использует).
    #[test]
    fn identity_model_reconstructs_audio() {
        let stft = Stft::new();
        let n = 600_000; // ~13.6 с — три куска, проверяем и стыки
        let make = |f: f32| -> Vec<f32> {
            (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / SAMPLE_RATE as f32).sin() * 0.5)
                .collect()
        };
        let audio = [make(440.0), make(1000.0)];
        let out = separate(&stft, &audio, Ok).unwrap();
        assert_eq!(out[0].len(), n);
        // На краях сигнал обрывается резко — часть энергии уходит в отброшенные полосы
        // выше 17.6 кГц. Внутри (в том числе на стыках кусков) восстановление должно быть точным.
        let err = |c: usize, from: usize, to: usize| {
            audio[c][from..to].iter().zip(&out[c][from..to]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max)
        };
        let edge = N_FFT;
        let inner = (err(0, edge, n - edge), err(1, edge, n - edge));
        // На стыках кусков остаётся ~0.2% от амплитуды: края окна видят отражённый сигнал,
        // а полосы выше DIM_F отбрасываются — так же устроен MDX в UVR, на слух неразличимо.
        assert!(
            inner.0 < 1.5e-3 && inner.1 < 1.5e-3,
            "ошибка внутри: {inner:?}; на краях: {} {}",
            err(0, 0, n),
            err(1, 0, n)
        );
    }

    #[test]
    fn reflect_padding() {
        assert_eq!(reflect_pad(&[1.0, 2.0, 3.0, 4.0], 2), vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0]);
    }
}
