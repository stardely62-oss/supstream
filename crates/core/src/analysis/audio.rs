//! Декодирование звука в PCM без ffmpeg.
//!
//! * WebM/Ogg с Opus (так отдаёт YouTube) — контейнер разбирает Symphonia, звук декодирует libopus;
//! * mp3, flac, wav, ogg/vorbis, m4a/aac (своя папка) — целиком Symphonia.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, bail};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Звук: каналы по отдельности.
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Pcm {
    pub fn duration(&self) -> f64 {
        self.channels.first().map(|c| c.len()).unwrap_or(0) as f64 / self.sample_rate as f64
    }

    /// Стерео нужной частоты (моно дублируется, лишние каналы сводятся).
    pub fn to_stereo(&self, rate: u32) -> [Vec<f32>; 2] {
        let (l, r) = match self.channels.len() {
            0 => (Vec::new(), Vec::new()),
            1 => (self.channels[0].clone(), self.channels[0].clone()),
            _ => (self.channels[0].clone(), self.channels[1].clone()),
        };
        [resample(&l, self.sample_rate, rate), resample(&r, self.sample_rate, rate)]
    }
}

/// Линейная передискретизация. Для анализа речи её точности достаточно.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let n = (x.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let pos = i as f64 * step;
            let j = pos as usize;
            let frac = (pos - j as f64) as f32;
            let a = x[j.min(x.len() - 1)];
            let b = x[(j + 1).min(x.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Сведение в моно.
pub fn mono(ch: &[Vec<f32>]) -> Vec<f32> {
    let n = ch.iter().map(|c| c.len()).min().unwrap_or(0);
    (0..n).map(|i| ch.iter().map(|c| c[i]).sum::<f32>() / ch.len() as f32).collect()
}

pub fn decode(path: &Path) -> Result<Pcm> {
    let file = File::open(path).with_context(|| format!("не открыть {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .context("неизвестный формат звука")?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .context("в файле нет звуковой дорожки")?
        .clone();
    let params = &track.codec_params;
    let n_ch = params.channels.map(|c| c.count()).unwrap_or(2).clamp(1, 2);

    if params.codec == CODEC_TYPE_OPUS {
        // Opus всегда декодируется в 48 кГц. Pre-skip — служебные сэмплы в начале (из OpusHead).
        let pre_skip = params
            .extra_data
            .as_deref()
            .filter(|d| d.len() >= 12 && d.starts_with(b"OpusHead"))
            .map(|d| u16::from_le_bytes([d[10], d[11]]) as usize)
            .unwrap_or(0);
        let channels = if n_ch == 1 { opus::Channels::Mono } else { opus::Channels::Stereo };
        let mut dec = opus::Decoder::new(48_000, channels).context("libopus")?;
        let mut out = vec![vec![]; n_ch];
        let mut buf = vec![0f32; 5760 * n_ch]; // максимум 120 мс
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymError::ResetRequired) => break,
                Err(e) => return Err(e).context("ошибка чтения звука"),
            };
            if packet.track_id() != track.id {
                continue;
            }
            let n = match dec.decode_float(&packet.data, &mut buf, false) {
                Ok(n) => n,
                Err(_) => continue, // битый пакет — пропускаем
            };
            for i in 0..n {
                for (c, ch) in out.iter_mut().enumerate() {
                    ch.push(buf[i * n_ch + c]);
                }
            }
        }
        for ch in &mut out {
            ch.drain(..pre_skip.min(ch.len()));
        }
        return Ok(Pcm { sample_rate: 48_000, channels: out });
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(params, &DecoderOptions::default())
        .context("этот кодек не поддерживается")?;
    let mut rate = params.sample_rate.unwrap_or(44_100);
    let mut out: Vec<Vec<f32>> = vec![vec![]; n_ch];
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(e).context("ошибка чтения звука"),
        };
        if packet.track_id() != track.id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e).context("ошибка декодирования"),
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        let src_ch = spec.channels.count();
        let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sb.copy_interleaved_ref(decoded);
        for frame in sb.samples().chunks(src_ch) {
            for (c, ch) in out.iter_mut().enumerate() {
                ch.push(frame[c.min(src_ch - 1)]);
            }
        }
    }
    if out[0].is_empty() {
        bail!("в файле нет звука");
    }
    Ok(Pcm { sample_rate: rate, channels: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный WAV (16 бит) для проверки декодера.
    fn write_wav(path: &Path, rate: u32, samples: &[i16]) {
        let mut b = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // моно
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn decodes_wav_and_resamples() {
        let path = std::env::temp_dir().join(format!("supstream-{}.wav", uuid::Uuid::new_v4()));
        let samples: Vec<i16> = (0..22050).map(|i| ((i as f32 * 0.05).sin() * 10000.0) as i16).collect();
        write_wav(&path, 22050, &samples);
        let pcm = decode(&path).unwrap();
        assert_eq!(pcm.sample_rate, 22050);
        assert_eq!(pcm.channels.len(), 1);
        assert!((pcm.duration() - 1.0).abs() < 0.01);
        let [l, r] = pcm.to_stereo(44100);
        assert_eq!(l.len(), 44100);
        assert_eq!(l, r);
        assert!((l[2000] - pcm.channels[0][1000]).abs() < 1e-3);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn resample_keeps_shape() {
        let x: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let y = resample(&x, 1000, 500);
        assert_eq!(y.len(), 500);
        assert!((y[100] - 200.0).abs() < 1e-3);
        assert_eq!(mono(&[vec![1.0, 3.0], vec![3.0, 5.0]]), vec![2.0, 4.0]);
    }
}
