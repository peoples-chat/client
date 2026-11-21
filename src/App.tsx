import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type CoreLink =
  | { state: "starting" }
  | { state: "linked"; version: string }
  | { state: "failed"; reason: string };

// Placeholder screen. It asks the Rust core for its version to prove that the
// web view, the app shell and the core are wired together on this platform.
function App() {
  const [core, setCore] = useState<CoreLink>({ state: "starting" });

  useEffect(() => {
    let cancelled = false;
    invoke<string>("core_version")
      .then((version) => {
        if (!cancelled) setCore({ state: "linked", version });
      })
      .catch((e) => {
        if (!cancelled) setCore({ state: "failed", reason: String(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <main>
      <h1>peoples.chat</h1>
      <p className="tagline">Messaging that keeps working when the internet is gone.</p>

      {core.state === "linked" && <p className="status ok">Core {core.version} linked</p>}
      {core.state === "failed" && <p className="status failed">Core not reachable: {core.reason}</p>}
      {core.state === "starting" && <p className="status">Starting…</p>}
    </main>
  );
}

export default App;
