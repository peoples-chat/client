import { useEffect, useState } from "react";
import {
  Format,
  cancel,
  checkPermissions,
  openAppSettings,
  requestPermissions,
  scan,
} from "@tauri-apps/plugin-barcode-scanner";
import { core } from "../core";
import { useNavigation } from "../navigation/Navigator";
import { Button } from "../ui/Button";
import { ScreenHeader } from "../ui/ScreenHeader";
import layout from "../ui/layout.module.css";
import { errorText } from "./shared";
import styles from "./Scan.module.css";

type Step =
  | { name: "starting" }
  | { name: "scanning" }
  | { name: "denied" }
  | { name: "adding" }
  | { name: "failed"; reason: string };

const MESSAGES: Record<Exclude<Step["name"], "failed">, string> = {
  starting: "Starting the camera…",
  scanning: "Point the camera at the other person's code. They find it on their Profile.",
  denied: "peoples.chat needs the camera to scan codes. Allow it in the phone's settings.",
  adding: "Starting the chat…",
};

/**
 * Scans a contact's QR code and starts a chat from it. The design has no scan screen, so this
 * is a minimal one: the camera shows through the web view, with a frame, a hint and Close.
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
        if (!(await cameraAllowed())) {
          if (active) setStep({ name: "denied" });
          return;
        }
        if (!active) return;
        setStep({ name: "scanning" });
        root.dataset.scanning = "";
        const { content } = await scan({ windowed: true, formats: [Format.QRCode] });
        delete root.dataset.scanning;
        if (!active) return;

        setStep({ name: "adding" });
        const chat = await core.addContact(content);
        if (active) navigation.reset([{ name: "chats" }, { name: "chat", chatId: chat.id }]);
      } catch (error) {
        delete root.dataset.scanning;
        if (active) setStep({ name: "failed", reason: errorText(error) });
      }
    }, 0);

    return () => {
      active = false;
      clearTimeout(timer);
      delete root.dataset.scanning;
      cancel().catch(() => {});
    };
  }, [attempt, navigation]);

  const live = step.name === "scanning";
  return (
    <div className={`${layout.screen} ${live ? styles.live : ""}`}>
      <ScreenHeader title="Scan a QR code" leading="close" onLeading={navigation.back} />
      <main className={styles.main}>
        {live && <div className={styles.frame} aria-hidden="true" />}
        <p className={styles.message} role="status">
          {step.name === "failed" ? step.reason : MESSAGES[step.name]}
        </p>
        {step.name === "denied" && <Button onClick={() => void openAppSettings()}>Open settings</Button>}
        {step.name === "failed" && (
          <Button
            onClick={() => {
              setStep({ name: "starting" });
              setAttempt((n) => n + 1);
            }}
          >
            Scan again
          </Button>
        )}
      </main>
    </div>
  );
}

async function cameraAllowed(): Promise<boolean> {
  const permission = await checkPermissions();
  if (permission === "granted") return true;
  if (permission === "denied") return false;
  // Not asked yet, or Android wants the request explained after an earlier refusal.
  return (await requestPermissions()) === "granted";
}
