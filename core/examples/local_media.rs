//! A Blossom media server on this machine, for profile photos from apps in a simulator or an
//! emulator (#94). It keeps what is uploaded in memory and prints each upload, so nothing reaches
//! a public media server during a test.
//!
//! It is the server the core's tests use (`tests/support/blossom.rs`). Build the app with
//! `TAURI_PEOPLES_CHAT_MEDIA_SERVER=http://127.0.0.1:<port>`; only debug builds accept it. For the
//! Android emulator, run `adb reverse tcp:<port> tcp:<port>` first.
//!
//! Usage: `cargo run -p peoples-chat-core --example local_media -- [port]` (port 7778 by default).

// The tests use the rest of it.
#[allow(dead_code)]
#[path = "../tests/support/blossom.rs"]
mod blossom;

use std::time::Duration;

use blossom::TestBlossom;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = match std::env::args().nth(1) {
        Some(port) => port.parse()?,
        None => 7778,
    };
    let server = TestBlossom::start_at(&format!("127.0.0.1:{port}")).await;
    println!("media server listening at {}", server.url());
    let mut seen = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let blobs = server.blobs();
        if blobs.len() > seen {
            for blob in &blobs[seen..] {
                println!("upload of {} bytes", blob.len());
            }
            seen = blobs.len();
        }
    }
}
