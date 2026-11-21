fn main() {
    // `generate_context!` embeds the app icon (on macOS dev builds, icons/icon.icns) when this
    // crate compiles, but tauri-build does not rebuild the crate when only the icons change.
    // Watching the folder here does, so new icons show up in `tauri dev`.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
