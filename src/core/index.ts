import { isTauri } from "@tauri-apps/api/core";
import { createMockCore } from "./mockCore";
import { createTauriCore } from "./tauriCore";
import type { Core } from "./types";

/**
 * True when the interface talks to the real Rust core in the app shell. Until the real core
 * covers the whole flow (#20), builds opt in with `VITE_CORE=rust`; the browser preview always
 * uses the mock.
 */
const usesRustCore = isTauri() && import.meta.env.VITE_CORE === "rust";

/** The core the interface talks to: the real one, or the mock with demo data. */
export const core: Core = usesRustCore ? createTauriCore() : createMockCore();

/** True while `core` is the mock, so the interface can say that it shows demo data. */
export const usesDemoData = !usesRustCore;

export * from "./types";
