//! Tauri shell of the LumaNote desktop assistant.
//!
//! Three windows, a system tray, single instance and a few window-management commands.
//! There is deliberately no business logic here (handbook 3.1).
//!
//! Window lifecycle (the only way the process ends is the tray item "結束"):
//!  - `floating`: always there; the user cannot close it (close requests are swallowed).
//!  - `main`: closing it only hides it; tray / sprite / second launch show it again.
//!  - `settings`: created on demand; closing it destroys just that window.
//!  - If the last window were ever destroyed anyway, `RunEvent::ExitRequested { code: None }`
//!    is prevented in `run()`; only `app.exit(..)` (code `Some`) ends the process.

mod floating;
mod geometry;

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

pub(crate) const LABEL_FLOATING: &str = "floating";
const LABEL_MAIN: &str = "main";
const LABEL_SETTINGS: &str = "settings";

/// Fallback so a floating window that never finishes loading still appears.
const FLOATING_SHOW_FALLBACK: std::time::Duration = std::time::Duration::from_secs(5);

/// Tray toggles. They only remember the state for now; nothing reads them yet.
#[derive(Default)]
struct ShellState {
    observation_paused: AtomicBool,
    do_not_disturb: AtomicBool,
}

/// Error shape that matches `IpcErrorPayload` in `src/lib/ipc-types.ts`.
#[derive(Debug, Serialize)]
pub(crate) struct CommandError {
    kind: &'static str,
    message: String,
    retryable: bool,
}

impl CommandError {
    pub(crate) fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: false,
        }
    }
}

#[derive(Debug, Serialize)]
struct AppInfo {
    app_name: &'static str,
    app_version: &'static str,
    tauri_version: &'static str,
}

/// Version information for the About / settings screens.
#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        app_name: "LumaNote",
        app_version: env!("CARGO_PKG_VERSION"),
        tauri_version: tauri::VERSION,
    }
}

/// Opens (or focuses) the `main` or `settings` window.
///
/// `async` on purpose: creating a window from a synchronous command can deadlock on Windows.
/// The floating window is only allowed to open `main`; this is checked here in addition to the
/// capability files because the capability system cannot restrict arguments.
#[tauri::command]
async fn open_window(
    caller: WebviewWindow,
    app: AppHandle,
    label: String,
) -> Result<(), CommandError> {
    if caller.label() == LABEL_FLOATING && label != LABEL_MAIN {
        return Err(CommandError::new(
            "permission_denied",
            "the floating window may only open the main panel",
        ));
    }
    match label.as_str() {
        LABEL_MAIN => {
            show_main(&app);
            Ok(())
        }
        LABEL_SETTINGS => open_settings(&app).map_err(|e| {
            CommandError::new("external_failure", format!("could not open settings: {e}"))
        }),
        other => Err(CommandError::new(
            "not_found",
            format!("unknown window label: {other}"),
        )),
    }
}

/// Shows, un-minimizes and focuses the main panel (it is only ever hidden, never destroyed).
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL_MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The settings window is created on demand and destroyed when closed.
fn open_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL_SETTINGS) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(app, LABEL_SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("LumaNote 設定")
        .inner_size(760.0, 760.0)
        .min_inner_size(520.0, 480.0)
        .center()
        .build()?;
    Ok(())
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id("open", "開啟").build(app)?;
    let pause = CheckMenuItemBuilder::with_id("pause_observation", "暫停觀察")
        .checked(false)
        .build(app)?;
    let dnd = CheckMenuItemBuilder::with_id("do_not_disturb", "勿擾")
        .checked(false)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "結束").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&open)
        .separator()
        .item(&pause)
        .item(&dnd)
        .separator()
        .item(&quit)
        .build()?;

    let mut tray = TrayIconBuilder::with_id("luma-tray")
        .tooltip("LumaNote（左鍵開啟，右鍵選單）")
        .menu(&menu)
        // Left click opens the main panel; right click shows the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().0.as_str() {
            "open" => show_main(app),
            "pause_observation" => {
                // Flip our own flag and write it back, so the check mark and the flag
                // cannot drift apart whatever the native menu does on click.
                let now = !app
                    .state::<ShellState>()
                    .observation_paused
                    .fetch_xor(true, Ordering::SeqCst);
                let _ = pause.set_checked(now);
            }
            "do_not_disturb" => {
                let now = !app
                    .state::<ShellState>()
                    .do_not_disturb
                    .fetch_xor(true, Ordering::SeqCst);
                let _ = dnd.set_checked(now);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Reveals the floating window without activating it. Safe to call repeatedly.
fn reveal_floating(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL_FLOATING) {
        if !window.is_visible().unwrap_or(false) {
            let _ = window.show();
        }
    }
}

/// Entry point called from `main.rs`.
pub fn run() {
    let app = tauri::Builder::default()
        // Must be registered first (plugin docs). A second launch lands here in the first
        // process and just brings the main panel to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }))
        .manage(ShellState::default())
        .manage(floating::FloatingState::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            open_window,
            floating::set_floating_expanded
        ])
        // The floating window is created hidden (`visible: false`) and revealed once its page
        // has loaded, which avoids a flash on a transparent window.
        .on_page_load(|webview, payload| {
            if webview.label() == LABEL_FLOATING
                && matches!(payload.event(), PageLoadEvent::Finished)
            {
                reveal_floating(webview.app_handle());
            }
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => match window.label() {
                // Closing the main panel only hides it; the app keeps running in the tray.
                LABEL_MAIN => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                // The sprite is not user-closable (no decorations, not focusable, but a
                // programmatic or shell-level close must not remove the app's last handle).
                LABEL_FLOATING => api.prevent_close(),
                // `settings` (and anything else): default behaviour, the window is destroyed
                // and nothing else is affected.
                _ => {}
            },
            WindowEvent::Moved(_) if window.label() == LABEL_FLOATING => {
                floating::note_moved(window.app_handle());
            }
            _ => {}
        })
        .setup(|app| {
            build_tray(app)?;
            floating::init(app.handle());
            // If the page never reports "finished" (dev server down, load error), still show
            // the sprite so the user is not left with an invisible app.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(FLOATING_SHOW_FALLBACK);
                reveal_floating(&handle);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Tauri application");

    app.run(|_app, event| {
        // `code: None` means the last window went away (not a request from us); keep running
        // in the tray. The tray "結束" item calls `app.exit(0)` (code = Some(0)) and is not
        // blocked.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
    });
}
