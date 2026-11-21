import { createMockCore } from "./mockCore";
import type { Core } from "./types";

/**
 * The core the interface talks to. The mock with demo data for now; the real Rust core
 * replaces it behind the same interface once the Tauri boundary exists (#16).
 */
export const core: Core = createMockCore();

/** True while `core` is the mock, so the interface can say that it shows demo data. */
export const usesDemoData = true;

export * from "./types";
