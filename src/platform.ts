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

/**
 * Saves `text` as a file named `fileName`. The phone apps go through the share sheet, where
 * Save to Files (iOS) and Drive (Android) make the file. Elsewhere the web share API shares the
 * file where it can, and the browser downloads it otherwise.
 */
export async function saveAsFile(text: string, fileName: string): Promise<void> {
  if (inPhoneApp) {
    await invoke("plugin:share-sheet|share", { text });
    return;
  }
  const file = new File([text], fileName, { type: "text/plain" });
  if (typeof navigator.canShare === "function" && navigator.canShare({ files: [file] })) {
    await navigator.share({ files: [file] });
    return;
  }
  const url = URL.createObjectURL(file);
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  link.click();
  URL.revokeObjectURL(url);
}

/** Opens the system share sheet with `text`. Resolves once the sheet is up, or closed. */
export async function share(text: string, title: string): Promise<void> {
  if (inPhoneApp) {
    await invoke("plugin:share-sheet|share", { text });
  } else {
    await navigator.share({ title, text });
  }
}
