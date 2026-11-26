// Desktop end-to-end tests: WebdriverIO drives the real macOS app through the WebDriver server
// that `--features e2e` builds into it (macOS has no WKWebView driver of its own).
// Run with `pnpm e2e`, which builds the app first. Specs: e2e/*.spec.ts, named after their issue.
import { mkdtempSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const app = join(root, "target", "debug", "peoples-chat");

// Every run starts without an account and never touches the app's real data (see bridge.rs).
process.env.PEOPLES_CHAT_E2E_DATA_DIR = mkdtempSync(join(tmpdir(), "peoples-chat-e2e-"));
mkdirSync(join(root, "evidence"), { recursive: true });

export const config: WebdriverIO.Config = {
  runner: "local",
  specs: [join(root, "e2e", "**", "*.spec.ts")],
  maxInstances: 1,
  services: [["@wdio/tauri-service", { appBinaryPath: app, driverProvider: "embedded" }]],
  capabilities: [{ browserName: "tauri", "tauri:options": { application: app } } as WebdriverIO.Capabilities],
  framework: "mocha",
  reporters: ["spec"],
  logLevel: "warn",
  waitforTimeout: 15_000,
  mochaOpts: { ui: "bdd", timeout: 120_000 },
};
