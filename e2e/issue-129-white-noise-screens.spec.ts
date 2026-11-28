// Issue #129: screens that match what White Noise users expect. Runs against the real core.
import { mkdirSync } from "node:fs";
import { $, browser, expect } from "@wdio/globals";

const EVIDENCE = "evidence/129";

type E2eWindow = { peoplesChatE2eScan?: (text: string) => void };

/**
 * BIP-340's first test vector, secret key 3. Someone has published a public profile for it (named
 * "test" when this was written), and the scan preview reads that name from public relays, so the
 * test only asks for some name (`/\S/`) rather than a fixed one.
 */
const OTHER_KEY_HEX = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";

describe("issue #129: screens White Noise users expect", () => {
  before(async () => {
    mkdirSync(EVIDENCE, { recursive: true });
    await $('[data-testid="welcome-create-account"]').click();
    await $('[data-testid="create-profile-name"]').setValue("Emil Dahl");
    await $('[data-testid="create-profile-continue"]').click();
    await expect($('[data-testid="tab-settings"]')).toBeDisplayed();
  });

  it("shows the npub on Profile, with Copy", async () => {
    await $('[data-testid="tab-settings"]').click();
    await $('[data-testid="settings-profile"]').click();
    const npub = $('[data-testid="profile-npub"]');
    await expect(npub).toBeDisplayed();
    await expect(npub).toHaveText(/^npub1[02-9ac-hj-np-z]{58}$/);
    await browser.saveScreenshot(`${EVIDENCE}/profile-npub.png`);

    await $('[data-testid="profile-npub-copy"]').click();
    // WebKit's driver refuses to read the system clipboard; the copied text has a Vitest test
    // (src/screens/issue-129-profile-npub.test.ts), so here the app's confirmation is enough.
    await expect($('[data-testid="profile-npub-copy"]')).toHaveText("Copied");
    await $('[data-testid="header-leading"]').click();
  });

  it("drops Read receipts and Typing indicators from Privacy", async () => {
    await $('[data-testid="settings-privacy"]').click();
    const privacy = $('[data-testid="privacy-screen"]');
    await expect(privacy).toBeDisplayed();
    const text = await privacy.getText();
    expect(text).not.toContain("Read receipts");
    expect(text).not.toContain("Typing indicators");
    expect(text).toContain("Relays and servers");
    await browser.saveScreenshot(`${EVIDENCE}/privacy.png`);
    await $('[data-testid="header-leading"]').click();
  });

  it("says what deleting the account deletes, and what stays on the relays", async () => {
    await $('[data-testid="settings-account"]').click();
    await expect($('[data-testid="account-delete"]')).toHaveText(expect.stringContaining("Deletes your key and chats from this phone"));
    await $('[data-testid="account-delete"]').click();
    const text = $('[data-testid="delete-account-text"]');
    await expect(text).toHaveText(expect.stringContaining("asks the relays to delete your invitation key"));
    await expect(text).toHaveText(expect.stringContaining("follow list and relay lists stay on the relays"));
    await expect(text).not.toHaveText(expect.stringContaining("Your profile is removed"));
    await browser.saveScreenshot(`${EVIDENCE}/delete-account.png`);
    await $('[data-testid="account-delete-cancel"]').click();
  });

  it("says White Noise needs the plain key on Export", async () => {
    await $('[data-testid="account-export-key"]').click();
    await expect($('[data-testid="export-kind-protected"]')).toHaveText(expect.stringContaining("White Noise needs the plain key"));
    await expect($('[data-testid="export-kind-protected"]')).not.toHaveText(expect.stringContaining("Opens in other Nostr apps"));
    await expect($('[data-testid="export-kind-plain"]')).toHaveText(expect.stringContaining("White Noise signs in with this"));
    await browser.saveScreenshot(`${EVIDENCE}/export-key.png`);
    for (let i = 0; i < 3; i++) await $('[data-testid="header-leading"]').click();
  });

  it("shows who a scanned code is before starting a chat", async () => {
    await $('[data-testid="chats-new-chat"]').click();
    await $('[data-testid="new-chat-scan"]').click();
    await browser.waitUntil(() =>
      browser.execute(() => typeof (window as unknown as E2eWindow).peoplesChatE2eScan === "function"),
    );
    // A White Noise profile code. The encoder matches NIP-19's example first.
    expect(npub("3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d")).toBe(
      "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6",
    );
    const code = `marmot://profile/${npub(OTHER_KEY_HEX)}`;
    await browser.execute((text) => (window as unknown as E2eWindow).peoplesChatE2eScan?.(text), code);

    await expect($('[data-testid="scan-found"]')).toBeDisplayed({ wait: 30_000 });
    await expect($('[data-testid="scan-found-name"]')).toHaveText(/\S/);
    await expect($('[data-testid="scan-start-chat"]')).toBeEnabled();
    await browser.saveScreenshot(`${EVIDENCE}/scan-found.png`);

    // Nothing started: closing leaves New chat without contacts.
    await $('[data-testid="header-leading"]').click();
    await expect($('[data-testid="new-chat-scan"]')).toBeDisplayed();
    await expect($('[data-testid="new-chat-contacts"]')).not.toBeExisting();
    await $('[data-testid="header-leading"]').click();
  });

  // The data folder is shared by every spec in a run: leave it without an account, as found.
  after(async () => {
    await $('[data-testid="tab-settings"]').click();
    await $('[data-testid="settings-account"]').click();
    await $('[data-testid="account-delete"]').click();
    await $('[data-testid="account-delete-confirm"]').click();
    await expect($('[data-testid="welcome-screen"]')).toBeDisplayed({ wait: 30_000 });
  });
});

/** NIP-19 `npub` of a hex public key: bech32 (BIP-173) with the `npub` prefix. */
function npub(hex: string): string {
  const CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
  const bytes = hex.match(/../g)!.map((b) => parseInt(b, 16));
  const data: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const byte of bytes) {
    acc = (acc << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      data.push((acc >> bits) & 31);
    }
  }
  if (bits > 0) data.push((acc << (5 - bits)) & 31);
  const hrp = "npub";
  const polymod = (values: number[]) => {
    const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    let chk = 1;
    for (const v of values) {
      const top = chk >> 25;
      chk = ((chk & 0x1ffffff) << 5) ^ v;
      for (let i = 0; i < 5; i++) if ((top >> i) & 1) chk ^= GEN[i];
    }
    return chk;
  };
  const expanded = [...[...hrp].map((c) => c.charCodeAt(0) >> 5), 0, ...[...hrp].map((c) => c.charCodeAt(0) & 31)];
  const mod = polymod([...expanded, ...data, 0, 0, 0, 0, 0, 0]) ^ 1;
  const checksum = Array.from({ length: 6 }, (_, i) => (mod >> (5 * (5 - i))) & 31);
  return `${hrp}1${[...data, ...checksum].map((d) => CHARSET[d]).join("")}`;
}
