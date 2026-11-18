//! Запуск ядра без окна: для разработки, сервера или слабого ПК.
//! Панель открывается в любом браузере по ссылке из лога.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let mut opts = supstream_core::StartOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-dir" => opts.data_dir = args.next().map(Into::into),
            "--port" => opts.port = args.next().and_then(|p| p.parse().ok()),
            "-h" | "--help" => {
                println!("supstream-headless [--data-dir ПАПКА] [--port ПОРТ]");
                return Ok(());
            }
            other => anyhow::bail!("неизвестный аргумент: {other}"),
        }
    }

    let running = supstream_core::start(opts).await?;
    tracing::info!("панель: {}", running.panel_url());
    tokio::signal::ctrl_c().await?;
    tracing::info!("остановка");
    running.shutdown();
    Ok(())
}
