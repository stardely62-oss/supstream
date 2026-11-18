//! Десктоп-оболочка: запускает ядро, показывает панель в окне и живёт в трее.
//!
//! Окно просто открывает локальную панель (http://127.0.0.1:7272) — тот же
//! интерфейс доступен из браузера и с телефона, IPC Tauri не нужен.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const MAIN: &str = "main";

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let started = tauri::async_runtime::block_on(supstream_core::start(Default::default()));

    tauri::Builder::default()
        // Второй запуск exe просто показывает уже открытое окно.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let running = match started {
                Ok(r) => r,
                Err(e) => {
                    app.dialog()
                        .message(format!("{e:#}"))
                        .title("Supstream не запустился")
                        .kind(MessageDialogKind::Error)
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            let url = running.panel_url().parse()?;
            app.manage(running);

            WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(url))
                .title("Supstream")
                .inner_size(1180.0, 760.0)
                .min_inner_size(380.0, 520.0)
                .build()?;

            let open = MenuItem::with_id(app, "open", "Открыть панель", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("нет иконки"))
                .tooltip("Supstream")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        // Крестик прячет окно в трей: ядро продолжает работать во время стрима.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("ошибка запуска Tauri");
}
