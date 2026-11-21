import { isTauri } from "@tauri-apps/api/core";

/** True in the iOS and Android apps, false on desktop and in the browser preview. */
const inPhoneApp = isTauri() && /android|iphone|ipad|ipod/i.test(navigator.userAgent);

/** Scanning QR codes uses the camera plugin, which the iOS and Android apps have. */
export const canScanQrCodes = inPhoneApp;

/**
 * The system share sheet through the web share API. Not in the phone apps: on iOS the web view
 * opens it as a clipped popover, and Android's web view lacks the API. A native share sheet
 * replaces it there (#66).
 */
export const canShare = !inPhoneApp && typeof navigator.share === "function";
