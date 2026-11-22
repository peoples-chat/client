import { invoke, isTauri } from "@tauri-apps/api/core";

/** True in the iOS and Android apps, false on desktop and in the browser preview. */
const inPhoneApp = isTauri() && /android|iphone|ipad|ipod/i.test(navigator.userAgent);

/** Scanning QR codes uses the camera plugin, which the iOS and Android apps have. */
export const canScanQrCodes = inPhoneApp;

/**
 * Whether the system share sheet can open. The phone apps open it through the app's share-sheet
 * plugin (#66): on iOS the web view opens the web share API's sheet as a clipped popover, and
 * Android's web view lacks the API. Elsewhere the web share API opens it, where there is one.
 */
export const canShare = inPhoneApp || typeof navigator.share === "function";

/** Opens the system share sheet with `text`. Resolves once the sheet is up, or closed. */
export async function share(text: string, title: string): Promise<void> {
  if (inPhoneApp) {
    await invoke("plugin:share-sheet|share", { text });
  } else {
    await navigator.share({ title, text });
  }
}
