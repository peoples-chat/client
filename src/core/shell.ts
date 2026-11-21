import { invoke, isTauri } from "@tauri-apps/api/core";

/**
 * Version of the Rust core linked into the app shell, or null when the interface runs outside
 * the app, as in the browser preview. Shows that the web view, shell and core are wired up.
 */
export async function coreVersion(): Promise<string | null> {
  if (!isTauri()) return null;
  return invoke<string>("core_version");
}
