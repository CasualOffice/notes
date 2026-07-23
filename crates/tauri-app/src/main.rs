//! Casual Note desktop binary entry point. Delegates to [`tauri_app::run`].
//! See HLD §4 (deployment view) and Architecture §12 (packaging).

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri_app::run();
}
