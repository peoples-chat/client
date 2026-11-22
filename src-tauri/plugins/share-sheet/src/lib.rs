//! The system share sheet on phones (#66): `UIActivityViewController` on iOS, and an
//! `ACTION_SEND` chooser on Android. The web view calls `plugin:share-sheet|share` with
//! `{ text }`, and the call resolves once the sheet is up.
//!
//! Desktop has no part in it: the web share API covers desktops, and the phones' web views can't
//! use it (`src/platform.ts`).

#![cfg(mobile)]

use tauri::Runtime;
use tauri::plugin::{Builder, TauriPlugin};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "chat.peoples.sharesheet";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_share_sheet);

/// The plugin, for `tauri::Builder::plugin`.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("share-sheet")
        .setup(|_app, api| {
            #[cfg(target_os = "android")]
            api.register_android_plugin(PLUGIN_IDENTIFIER, "ShareSheetPlugin")?;
            #[cfg(target_os = "ios")]
            api.register_ios_plugin(init_plugin_share_sheet)?;
            Ok(())
        })
        .build()
}
