import { isTauri } from "@tauri-apps/api/core";
import { createMockCore } from "./mockCore";
import { createTauriCore } from "./tauriCore";
import type { Core } from "./types";

/**
 * True when the interface talks to the real Rust core in the app shell. The app does since the
 * real core covers the whole flow (#20), unless it is started with `VITE_CORE=mock`, which shows
 * demo data. The browser preview always uses the mock.
 */
const usesRustCore = isTauri() && import.meta.env.VITE_CORE !== "mock";

/** The core the interface talks to: the real one, or the mock with demo data. */
export const core: Core = usesRustCore ? createTauriCore() : createMockCore();

/** True while `core` is the mock, so the interface can say that it shows demo data. */
export const usesDemoData = !usesRustCore;

export * from "./types";
