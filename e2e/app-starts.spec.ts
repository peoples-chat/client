// The app builds, starts and shows its first screen. The harness's own check, not an issue's.
import { $, browser, expect } from "@wdio/globals";

describe("desktop app", () => {
  it("starts on the welcome screen", async () => {
    await expect($('[data-testid="welcome-screen"]')).toBeDisplayed();
    await expect($('[data-testid="welcome-title"]')).toHaveText("peoples.chat");
    await expect($('[data-testid="welcome-create-account"]')).toBeEnabled();
    await browser.saveScreenshot("evidence/e2e-app-starts.png");
  });
});
