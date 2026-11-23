//! A relay for the two-phone tests that describes itself the way public relays do.
//!
//! A relay inside the test process (`nostr_relay_builder`) serves no NIP-11 information document.
//! Without one, MDK can't tell whether it has seen a group's whole history, and holds back a
//! group's messages that were stored before the member joined. It releases them only when its
//! recovery gives up, after about 15 minutes. Public relays publish the document, so this relay
//! answers NIP-11 requests at its front door, with the filter limit it enforces, and passes
//! everything else through.
//!
//! The same pattern as `crates/marmot-app/tests/support/limited_relay.rs` in MDK (MIT).
//!
//! The front door can also be shut (#43): it then cuts every connection and refuses new ones, as
//! when the network is gone, while the relay keeps what it has. Shut with
//! [`TestRelay::block_messages`], it still answers NIP-11 requests: then MDK has the relay's
//! limits when it reconnects, and doesn't hold back what it fetches then (#78).
//!
//! `examples/local_relay.rs` runs the same relay on the Mac for the apps on phones.

use std::time::Duration;

use nostr_relay_builder::{LocalRelay, RelayBuilder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

/// The most events the relay returns for one filter, and says it does.
const MAX_LIMIT: usize = 500;
/// The largest HTTP header the front door reads.
const MAX_HEADER: usize = 8192;

/// A relay in the test process, reached through a front door that answers NIP-11 requests.
pub struct TestRelay {
    _relay: LocalRelay,
    door: JoinHandle<()>,
    url: String,
    blocked: watch::Sender<Shut>,
}

/// How far the front door is shut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shut {
    Open,
    /// Nothing gets through.
    All,
    /// Only NIP-11 requests are answered.
    Messages,
}

impl Drop for TestRelay {
    fn drop(&mut self) {
        self.door.abort();
    }
}

impl TestRelay {
    /// A relay with its front door at a free port on the loopback interface.
    pub async fn start() -> Self {
        Self::start_with(RelayBuilder::default(), "127.0.0.1:0").await
    }

    /// A relay built from `builder`, with its front door at `address`.
    pub async fn start_with(builder: RelayBuilder, address: &str) -> Self {
        let relay = LocalRelay::new(builder.max_filter_limit(MAX_LIMIT));
        relay.run().await.unwrap();
        let inside = relay.url().await.to_string();
        let inside = inside
            .trim_start_matches("ws://")
            .trim_end_matches('/')
            .to_owned();
        let listener = TcpListener::bind(address).await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (blocked, shut) = watch::channel(Shut::Open);
        let door = tokio::spawn(front_door(listener, inside, shut));
        Self {
            _relay: relay,
            door,
            url,
            blocked,
        }
    }

    /// The relay's URL, as the cores use it.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Cuts every connection to the relay and refuses new ones, as when the network is gone.
    pub fn block(&self) {
        self.blocked.send_replace(Shut::All);
    }

    /// Cuts every connection to the relay and refuses new ones, but goes on answering NIP-11
    /// requests. This keeps #78's delay out of a test about something else.
    pub fn block_messages(&self) {
        self.blocked.send_replace(Shut::Messages);
    }

    /// Lets connections through again.
    pub fn unblock(&self) {
        self.blocked.send_replace(Shut::Open);
    }
}

/// Answers NIP-11 requests itself, and passes every other connection to the relay, unless it is
/// shut.
async fn front_door(listener: TcpListener, inside: String, shut: watch::Receiver<Shut>) {
    let information =
        format!(r#"{{"supported_nips":[],"limitation":{{"max_limit":{MAX_LIMIT}}}}}"#);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((client, _)) = accepted else { continue };
                // Shut: the connection closes at once.
                if *shut.borrow() == Shut::All {
                    continue;
                }
                connections.spawn(serve(client, inside.clone(), information.clone(), shut.clone()));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve(
    mut client: TcpStream,
    inside: String,
    information: String,
    mut shut: watch::Receiver<Shut>,
) {
    let mut header = Vec::new();
    let read_header = async {
        while !header.windows(4).any(|window| window == b"\r\n\r\n") {
            let mut bytes = [0; 1024];
            let read = client.read(&mut bytes).await.ok()?;
            if read == 0 || header.len() + read > MAX_HEADER {
                return None;
            }
            header.extend_from_slice(&bytes[..read]);
        }
        Some(())
    };
    if tokio::time::timeout(Duration::from_secs(5), read_header).await != Ok(Some(())) {
        return;
    }
    let asks_for_information = String::from_utf8_lossy(&header).lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("accept") && value.contains("application/nostr+json")
        })
    });
    if asks_for_information {
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/nostr+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{information}",
            information.len()
        );
        let _ = client.write_all(reply.as_bytes()).await;
    } else if *shut.borrow() != Shut::Open {
        // Shut for messages: only the information gets through.
    } else if let Ok(mut relay) = TcpStream::connect(&inside).await
        && relay.write_all(&header).await.is_ok()
    {
        // Shutting the door closes the connection.
        tokio::select! {
            _ = tokio::io::copy_bidirectional(&mut client, &mut relay) => {}
            _ = shut.wait_for(|shut| *shut != Shut::Open) => {}
        }
    }
}
