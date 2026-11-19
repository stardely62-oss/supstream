//! Проверка точного глушения на настоящих моделях. Запускается вручную:
//!
//! ```text
//! SUPSTREAM_ORT_DYLIB=/путь/libonnxruntime.so \
//! SUPSTREAM_TEST_DATA=/папка/с/models \
//! SUPSTREAM_TEST_TRACK=/путь/mix.webm SUPSTREAM_TEST_TRUTH=/путь/truth.json \
//! cargo test -p supstream-core --test analysis_real -- --ignored --nocapture
//! ```
//!
//! `truth.json`: `{ "lines": [{ "time", "text" }], "truth": [{ "word", "start", "end" }] }` —
//! трек, где точное время каждого слова известно.

use std::sync::Arc;

use serde_json::Value;
use supstream_core::analysis::{AnalysisService, LineRequest, PAD_AFTER, PAD_BEFORE};
use supstream_core::music::filter::Filter;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "нужны настоящие модели (~400 МБ)"]
async fn precise_mutes_cover_every_bad_word() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("нужна переменная {k}"));
    let data = var("SUPSTREAM_TEST_DATA");
    let track = var("SUPSTREAM_TEST_TRACK");
    let truth: Value =
        serde_json::from_str(&std::fs::read_to_string(var("SUPSTREAM_TEST_TRUTH")).unwrap()).unwrap();

    let svc = Arc::new(AnalysisService::new(std::path::Path::new(&data)));
    assert!(svc.installed(), "в {data}/models нет моделей");

    let filter = Filter::with_custom(&[], &[]);
    let lines = truth["lines"].as_array().unwrap();
    let mut requests = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let text = l["text"].as_str().unwrap().to_string();
        let hits: Vec<(usize, usize)> = filter.scan(&text).iter().map(|h| (h.start, h.end)).collect();
        if hits.is_empty() {
            continue;
        }
        let start = l["time"].as_f64().unwrap();
        let end = lines.get(i + 1).and_then(|n| n["time"].as_f64()).unwrap_or(start + 4.0);
        requests.push(LineRequest { index: i, start, end, text, hits });
    }

    let t = std::time::Instant::now();
    let times = svc.refine(track.clone().into(), requests.clone(), 4).await.unwrap();
    println!("анализ: {:.1} с (с загрузкой моделей)", t.elapsed().as_secs_f64());

    let mut covered = 0;
    let mut total = 0;
    for r in &requests {
        let chars: Vec<char> = r.text.chars().collect();
        for &(hs, he) in &r.hits {
            total += 1;
            let word: String = chars[hs..he].iter().collect();
            let g = truth["truth"].as_array().unwrap().iter().find(|t| t["word"] == word.as_str()).unwrap();
            let (gs, ge) = (g["start"].as_f64().unwrap(), g["end"].as_f64().unwrap());
            match times.get(&(r.index, hs)) {
                Some(&(s, e)) => {
                    let ok = s - PAD_BEFORE <= gs && e + PAD_AFTER >= ge;
                    covered += ok as usize;
                    println!(
                        "{word:8} настоящее {gs:6.2}–{ge:5.2}  найдено {s:6.2}–{e:5.2}  отметка {:.2} с  {}",
                        e + PAD_AFTER - (s - PAD_BEFORE),
                        if ok { "покрыто" } else { "НЕ ПОКРЫТО" }
                    );
                }
                None => println!("{word:8} не найдено — будет глушиться по оценке строки"),
            }
        }
    }
    assert_eq!(covered, total, "каждое запрещённое слово должно быть закрыто отметкой");

    // Повторный запуск — модели уже в памяти, должно быть быстрее.
    let t = std::time::Instant::now();
    svc.refine(track.into(), requests, 4).await.unwrap();
    println!("повторный анализ: {:.1} с", t.elapsed().as_secs_f64());
}
