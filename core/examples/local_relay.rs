//! A Nostr relay on this machine, for trying the core from a simulator, an emulator or another
//! process. It prints every event it receives, so you can see what an app published.
//!
//! Usage: `cargo run -p peoples-chat-core --example local_relay -- [port]` (port 7777 by default).
//! The iOS simulator reaches it at `ws://127.0.0.1:<port>`. For the Android emulator, run
//! `adb reverse tcp:<port> tcp:<port>` first; the emulator then reaches it at the same address.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use nostr_relay_builder::prelude::*;

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
    let relay = LocalRelay::new(
        RelayBuilder::default()
            .addr(IpAddr::V4(Ipv4Addr::LOCALHOST))
            .port(port)
            .database(database.clone()),
    );
    relay.run().await?;
    println!("relay listening at {}", relay.url().await);

    let mut seen = HashSet::new();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
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
