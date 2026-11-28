// #90: the app uses the project's relay, wss://relay.peoples.chat, and shows it as run by
// peoples.chat. A new account on the default relays opens Settings → Privacy → Relays and servers.
// The account is throwaway: the spec deletes it at the end, so it doesn't stay on public relays.
import { mkdirSync } from "node:fs";
import { $, $$, browser, expect } from "@wdio/globals";

mkdirSync("evidence/90", { recursive: true });

const testId = (id: string) => `[data-testid="${id}"]`;
const PROJECT_RELAY = testId("relay-relay.peoples.chat");
const PROJECT_RELAY_LABEL = testId("relay-relay.peoples.chat-label");
const PROJECT_RELAY_DESCRIPTION = testId("relay-relay.peoples.chat-description");
const OTHER_DEFAULTS = ["relay.damus.io", "nos.lol", "relay.primal.net"];

/** One step back, as the browser's back button: the navigator follows history (Navigator.tsx). */
async function back() {
  await browser.execute(() => history.back());
}

describe("issue 90: the project's relay", () => {
  it("is among a new account's relays, first, as run by peoples.chat", async () => {
    await $('[data-testid="welcome-create-account"]').click();
    await $('[data-testid="create-profile-name"]').setValue("Relay Check 90");
    await $('[data-testid="create-profile-continue"]').click();

    // Publishing the account can take a while on public relays.
    await $('[data-testid="tab-settings"]').waitForDisplayed({ timeout: 90_000 });
    await $('[data-testid="tab-settings"]').click();
    await $('[data-testid="settings-privacy"]').click();
    await $('[data-testid="privacy-relays"]').click();
    await expect($('[data-testid="relays-screen"]')).toBeDisplayed();

    await $(PROJECT_RELAY).waitForDisplayed({ timeout: 30_000 });
    await expect($(PROJECT_RELAY_LABEL)).toHaveText("relay.peoples.chat");
    // Connected once MDK has opened the connection.
    await browser.waitUntil(
      async () => (await $(PROJECT_RELAY_DESCRIPTION).getText()) === "Run by peoples.chat. Connected",
      { timeout: 30_000, timeoutMsg: "relay.peoples.chat did not show as run by peoples.chat and connected" },
    );

    // The three other defaults are there too, each run independently.
    for (const other of OTHER_DEFAULTS) {
      await $(`[data-testid="relay-${other}"]`).waitForDisplayed({ timeout: 30_000 });
      await expect($(`[data-testid="relay-${other}-description"]`)).toHaveText(
        expect.stringContaining("Independent."),
      );
    }

    // The project's relay comes first. Rows come before their own label and description.
    const rows = await $$('[data-testid="relays-screen"] [data-testid^="relay-"]');
    expect(await rows[0].getAttribute("data-testid")).toBe("relay-relay.peoples.chat");

    await browser.saveScreenshot("evidence/90/relays-and-servers.png");
  });

  it("deletes the throwaway account again", async () => {
    await back(); // Privacy
    await back(); // Settings
    await $('[data-testid="settings-account"]').waitForDisplayed();
    await $('[data-testid="settings-account"]').click();
    await $('[data-testid="account-delete"]').click();
    await $('[data-testid="account-delete-confirm"]').click();
    await $('[data-testid="welcome-screen"]').waitForDisplayed({ timeout: 60_000 });
  });
});
