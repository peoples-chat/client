import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/albert-sans";
import "./styles/tokens.css";
import "./styles/global.css";
import App from "./App";
import { store } from "./state";
import { followVisualViewport } from "./viewport";

// Desktop e2e builds only (`pnpm e2e` sets VITE_E2E): WebdriverIO's helper inside the web view.
// In every other build the condition is false at build time, so the import is left out.
if (import.meta.env.VITE_E2E === "1") void import("@wdio/tauri-plugin");

followVisualViewport();
void store.start();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
