/** Scanning QR codes needs the camera plugin, which is not added yet. */
export const canScanQrCodes = false;

/** The system share sheet, where the web view offers it. */
export const canShare = typeof navigator.share === "function";
