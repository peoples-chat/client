/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `mock` makes the app show the demo mock instead of the real core (`src/core/index.ts`). */
  readonly VITE_CORE?: string;
}
