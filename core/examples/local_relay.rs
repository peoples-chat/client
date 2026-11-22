//! A Nostr relay on this machine, for trying the core from a simulator, an emulator or another
//! process. It prints every event it receives, so you can see what an app published.
//!
//! It is the relay the core's tests use (`tests/support/relay.rs`): it describes itself the way
//! public relays do, and it can be blocked. Type `block` and Enter to cut every connection and
//! refuse new ones, as when the internet is gone, and `unblock` to let them through again. That
//! takes the relays away from the apps without turning off their phones' Bluetooth (#43).
//!
//! Usage: `cargo run -p peoples-chat-core --example local_relay -- [port]` (port 7777 by default).
//! The iOS simulator reaches it at `ws://127.0.0.1:<port>`. For the Android emulator, run
//! `adb reverse tcp:<port> tcp:<port>` first; the emulator then reaches it at the same address.

// The tests use the rest of it.
#[allow(dead_code)]
#[path = "../tests/support/relay.rs"]
mod relay;

use std::collections::HashSet;
use std::time::Duration;

use nostr_relay_builder::prelude::*;
use relay::TestRelay;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = match std::env::args().nth(1) {
        Some(port) => port.parse()?,
        None => 7777,
    };
    let database = MemoryDatabase::with_opts(MemoryDatabaseOptions {
        events: true,
        max_events: None,
    });
    let relay = TestRelay::start_with(
        RelayBuilder::default().database(database.clone()),
        &format!("127.0.0.1:{port}"),
    )
    .await;
    println!("relay listening at {}", relay.url());
    println!("type `block` or `unblock` and Enter to take it away from the apps or give it back");

    // Commands come from a thread of their own: a read from the terminal can't be cancelled, and
    // Tokio's own stdin would hold up the exit until Enter is pressed.
    let (sender, mut commands) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    // Input can end, for example when the relay runs in the background. It keeps running.
    let mut reading = true;
    let mut seen = HashSet::new();
    let stop = tokio::signal::ctrl_c();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            command = commands.recv(), if reading => match command.as_deref().map(str::trim) {
                Some("block" | "b") => {
                    relay.block();
                    println!("blocked");
                }
                Some("unblock" | "u") => {
                    relay.unblock();
                    println!("unblocked");
                }
                Some("") => {}
                Some(other) => println!("unknown command {other:?}: type `block` or `unblock`"),
                None => reading = false,
            },
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                for event in database.query(Filter::new()).await? {
                    if seen.insert(event.id) {
                        println!("event kind {} from {}", event.kind.as_u16(), event.pubkey.to_hex());
                    }
                }
            }
        }
    }
    Ok(())
}
