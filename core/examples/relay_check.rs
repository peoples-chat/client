//! Checks a relay against what peoples.chat needs from it (#90, `relay/README.md`). Run it before
//! a relay becomes one of the app's defaults:
//!
//! 1. Two accounts that know only this relay set up, become contacts and exchange messages.
//! 2. A public note (kind 1) is refused.
//! 3. A group message of 1.5 MB is taken, and one of 2.2 MB is refused.
//! 4. A gift wrap reaches its recipient. It also says whether anyone else can read it: the
//!    project's relay serves gift wraps to anyone, for now (`relay/README.md`).
//! 5. An event is gone once its expiry time has passed (NIP-40).
//!
//! Check 2 holds only on a relay that restricts what it takes, as the project's relay does.
//!
//! Usage: `cargo run -p peoples-chat-core --example relay_check -- <relay url>`, for example
//! `wss://relay.peoples.chat`, or `ws://127.0.0.1:7777` for a relay on this machine.
//!
//! The accounts and keys are throwaway: they live in memory and a temporary folder, and both
//! accounts are deleted at the end. Their profiles and deletion requests stay on the relay.

// The tests use the rest of it.
#[allow(unused_imports)]
#[path = "../tests/support/mod.rs"]
mod support;

use std::process::ExitCode;
use std::time::Duration;

use nostr_sdk::prelude::*;
use peoples_chat_core::Core;
use peoples_chat_core::interface::{CoreEvent, Direction};
use support::{Phone, make_contacts};

/// How long to wait for the relay to answer one request.
const ANSWER: Duration = Duration::from_secs(10);

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
    let Some(url) = std::env::args().nth(1) else {
        eprintln!("usage: relay_check <relay url>");
        return ExitCode::from(2);
    };
    println!("checking {url}\n");
    let results = [
        report(
            "two accounts become contacts and chat",
            accounts(&url).await,
        ),
        report("a public note is refused", public_note(&url).await),
        report("large group messages", large_events(&url).await),
        report("gift wraps reach their recipient", gift_wraps(&url).await),
        report("expired events are dropped", expiry(&url).await),
    ];
    if results.iter().all(|passed| *passed) {
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
        Ok(detail) => println!("ok    {name}: {detail}"),
        Err(detail) => println!("FAIL  {name}: {detail}"),
    }
    result.is_ok()
}

