import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";

/** True in the iOS and Android apps, false on desktop and in the browser preview. */
const inPhoneApp = isTauri() && /android|iphone|ipad|ipod/i.test(navigator.userAgent);

/**
 * Scanning QR codes uses the camera plugin, which the iOS and Android apps have. Desktop e2e
 * builds scan too: their tests hand in the scanned text (`src/screens/Scan.tsx`).
 */
export const canScanQrCodes = inPhoneApp || import.meta.env.VITE_E2E === "1";

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

/** The longest side of a photo the interface hands the core (#94). The core cuts it square and shrinks it further. */
const PHOTO_MAX_SIDE = 1024;

/** A photo the interface can't read. */
export class UnreadablePhoto extends Error {}

/**
 * A chosen photo as JPEG bytes, at most 1024 pixels on its longest side (#94). Drawing it turns
 * it upright, and turns formats the core can't read, such as an iPhone's HEIC, into JPEG. The
 * core saves it again anyway, which is what drops the metadata.
 */
export async function photoBytes(file: Blob): Promise<Uint8Array> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new UnreadablePhoto();
  }
  const scale = Math.min(1, PHOTO_MAX_SIDE / Math.max(bitmap.width, bitmap.height));
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  canvas.getContext("2d")?.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close();
  const jpeg = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.9));
  if (!jpeg) throw new UnreadablePhoto();
  return new Uint8Array(await jpeg.arrayBuffer());
}

/** JPEG bytes as a `data:` URL, which the web view's policy allows for images. */
export function jpegDataUrl(jpeg: Uint8Array): string {
  let binary = "";
  for (const byte of jpeg) binary += String.fromCharCode(byte);
  return `data:image/jpeg;base64,${btoa(binary)}`;
}

/**
 * Calls `onLink` with each link that opened the app (#72): the one it started with, and each one
 * tapped while it runs. Only the phone apps open links.
 */
export async function followOpenedLinks(onLink: (url: string) => void): Promise<() => void> {
  if (!inPhoneApp) return () => {};
  const started = await getCurrent().catch(() => null);
  for (const url of started ?? []) onLink(url);
  return onOpenUrl((urls) => urls.forEach(onLink));
}
