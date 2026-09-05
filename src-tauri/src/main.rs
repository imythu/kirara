#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod sse;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bridge::{Backend, BackendRuntime};
use kirara::{ListenEndpoint, ServerOptions};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

async fn start_backend(base_dir: std::path::PathBuf) -> Result<BackendRuntime, String> {
    #[cfg(unix)]
    let (listen, socket_dir) = {
        // macOS app data and TMPDIR paths can exceed sockaddr_un's length limit.
        // tempfile creates a random, owner-only directory in this short parent.
        let directory = tempfile::Builder::new()
            .prefix("kirara-")
            .tempdir_in("/tmp")
            .map_err(|error| error.to_string())?;
        let endpoint = ListenEndpoint::Unix(directory.path().join("api.sock"));
        (endpoint, Some(directory))
    };
    #[cfg(windows)]
    let (listen, socket_dir) = (
        ListenEndpoint::NamedPipe(format!(r"\\.\pipe\kirara-{}", uuid::Uuid::new_v4())),
        None,
    );
    let server = kirara::start(ServerOptions {
        db_dir: base_dir.clone(),
        base_dir,
        listen,
    })
    .await
    .map_err(|error| format!("无法启动 Kirara：{error}"))?;
    Ok(BackendRuntime { server, socket_dir })
}

fn is_app_url(url: &tauri::Url) -> bool {
    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(5173))
}

fn open_external(app: &tauri::AppHandle, url: &tauri::Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto") {
        if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
            app.dialog()
                .message(format!("无法打开链接：{error}"))
                .title("Kirara")
                .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                .show(|_| {});
        }
    }
}

fn main() {
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        // Native navigation handlers cover both anchors and window.open.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            bridge::api_request,
            bridge::logs_open,
            bridge::logs_close,
        ])
        .setup(|app| {
            let startup = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())
                .and_then(|directory| tauri::async_runtime::block_on(start_backend(directory)));
            let state = Arc::new(Backend::new(startup));
            app.manage(state.clone());
            let navigation_app = app.handle().clone();
            let new_window_app = app.handle().clone();
            let page_state = state.clone();
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("云母 · Kirara")
                    .inner_size(1280.0, 860.0)
                    .min_inner_size(800.0, 600.0)
                    .on_navigation(move |url| {
                        if is_app_url(url) {
                            true
                        } else {
                            open_external(&navigation_app, url);
                            false
                        }
                    })
                    .on_new_window(move |url, _| {
                        if is_app_url(&url) {
                            if let Some(window) = new_window_app.get_webview_window("main") {
                                let _ = window.navigate(url);
                            }
                        } else {
                            open_external(&new_window_app, &url);
                        }
                        tauri::webview::NewWindowResponse::Deny
                    })
                    .on_page_load(move |_, payload| {
                        if payload.event() == tauri::webview::PageLoadEvent::Started {
                            page_state.close_streams();
                        }
                    })
                    .build();
            if let Err(error) = window {
                let _ = tauri::async_runtime::block_on(state.shutdown());
                return Err(error.into());
            }
            if let Some(error) = &state.startup_error {
                app.dialog()
                    .message(error)
                    .title("Kirara 启动失败")
                    .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                    .show(|_| {});
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the desktop application");

    let exiting = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    application.run(move |app, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if stopped.load(Ordering::Acquire) {
                return;
            }
            api.prevent_exit();
            if exiting.swap(true, Ordering::AcqRel) {
                return;
            }
            let state = app.state::<Arc<Backend>>().inner().clone();
            let app = app.clone();
            let stopped = stopped.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = state.shutdown().await {
                    // blocking_show is safe here: this runs outside the UI thread.
                    app.dialog()
                        .message(format!("服务退出时发生错误：{error}"))
                        .title("Kirara")
                        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                        .blocking_show();
                }
                stopped.store(true, Ordering::Release);
                app.exit(code.unwrap_or(0));
            });
        }
    });
}
