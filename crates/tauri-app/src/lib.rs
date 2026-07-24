//! # tauri-app
//!
//! The Tauri host process: the **only WebView↔Core door** (HLD §4/§6). Owns the
//! command router (the public `#[tauri::command]` surface in [`commands`]), the
//! `AppEvent` event bus (HLD §7), window/activation setup, and the per-window
//! deny-by-default capability files (Architecture §9/§12).
//!
//! Security invariants (CLAUDE.md): capabilities are deny-by-default per window; a
//! strict CSP with no remote content; the WebView never sees SQL, raw FS paths, or
//! raw PCM. `tauri-plugin-sql` is intentionally **not** used — all DB access is
//! Rust-side via `storage` (direct rusqlite + SQLCipher), composed by `app-service`.
//!
//! ## Boot sequence (`setup`)
//! 1. Resolve the app-data root and install an [`EventSink`] that emits every
//!    `SequencedEvent` to the WebView as the single `"app-event"` channel.
//! 2. [`app_service::Service::open`] provisions the SQLCipher master key (OS
//!    keystore, dev-file fallback), opens the encrypted store, and builds the
//!    service — store custody lives in `app-service`, not here.
//! 3. Replay the op-journal (crash recovery) and register the command surface.
//! 4. Build the system tray and register the global quick-capture hotkey
//!    (HLD §8.2). The frameless `quick-capture` window is declared hidden in
//!    `tauri.conf.json`; the tray and the hotkey toggle its visibility.

mod commands;
mod session;

use std::sync::Arc;

use app_service::{EventSink, Service};
use session::SessionManager;
use storage::Paths;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Window label of the frameless global quick-capture surface (HLD §8.2).
const QUICK_CAPTURE_LABEL: &str = "quick-capture";
/// Window label of the primary application window.
const MAIN_LABEL: &str = "main";

/// The global quick-capture hotkey: `CmdOrCtrl+Shift+Space` (⌘⇧Space on macOS,
/// Ctrl+Shift+Space elsewhere — HLD §8.2).
fn quick_capture_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let primary = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let primary = Modifiers::CONTROL;
    Shortcut::new(Some(primary | Modifiers::SHIFT), Code::Space)
}

/// Show + focus a window by label (creating nothing — the window is declared in
/// config). No-op if the label is unknown.
fn show_window(app: &tauri::AppHandle, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Toggle the frameless quick-capture window: hide it if already visible+focused,
/// otherwise center, show, and focus it (HLD §8.2 — "toggle capture window").
fn toggle_quick_capture(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window(QUICK_CAPTURE_LABEL) else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        let _ = win.center();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Build the tray icon + menu (Open Casual Note / Quick Capture / Quit) and wire
/// its actions (HLD §4 — window/tray). The icon reuses the app's default window
/// icon so no extra asset is needed.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "tray.open", "Open Casual Note", true, None::<&str>)?;
    let capture_item = MenuItem::with_id(app, "tray.capture", "Quick Capture", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "tray.quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &capture_item, &sep, &quit_item])?;

    let mut builder = TrayIconBuilder::new()
        .tooltip("Casual Note")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray.open" => show_window(app, MAIN_LABEL),
            "tray.capture" => toggle_quick_capture(app),
            "tray.quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Build and run the Casual Note desktop application (HLD §4 deployment view).
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        // The global quick-capture hotkey is driven entirely Rust-side; the handler
        // toggles the frameless capture window (HLD §8.2). The WebView never invokes
        // the plugin, so no per-window ACL grant is required.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed
                        && shortcut == &quick_capture_shortcut()
                    {
                        toggle_quick_capture(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            // macOS: a regular dock app (activation policy). No-op elsewhere.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            let data_dir = app.path().app_data_dir()?;
            let paths = Paths::new(data_dir);

            // The single event bus: every derived-fact event → the WebView (HLD §7).
            let handle = app.handle().clone();
            let sink: EventSink = Box::new(move |ev| {
                if let Err(e) = handle.emit("app-event", ev) {
                    tracing::warn!(error = %e, "failed to emit app-event");
                }
            });

            // Open the encrypted store (key from the OS keystore, dev-file
            // fallback) and build the service — key custody lives in app-service.
            // A failure here is fatal: without the store there is no app, so log a
            // clear diagnostic and abort setup rather than run a half-open shell.
            let service = match Service::open(paths, "local", sink) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "failed to open the encrypted store; aborting boot");
                    return Err(Box::new(e) as Box<dyn std::error::Error>);
                }
            };
            match service.recover() {
                Ok(n) if n > 0 => tracing::info!(reapplied = n, "recovered ops from journal"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "journal recovery failed"),
            }
            let service = Arc::new(service);
            app.manage(service.clone());

            // The M2 meeting-intelligence runner: the mock-engine session coordinator
            // (mock capture + speech + deterministic-fallback LLM) plus the host-side
            // discrete-control registry driving it (HLD §8.4). Sharing the one
            // `Service` keeps every meeting mutation on the single op-log writer. A
            // runtime-build failure here is fatal like the store: without it the
            // meeting command surface cannot function.
            let manager = match SessionManager::new_with_mocks(service.clone()) {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "failed to build the meeting session manager");
                    return Err(Box::new(e) as Box<dyn std::error::Error>);
                }
            };
            app.manage(Arc::new(manager));

            // System tray: Open / Quick Capture / Quit (HLD §4).
            build_tray(app)?;

            // Register the global quick-capture hotkey (HLD §8.2). A failure (e.g.
            // the combo is already claimed OS-wide) is non-fatal — the tray still
            // toggles the capture window.
            if let Err(e) = app.global_shortcut().register(quick_capture_shortcut()) {
                tracing::warn!(error = %e, "failed to register the quick-capture global shortcut");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::notes_create,
            commands::notes_get,
            commands::notes_save,
            commands::notes_list,
            commands::notes_delete,
            commands::notes_resolve_links,
            commands::notes_move,
            commands::notes_export_markdown,
            commands::notes_import_markdown,
            commands::notebooks_list,
            commands::notebooks_create,
            commands::daily_get_or_create,
            commands::links_backlinks,
            commands::links_unlinked_mentions,
            commands::blocks_get,
            commands::blocks_backlinks,
            commands::tasks_create,
            commands::tasks_update,
            commands::tasks_complete,
            commands::tasks_reorder,
            commands::tasks_bucket,
            commands::projects_create,
            commands::areas_create,
            commands::reminders_create,
            commands::reminders_snooze,
            commands::reminders_cancel,
            commands::reminders_upcoming,
            commands::capture_quick,
            commands::nlp_parse,
            commands::get_capabilities,
            commands::search_query,
            commands::palette_run,
            // --- Meeting intelligence (M2) session surface (HLD §6 `meeting.*`) ---
            session::list_capture_apps,
            session::run_preflight,
            session::start_session,
            session::pause_session,
            session::resume_session,
            session::stop_session,
            session::cancel_job,
            session::regenerate_artifact,
            session::get_session,
            session::list_action_items,
            session::action_item_to_task,
            commands::ai_ask,
            commands::ai_suggestions_list,
            commands::models_list,
            commands::models_install,
            commands::export_note,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Casual Note application");
}
