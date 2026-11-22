/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `rust` makes the app talk to the real core instead of the demo mock (`src/core/index.ts`). */
  readonly VITE_CORE?: string;
}
