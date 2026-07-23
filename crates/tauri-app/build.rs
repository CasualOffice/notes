//! Tauri build script: bakes `tauri.conf.json`, capability files, and codegen
//! context into the binary (HLD §4). See Architecture §12 (packaging).

fn main() {
    tauri_build::build();
}
