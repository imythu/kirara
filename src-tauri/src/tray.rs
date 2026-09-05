use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

pub fn show_main_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.show();

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "tray-open", "打开主窗口", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;
    let icon = app
        .default_window_icon()
        .ok_or_else(|| tauri::Error::InvalidIcon(std::io::Error::other("缺少 Kirara 应用图标")))?;

    TrayIconBuilder::with_id("main")
        .icon(icon.clone())
        .tooltip("云母 · Kirara")
        .menu(&menu)
        // macOS menu bar icons open their menu on either mouse button.
        // On Windows, left-click restores the window and right-click opens the menu.
        .show_menu_on_left_click(!cfg!(target_os = "windows"))
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-open" => show_main_window(app),
            // Keep the existing ExitRequested handler responsible for stopping the service.
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if cfg!(target_os = "windows")
                && matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                )
            {
                show_main_window(tray.app_handle());
            }
        })
        // Tauri registers and retains the icon until the application exits.
        .build(app)?;

    Ok(())
}
