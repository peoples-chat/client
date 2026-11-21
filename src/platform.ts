import { isTauri } from "@tauri-apps/api/core";

/** Scanning QR codes uses the camera plugin, which the iOS and Android apps have. */
export const canScanQrCodes = isTauri() && /android|iphone|ipad|ipod/i.test(navigator.userAgent);

/** The system share sheet, where the web view offers it. */
export const canShare = typeof navigator.share === "function";
