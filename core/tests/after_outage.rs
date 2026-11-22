//! A measurement for #78: after an outage, how long until the other phone shows a message
//! written during it. Ignored, because a slow run can take minutes. Run it with:
//!
//! `cargo test -p peoples-chat-core --test after_outage -- --ignored --nocapture`
//!
//! On 22 November 2026, with the relay fully blocked (no NIP-11 answers either):
//!
//! - Since #46 the message waits in the core and goes once a relay is connected again. It
//!   arrived 5.1 seconds after the relay came back, in 4 of 4 runs.
//! - Before #46, when MDK held the message and published it at once on reconnecting, the same
//!   runs took 25.0 seconds, 4 of 4.
//! - MDK's full-history repair on the receiving phone, run as soon as its relay was back, made no
//!   difference to either.
//!
//! The multi-minute delays that #78 found didn't show in these runs.

mod support;

use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::Reach;
use support::{Phone, TestRelay, make_contacts, until};
use tokio::time::Instant;

/// How long a run waits for the message before calling it slow.
const GIVE_UP: Duration = Duration::from_secs(90);
const RUNS: usize = 4;

fn reach(core: &Core, chat_id: &str) -> Option<Reach> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.reach)
}

fn has(core: &Core, chat_id: &str, text: &str) -> bool {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .is_some_and(|chat| chat.messages.iter().any(|m| m.text == text))
}

/// One outage: the time from the relay's return until Bea shows Ada's message, or `None` after
/// [`GIVE_UP`].
async fn one_outage(name: &str) -> Option<Duration> {
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new(&format!("{name}-ada"), &url);
    let bea_phone = Phone::new(&format!("{name}-bea"), &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    until("Bea's chat", || reach(&bea, &chat_id)).await;

    relay.block();
    until("both cut off", || {
        (reach(&ada, &chat_id) == Some(Reach::Unreachable)
            && reach(&bea, &chat_id) == Some(Reach::Unreachable))
        .then_some(())
    })
    .await;
    ada.send_message(&chat_id, "Written during the outage")
        .await
        .unwrap();

    relay.unblock();
    let back = Instant::now();
    let arrived = loop {
        if has(&bea, &chat_id, "Written during the outage") {
            break Some(back.elapsed());
        }
        if back.elapsed() > GIVE_UP {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    ada.close().await.unwrap();
    bea.close().await.unwrap();
    arrived
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement for #78; slow runs take minutes"]
async fn how_late_a_message_written_during_an_outage_arrives() {
    let _turn = support::take_turn().await;
    let mut times = Vec::new();
    for run in 0..RUNS {
        let arrived = one_outage(&format!("outage-{run}")).await;
        println!(
            "run {run}: {}",
            arrived.map_or("not within 90 s".to_owned(), |t| format!(
                "{:.1} s",
                t.as_secs_f32()
            ))
        );
        times.push(arrived);
    }
    println!("{times:?}");
}
