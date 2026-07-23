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
//! 1. Resolve the app-data root and open the encrypted [`storage::Store`] (key from
//!    the OS keystore, with a dev-file fallback for headless boxes).
//! 2. Build the [`app_service::Service`] over it, installing an [`EventSink`] that
//!    emits every `SequencedEvent` to the WebView as the single `"app-event"` channel.
//! 3. Replay the op-journal (crash recovery) and register the command surface.

mod commands;

use std::sync::Arc;

use app_service::{EventSink, Service};
use storage::{DevFileKeyStore, KeyMaterial, Paths, StorageResult, Store};
use tauri::{Emitter, Manager};

/// Build and run the Casual Note desktop application (HLD §4 deployment view).
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            // macOS: a regular dock app (activation policy). No-op elsewhere.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            let data_dir = app.path().app_data_dir()?;
            let paths = Paths::new(data_dir);
            let key = provision_key(&paths)?;
            let store = Store::open(paths, key)?;

            // The single event bus: every derived-fact event → the WebView (HLD §7).
            let handle = app.handle().clone();
            let sink: EventSink = Box::new(move |ev| {
                if let Err(e) = handle.emit("app-event", ev) {
                    tracing::warn!(error = %e, "failed to emit app-event");
                }
            });

            let service = Service::new(store, "local", sink);
            match service.recover() {
                Ok(n) if n > 0 => tracing::info!(reapplied = n, "recovered ops from journal"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "journal recovery failed"),
            }
            app.manage(Arc::new(service));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::notes_create,
            commands::notes_get,
            commands::notes_save,
            commands::notes_list,
            commands::notes_delete,
            commands::notes_resolve_links,
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
            commands::search_query,
            commands::palette_run,
            commands::meeting_preflight,
            commands::meeting_start,
            commands::meeting_stop,
            commands::meeting_artifact,
            commands::meeting_action_item_to_task,
            commands::ai_ask,
            commands::ai_suggestions_list,
            commands::models_list,
            commands::models_install,
            commands::export_note,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Casual Note application");
}

/// Provision the SQLCipher master key: OS keystore first (Keychain / Credential
/// Manager / Secret Service), a `0600` dev file beside the DB as a headless
/// fallback (Data Model §13.1 custody; the fallback warns loudly). `storage`'s
/// `os-keystore` feature is on by default, so [`storage::KeyringKeyStore`] exists.
fn provision_key(paths: &Paths) -> StorageResult<KeyMaterial> {
    match storage::keystore::provision_db_key(&storage::KeyringKeyStore::new()) {
        Ok(k) => Ok(k),
        Err(e) => {
            tracing::warn!(error = %e, "OS keystore unavailable; falling back to dev key file");
            let dev = DevFileKeyStore::new(paths.root().join(".dev-db-key"));
            storage::keystore::provision_db_key(&dev)
        }
    }
}
