//! Checks that peoples.chat and White Noise can chat through one relay alone (#90). The other
//! side is White Noise's own command-line app, `wn` from MDK (`crates/cli`), at the MDK version the
//! core uses. Build it once into this repository's `target/`:
//!
//! ```sh
//! cargo install --locked --git https://github.com/marmot-protocol/mdk --tag v0.18.0 wn-cli --root target/wn
//! ```
//!
//! Then: `cargo run -p peoples-chat-core --example white_noise_check -- <relay> target/wn/bin/wn`.
//!
//! Both directions, each in a chat of its own:
//!
//! 1. peoples.chat adds the White Noise account; White Noise accepts, and each side sends once.
//! 2. White Noise starts a chat with the peoples.chat account; peoples.chat accepts the request,
//!    and each side sends once.
//!
//! Both accounts are throwaway: the core's key stays in memory, `wn`'s in a file in a temporary
//! home, and both are deleted at the end. Their profiles and relay lists stay on the relay.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use peoples_chat_core::interface::{CoreEvent, Direction};
use peoples_chat_core::{Core, MessagingOptions};
use serde_json::Value;

/// How long one side waits for the other.
const WAIT: Duration = Duration::from_secs(90);
/// How often `wn` is asked again while waiting.
const POLL: Duration = Duration::from_secs(2);
/// As in the app (`src-tauri/src/lib.rs`): MDK can outgrow the default 2 MiB stack.
const WORKER_STACK: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK)
        .build()
        .expect("failed to build the async runtime")
        .block_on(run())
}

async fn run() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(relay), Some(wn)) = (args.next(), args.next()) else {
        eprintln!("usage: white_noise_check <relay url> <path to wn>");
        return ExitCode::from(2);
    };
    println!("checking peoples.chat with White Noise on {relay}\n");

    let wn_home = tempfile::tempdir().expect("a temporary folder for wn");
    let wn = Wn {
        program: PathBuf::from(wn),
        home: wn_home.path().to_path_buf(),
        account: String::new(),
    };
    keyring_core::set_default_store(keyring_core::mock::Store::new().expect("a mock keychain"));
    let core_data = tempfile::tempdir().expect("a temporary folder for the core");
    let core = Core::start(MessagingOptions {
        data_dir: core_data.path().to_path_buf(),
        relays: vec![relay.clone()],
        keychain_service: "chat.peoples.client.white-noise-check".to_owned(),
        keys_in_file: false,
        allow_loopback: true,
        media_server: None,
        profile_copies: Vec::new(),
        chat_media_servers: Vec::new(),
        lookup_relays: Vec::new(),
    })
    .await
    .expect("the core starts");

    let passed = match setup(&core, wn, &relay).await {
        Err(detail) => report("both accounts set up", Err(detail)),
        Ok(wn) => {
            let results = [
                report("both accounts set up", Ok(String::new())),
                report(
                    "peoples.chat starts a chat with White Noise",
                    from_peoples_chat(&core, &wn).await,
                ),
                report(
                    "White Noise starts a chat with peoples.chat",
                    from_white_noise(&core, &wn).await,
                ),
            ];
            wn.cleanup();
            results.iter().all(|passed| *passed)
        }
    };
    if core.snapshot().account.is_some()
        && let Err(error) = core.delete_account().await
    {
        println!("      could not delete the peoples.chat account: {error}");
    }
    let _ = core.close().await;
    if passed {
        println!("\nall checks passed");
        ExitCode::SUCCESS
    } else {
        println!("\nsome checks failed");
        ExitCode::FAILURE
    }
}

type Check = Result<String, String>;

fn report(name: &str, result: Check) -> bool {
    match &result {
        Ok(detail) if detail.is_empty() => println!("ok    {name}"),
        Ok(detail) => println!("ok    {name}: {detail}"),
        Err(detail) => println!("FAIL  {name}: {detail}"),
    }
    result.is_ok()
}

async fn setup(core: &Core, mut wn: Wn, relay: &str) -> Result<Wn, String> {
    wn.run(&[
        "daemon",
        "start",
        "--discovery-relays",
        relay,
        "--default-account-relays",
        relay,
    ])?;
    let identity = wn.run(&["create-identity"])?;
    wn.account = find_str(&identity, &["account_id", "account_id_hex", "pubkey"])
        .ok_or_else(|| format!("no account in {identity}"))?;
    wn.run(&["profile", "update", "--name", "White Noise Check"])?;
    core.create_account("peoples.chat Check")
        .await
        .map_err(|error| format!("the peoples.chat account: {error}"))?;
    core.wait_until_published(WAIT)
        .await
        .map_err(|error| format!("the peoples.chat account was not published: {error}"))?;
    Ok(wn)
}

