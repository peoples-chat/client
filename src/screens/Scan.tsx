import { useEffect, useState } from "react";
import {
  Format,
  cancel,
  checkPermissions,
  openAppSettings,
  requestPermissions,
  scan,
} from "@tauri-apps/plugin-barcode-scanner";
import { core, type Contact } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";
import { firstName, tintFor } from "../ui/people";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./Scan.module.css";

type Step =
  | { name: "starting" }
  | { name: "scanning" }
  | { name: "denied" }
  | { name: "lookingUp" }
  | { name: "found"; link: string; contact: Contact }
  | { name: "adding" }
  | { name: "failed"; reason: string };

const MESSAGES: Record<Exclude<Step["name"], "failed" | "found">, string> = {
  starting: "Starting the camera…",
  scanning: "Point the camera at the other person's code. They find it on their Profile.",
  denied: "peoples.chat needs the camera to scan codes. Allow it in the phone's settings.",
  lookingUp: "Looking them up…",
  adding: "Starting the chat…",
};

/**
 * Desktop e2e builds only: there is no camera, so a test hands the scanned text in through
 * `window.peoplesChatE2eScan(text)`. In every other build this is false at build time.
 */
const fakeCamera = import.meta.env.VITE_E2E === "1";

declare global {
  interface Window {
    peoplesChatE2eScan?: (text: string) => void;
  }
}

/**
 * Scans a contact's QR code and shows who it is, from their profile (#129). The chat starts, and
 * the invitation goes, only when the user taps Start, as White Noise opens the person's profile
 * first. The design has no scan screen, so this is a minimal one: the camera shows through the
 * web view, with a frame, a hint and Close.
 */
export function Scan() {
  const navigation = useNavigation();
  const [step, setStep] = useState<Step>({ name: "starting" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    const root = document.documentElement;

    // Deferred, so that React's development double mount starts only one scan.
    const timer = setTimeout(async () => {
      try {
        if (!fakeCamera && !(await cameraAllowed())) {
          if (active) setStep({ name: "denied" });
          return;
        }
        if (!active) return;
        setStep({ name: "scanning" });
        root.dataset.scanning = "";
        const link = fakeCamera ? await scannedByTest() : (await scan({ windowed: true, formats: [Format.QRCode] })).content;
        delete root.dataset.scanning;
        if (!active) return;

        setStep({ name: "lookingUp" });
        const contact = await core.previewInvite(link);
        if (active) setStep({ name: "found", link, contact });
      } catch (error) {
        delete root.dataset.scanning;
        if (active) setStep({ name: "failed", reason: errorText(error) });
      }
    }, 0);

    return () => {
      active = false;
      clearTimeout(timer);
      delete root.dataset.scanning;
      if (fakeCamera) delete window.peoplesChatE2eScan;
      else cancel().catch(() => {});
    };
  }, [attempt, navigation]);

  function scanAgain() {
    setStep({ name: "starting" });
    setAttempt((n) => n + 1);
  }

  async function start(link: string) {
    setStep({ name: "adding" });
    try {
      const chat = await core.addContact(link);
      navigation.reset([{ name: "chats" }, { name: "chat", chatId: chat.id }]);
    } catch (error) {
      setStep({ name: "failed", reason: errorText(error) });
    }
  }

  const live = step.name === "scanning";
  return (
    <div className={`${layout.screen} ${live ? styles.live : ""}`} data-testid="scan-screen">
      <ScreenHeader title="Scan a QR code" leading="close" onLeading={navigation.back} />
      {step.name === "found" ? (
        <Found contact={step.contact} onStart={() => void start(step.link)} onScanAgain={scanAgain} />
      ) : (
        <main className={styles.main}>
          {live && <div className={styles.frame} aria-hidden="true" />}
          <p className={styles.message} role="status" data-testid="scan-status">
            {step.name === "failed" ? step.reason : MESSAGES[step.name]}
          </p>
          {step.name === "denied" && <Button onClick={() => void openAppSettings()}>Open settings</Button>}
          {step.name === "failed" && (
            <Button onClick={scanAgain} data-testid="scan-again">
              Scan again
            </Button>
          )}
        </main>
      )}
    </div>
  );
}

/** Who the scanned code belongs to, and the tap that starts the chat. */
function Found({ contact, onStart, onScanAgain }: { contact: Contact; onStart: () => void; onScanAgain: () => void }) {
  const first = firstName(contact.name);
  return (
    <>
      <main className={styles.found} data-testid="scan-found">
        <Avatar name={contact.name} size={88} tint={tintFor(contact.name)} photo={contact.photo} />
        <h2 className={styles.foundName} data-testid="scan-found-name">
          {contact.name}
        </h2>
        <p className={styles.message}>Starting a chat sends {first} an invitation.</p>
      </main>
      <footer className={styles.footer}>
        <Button onClick={onStart} data-testid="scan-start-chat">
          Start a chat with {first}
        </Button>
        <Button variant="secondary" onClick={onScanAgain} data-testid="scan-again">
          Scan again
        </Button>
      </footer>
    </>
  );
}

/** The text a desktop e2e test hands in instead of a camera (`fakeCamera`). */
function scannedByTest(): Promise<string> {
  return new Promise((resolve) => {
    window.peoplesChatE2eScan = resolve;
  });
}

async function cameraAllowed(): Promise<boolean> {
  const permission = await checkPermissions();
  if (permission === "granted") return true;
  if (permission === "denied") return false;
  // Not asked yet, or Android wants the request explained after an earlier refusal.
  return (await requestPermissions()) === "granted";
}
