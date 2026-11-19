//! Компоненты анализа звука: модели и ONNX Runtime. Скачиваются один раз по кнопке в панели,
//! каждый файл сверяется по SHA-256 — подменённый или битый файл не будет использован.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy)]
pub enum Source {
    Url(&'static str),
    /// Один файл из zip-архива (пакета NuGet): качаем только его, через HTTP Range.
    ZipMember {
        url: &'static str,
        member: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Asset {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub source: Source,
}

pub const VOCALS: Asset = Asset {
    id: "vocals",
    label: "Выделение вокала (Kim Vocal 2, MDX-Net)",
    file: "kim_vocal_2.onnx",
    size: 66_759_214,
    sha256: "ce74ef3b6a6024ce44211a07be9cf8bc6d87728cc852a68ab34eb8e58cde9c8b",
    source: Source::Url(concat!(
        "https://huggingface.co",
        "/Acapellas/Kim_Vocal_2.onnx/resolve/main/Kim_Vocal_2.onnx"
    )),
};

pub const ALIGNER: Asset = Asset {
    id: "aligner",
    label: "Выравнивание текста (MMS 300M, int8)",
    file: "mms_fa_int8.onnx",
    size: 317_341_664,
    sha256: "2eb5c3d2f6db2ef476aa7a7e1e5800145973e9064eb5292b1b9b8ada1207712a",
    source: Source::Url(concat!(
        "https://huggingface.co",
        "/onnx-community/mms-300m-1130-forced-aligner-ONNX/resolve/main/onnx/model_int8.onnx"
    )),
};

pub const ALIGNER_VOCAB: Asset = Asset {
    id: "aligner_vocab",
    label: "Словарь выравнивания",
    file: "mms_fa_vocab.json",
    size: 351,
    sha256: "0ff83fc9d044537f7c72d92db15b04917f1e50cada799957963bca61e4305971",
    source: Source::Url(concat!(
        "https://huggingface.co",
        "/onnx-community/mms-300m-1130-forced-aligner-ONNX/resolve/main/vocab.json"
    )),
};

/// ONNX Runtime для Windows (CPU) из официального пакета NuGet Microsoft.ML.OnnxRuntime.
pub const RUNTIME_WINDOWS: Asset = Asset {
    id: "runtime",
    label: "ONNX Runtime 1.24.4",
    file: "onnxruntime.dll",
    size: 14_203_464,
    sha256: "b95efb2113b603bbbf3f191061c5516a871ed546893c820e4f3b7b6c358dbf2a",
    source: Source::ZipMember {
        url: "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.24.4/microsoft.ml.onnxruntime.1.24.4.nupkg",
        member: "runtimes/win-x64/native/onnxruntime.dll",
    },
};

pub fn all() -> Vec<Asset> {
    let mut v = vec![VOCALS, ALIGNER, ALIGNER_VOCAB];
    if cfg!(windows) {
        v.insert(0, RUNTIME_WINDOWS);
    }
    v
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub size: u64,
    pub installed: bool,
}

pub fn path(dir: &Path, a: &Asset) -> PathBuf {
    dir.join(a.file)
}

/// Установлен, если файл на месте и нужного размера (полную сверку SHA делаем при скачивании).
pub fn installed(dir: &Path, a: &Asset) -> bool {
    std::fs::metadata(path(dir, a)).map(|m| m.len() == a.size).unwrap_or(false)
}

/// Скачивает компонент. `progress(скачано)` вызывается по мере загрузки.
pub async fn download(
    http: &reqwest::Client,
    dir: &Path,
    a: &Asset,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let dest = path(dir, a);
    let tmp = dest.with_extension("part");
    let bytes_hash = match a.source {
        Source::Url(url) => stream_to_file(http, url, &tmp, &mut progress).await?,
        Source::ZipMember { url, member } => {
            let data = zip_member(http, url, member).await?;
            progress(data.len() as u64);
            tokio::fs::write(&tmp, &data).await?;
            hex(&Sha256::digest(&data))
        }
    };
    if bytes_hash != a.sha256 {
        let _ = tokio::fs::remove_file(&tmp).await;
        bail!("{}: контрольная сумма не совпала — файл повреждён или подменён", a.label);
    }
    tokio::fs::rename(&tmp, &dest).await?;
    Ok(())
}

async fn stream_to_file(
    http: &reqwest::Client,
    url: &str,
    tmp: &Path,
    progress: &mut impl FnMut(u64),
) -> Result<String> {
    let resp = http
        .get(url)
        .timeout(std::time::Duration::from_secs(3600))
        .send()
        .await
        .with_context(|| format!("не удалось скачать {url}"))?
        .error_for_status()?;
    let mut file = tokio::fs::File::create(tmp).await?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("загрузка прервалась")?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress(done);
    }
    file.flush().await?;
    Ok(hex(&hasher.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn range(http: &reqwest::Client, url: &str, from: u64, to: u64) -> Result<Vec<u8>> {
    let resp = http
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={from}-{to}"))
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await?
        .error_for_status()?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        bail!("сервер не поддерживает частичную загрузку");
    }
    Ok(resp.bytes().await?.to_vec())
}

fn u16le(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}
fn u32le(b: &[u8], at: usize) -> u64 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as u64
}

/// Достаёт один файл из zip по HTTP: читаем оглавление в конце архива, затем только нужные байты.
async fn zip_member(http: &reqwest::Client, url: &str, member: &str) -> Result<Vec<u8>> {
    let head = http.head(url).send().await?.error_for_status()?;
    let size = head.content_length().context("неизвестен размер архива")?;
    let tail_len = size.min(65_536 + 22);
    let tail = range(http, url, size - tail_len, size - 1).await?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .context("архив повреждён: нет оглавления")?;
    let cd_size = u32le(&tail, eocd + 12);
    let cd_off = u32le(&tail, eocd + 16);
    let cd = range(http, url, cd_off, cd_off + cd_size - 1).await?;
    let mut p = 0;
    while p + 46 <= cd.len() {
        let method = u16le(&cd, p + 10);
        let csize = u32le(&cd, p + 20);
        let usize_ = u32le(&cd, p + 24) as usize;
        let (nlen, elen, clen) = (u16le(&cd, p + 28), u16le(&cd, p + 30), u16le(&cd, p + 32));
        let off = u32le(&cd, p + 42);
        let name = std::str::from_utf8(&cd[p + 46..p + 46 + nlen]).unwrap_or_default();
        if name == member {
            let lh = range(http, url, off, off + 29).await?;
            let data_off = off + 30 + u16le(&lh, 26) as u64 + u16le(&lh, 28) as u64;
            let raw = range(http, url, data_off, data_off + csize - 1).await?;
            return match method {
                0 => Ok(raw),
                8 => {
                    use std::io::Read;
                    let mut out = Vec::with_capacity(usize_);
                    flate2::read::DeflateDecoder::new(&raw[..]).read_to_end(&mut out)?;
                    Ok(out)
                }
                m => bail!("неподдерживаемое сжатие в архиве ({m})"),
            };
        }
        p += 46 + nlen + elen + clen;
    }
    bail!("в архиве нет {member}")
}