/// Check 1.
async fn from_peoples_chat(core: &Core, wn: &Wn) -> Check {
    let started = tokio::time::Instant::now();
    let chat = core
        .add_contact(&wn.account)
        .await
        .map_err(|error| format!("adding the White Noise account: {error}"))?;
    let group = wn
        .until("the invitation reaches White Noise", |wn| {
            let invites = wn.run(&["groups", "invites"]).ok()?;
            find_str(&invites, &["group_id", "mls_group_id"])
        })
        .await?;
    wn.run(&["groups", "accept", &group])?;

    let text = "Hello White Noise, from peoples.chat";
    core.send_message(&chat.id, text)
        .await
        .map_err(|error| format!("sending from peoples.chat: {error}"))?;
    wn.until("peoples.chat's message reaches White Noise", |wn| {
        let messages = wn
            .run(&["messages", "list", &group, "--limit", "20"])
            .ok()?;
        contains_text(&messages, text).then_some(())
    })
    .await?;

    let reply = "Hello peoples.chat, from White Noise";
    let mut events = core.subscribe();
    wn.run(&["messages", "send", &group, reply])?;
    arrives(&mut events, reply).await?;
    Ok(format!(
        "invitation, a message each way, in {:.1} s",
        started.elapsed().as_secs_f32()
    ))
}

/// Check 2.
async fn from_white_noise(core: &Core, wn: &Wn) -> Check {
    let started = tokio::time::Instant::now();
    let account = core.snapshot().account.ok_or("no peoples.chat account")?;
    let mut events = core.subscribe();
    // MDK takes a chat with no name and two members for a direct chat.
    let created = wn.run(&["groups", "create", "", &account.public_key])?;
    let group =
        find_str(&created, &["group_id"]).ok_or_else(|| format!("no group in {created}"))?;
    let invitation = tokio::time::timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(CoreEvent::InvitationReceived { invitation }) => return Some(invitation),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
    .ok_or("White Noise's invitation did not reach peoples.chat as a request")?;
    let chat = core
        .accept_invitation(&invitation.id)
        .await
        .map_err(|error| format!("accepting White Noise's invitation: {error}"))?;

    let text = "Hi, White Noise started this chat";
    let mut events = core.subscribe();
    wn.run(&["messages", "send", &group, text])?;
    arrives(&mut events, text).await?;

    let reply = "And peoples.chat answers";
    core.send_message(&chat.id, reply)
        .await
        .map_err(|error| format!("sending from peoples.chat: {error}"))?;
    wn.until("peoples.chat's answer reaches White Noise", |wn| {
        let messages = wn
            .run(&["messages", "list", &group, "--limit", "20"])
            .ok()?;
        contains_text(&messages, reply).then_some(())
    })
    .await?;
    Ok(format!(
        "request accepted, a message each way, in {:.1} s",
        started.elapsed().as_secs_f32()
    ))
}

/// `text` arrives at the core as an incoming message.
async fn arrives(
    events: &mut tokio::sync::broadcast::Receiver<CoreEvent>,
    text: &str,
) -> Result<(), String> {
    let arrived = tokio::time::timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(CoreEvent::MessageAdded { message, .. })
                    if message.direction == Direction::In && message.text == text =>
                {
                    return true;
                }
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await;
    match arrived {
        Ok(true) => Ok(()),
        _ => Err(format!("{text:?} did not reach peoples.chat")),
    }
}

/// White Noise's command-line app, with a home of its own and its key in a file there.
struct Wn {
    program: PathBuf,
    home: PathBuf,
    /// The account's public key, in hex, once it has one.
    account: String,
}

impl Wn {
    /// Runs one command and returns its JSON result, or why it failed.
    fn run(&self, args: &[&str]) -> Result<Value, String> {
        let mut command = Command::new(&self.program);
        command
            .arg("--home")
            .arg(&self.home)
            .args(["--secret-store", "file", "--json"]);
        if !self.account.is_empty() {
            command.args(["--account", &self.account]);
        }
        let output = command
            .args(args)
            .output()
            .map_err(|error| format!("running wn: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let value: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
        if !output.status.success() || value.get("error").is_some_and(|e| !e.is_null()) {
            return Err(format!(
                "wn {}: {} {}",
                args.join(" "),
                stdout.trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(value.get("result").cloned().unwrap_or(value))
    }

    /// Asks again until `found` finds something, for at most [`WAIT`].
    async fn until<T>(
        &self,
        what: &str,
        mut found: impl FnMut(&Self) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if let Some(value) = found(self) {
                return Ok(value);
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("timed out waiting until {what}"));
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Deletes the account and stops the daemon.
    fn cleanup(&self) {
        if !self.account.is_empty()
            && let Err(error) = self.run(&["logout", &self.account])
        {
            println!("      could not delete the White Noise account: {error}");
        }
        let _ = self.run(&["daemon", "stop"]);
        let _ = wait_gone(&self.home);
    }
}

/// Gives the daemon a moment to let go of its home before the folder is removed.
fn wait_gone(home: &Path) -> bool {
    let socket = home.join("dev").join("wnd.sock");
    for _ in 0..20 {
        if !socket.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// The first string under one of `keys`, anywhere in `value`.
fn find_str(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(found)) = map.get(*key)
                    && !found.is_empty()
                {
                    return Some(found.clone());
                }
            }
            map.values().find_map(|value| find_str(value, keys))
        }
        Value::Array(items) => items.iter().find_map(|value| find_str(value, keys)),
        _ => None,
    }
}

/// Whether some string in `value` is `text`.
fn contains_text(value: &Value, text: &str) -> bool {
    match value {
        Value::String(found) => found == text,
        Value::Object(map) => map.values().any(|value| contains_text(value, text)),
        Value::Array(items) => items.iter().any(|value| contains_text(value, text)),
        _ => false,
    }
}
