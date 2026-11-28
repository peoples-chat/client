/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `mock` makes the app show the demo mock instead of the real core (`src/core/index.ts`). */
  readonly VITE_CORE?: string;
  /** `1` in desktop e2e builds (`pnpm e2e`): WebdriverIO's helper, and a scan a test hands in. */
  readonly VITE_E2E?: string;
}