/// Check 1: what the app itself does, through MDK.
async fn accounts(url: &str) -> Check {
    let ada_phone = Phone::new("relay-check-ada", url);
    let bea_phone = Phone::new("relay-check-bea", url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    let result = chat(&ada, &bea).await;
    for core in [&ada, &bea] {
        if core.snapshot().account.is_some()
            && let Err(error) = core.delete_account().await
        {
            println!("      could not delete a test account: {error}");
        }
        let _ = core.close().await;
    }
    result
}

async fn chat(ada: &Core, bea: &Core) -> Check {
    let started = tokio::time::Instant::now();
    ada.create_account("Relay Check Ada")
        .await
        .map_err(|error| format!("Ada's account: {error}"))?;
    bea.create_account("Relay Check Bea")
        .await
        .map_err(|error| format!("Bea's account: {error}"))?;
    for (who, core) in [("Ada", ada), ("Bea", bea)] {
        core.wait_until_published(support::WAIT)
            .await
            .map_err(|error| format!("{who}'s account was not published: {error}"))?;
    }
    // Panics in the shared helpers end the check with their message.
    let chat_id = tokio::spawn({
        let (ada, bea) = (ada.clone(), bea.clone());
        async move { make_contacts(&ada, &bea).await }
    })
    .await
    .map_err(|error| format!("becoming contacts: {error}"))?;
    send_and_receive(ada, bea, &chat_id, "Hello from Ada").await?;
    send_and_receive(bea, ada, &chat_id, "Hello from Bea").await?;
    Ok(format!("in {:.1} s", started.elapsed().as_secs_f32()))
}

/// `from` sends `text`, and it arrives at `to`.
async fn send_and_receive(from: &Core, to: &Core, chat_id: &str, text: &str) -> Result<(), String> {
    let mut events = to.subscribe();
    from.send_message(chat_id, text)
        .await
        .map_err(|error| format!("sending {text:?}: {error}"))?;
    let arrived = tokio::time::timeout(support::WAIT, async {
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
        _ => Err(format!("{text:?} did not arrive")),
    }
}

/// Check 2.
async fn public_note(url: &str) -> Check {
    let keys = Keys::generate();
    let note = EventBuilder::new(Kind::TextNote, "relay check")
        .finalize(&keys)
        .map_err(|error| error.to_string())?;
    match publish(url, &note).await? {
        Err(message) => Ok(format!("refused: {message}")),
        Ok(()) => Err("taken".to_owned()),
    }
}

/// Check 3: Marmot asks relays to take events of at least 2 MiB.
async fn large_events(url: &str) -> Check {
    let keys = Keys::generate();
    let message = |bytes: usize| {
        EventBuilder::new(Kind::MlsGroupMessage, "a".repeat(bytes))
            .tag(Tag::custom("h", [random_hex()]))
            .finalize(&keys)
            .map_err(|error| error.to_string())
    };
    if let Err(message) = publish(url, &message(1_500_000)?).await? {
        return Err(format!("1.5 MB refused: {message}"));
    }
    match publish(url, &message(2_200_000)?).await? {
        Err(message) => Ok(format!("1.5 MB taken, 2.2 MB refused: {message}")),
        Ok(()) => Ok("1.5 MB and 2.2 MB taken".to_owned()),
    }
}

/// Check 4.
async fn gift_wraps(url: &str) -> Check {
    let recipient = Keys::generate();
    let wrap = EventBuilder::new(Kind::GiftWrap, "relay check")
        .tag(Tag::public_key(recipient.public_key()))
        .finalize(&Keys::generate())
        .map_err(|error| error.to_string())?;
    if let Err(message) = publish(url, &wrap).await? {
        return Err(format!("refused: {message}"));
    }
    let filter = Filter::new()
        .kind(Kind::GiftWrap)
        .pubkey(recipient.public_key());
    let anyone = fetch(url, None, filter.clone()).await;
    let stranger = fetch(url, Some(Keys::generate()), filter.clone()).await;
    let addressee = fetch(url, Some(recipient), filter).await;
    let seen = |events: &Result<Vec<Event>, String>| {
        events
            .as_ref()
            .is_ok_and(|events| events.iter().any(|event| event.id == wrap.id))
    };
    match (seen(&anyone), seen(&stranger), seen(&addressee)) {
        (_, _, false) => Err(format!(
            "not served to its recipient{}",
            addressee
                .err()
                .map(|error| format!(" ({error})"))
                .unwrap_or_default()
        )),
        (false, false, true) => Ok("served to its recipient only, after authenticating".to_owned()),
        (true, _, true) => Ok("served to its recipient, and to anyone who asks".to_owned()),
        (false, true, true) => Ok("served to its recipient, and to other accounts".to_owned()),
    }
}

/// Check 5.
async fn expiry(url: &str) -> Check {
    let keys = Keys::generate();
    let expires = Timestamp::from_secs(Timestamp::now().as_secs() + 3);
    let message = EventBuilder::new(Kind::MlsGroupMessage, "relay check")
        .tag(Tag::custom("h", [random_hex()]))
        .tag(Tag::expiration(expires))
        .finalize(&keys)
        .map_err(|error| error.to_string())?;
    if let Err(message) = publish(url, &message).await? {
        return Err(format!("refused: {message}"));
    }
    let filter = Filter::new().id(message.id);
    let before = fetch(url, None, filter.clone()).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let after = fetch(url, None, filter).await?;
    match (before.len(), after.len()) {
        (1, 0) => Ok("served until it expired, then gone".to_owned()),
        (before, after) => Err(format!(
            "served {before} time(s) before expiry, {after} after"
        )),
    }
}

/// Publishes `event` to the relay alone. `Ok(Err(message))` is the relay's refusal.
async fn publish(url: &str, event: &Event) -> Result<Result<(), String>, String> {
    let client = Client::builder().build();
    client
        .add_relay(url)
        .await
        .map_err(|error| error.to_string())?;
    client.connect().await;
    let output = client
        .send_event(event)
        .ok_timeout(ANSWER)
        .await
        .map_err(|error| error.to_string());
    client.disconnect().await;
    let output = output?;
    if output.success.is_empty() {
        let message = output.failed.into_values().next().unwrap_or_default();
        Ok(Err(message))
    } else {
        Ok(Ok(()))
    }
}

/// The events matching `filter`, as an account with `keys` gets them (it answers the relay's
/// NIP-42 challenge), or as anyone without keys.
async fn fetch(url: &str, keys: Option<Keys>, filter: Filter) -> Result<Vec<Event>, String> {
    let client = match keys {
        Some(keys) => Client::builder()
            .authenticator(SignerAuthenticator::new(keys))
            .build(),
        None => Client::builder().build(),
    };
    client
        .add_relay(url)
        .await
        .map_err(|error| error.to_string())?;
    client.connect().await;
    let events = client
        .fetch_events(ReqTarget::single(url, [filter]))
        .timeout(ANSWER)
        .await
        .map(|events| events.into_iter().collect())
        .map_err(|error| error.to_string());
    client.disconnect().await;
    events
}

fn random_hex() -> String {
    Keys::generate().public_key().to_hex()
}
