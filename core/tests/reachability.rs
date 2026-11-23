//! A chat's reach (#43): when the relay stops answering, the chat is marked unreachable within a
//! set time, and marked as on the internet again once the relay is back. A short drop changes
//! nothing. Two cores share a relay whose front door can be shut, as two phones would.

mod support;

use std::time::Duration;

use peoples_chat_core::interface::{
    CoreEvent, DeliveryState, Reach, Relay, RelayOperator, RelayStatus,
};
use peoples_chat_core::{Core, DOWN_AFTER};
use support::{Phone, TestRelay, make_contacts, until};
use tokio::sync::broadcast;
use tokio::time::Instant;

/// How often MDK tries a closed connection again.
const RETRY: Duration = Duration::from_secs(5);
/// Time for MDK to notice a change and for the core to pass it on.
const SLACK: Duration = Duration::from_secs(5);

/// The next change of the chat's reach, skipping other events, if one comes within `within`.
async fn next_reach(
    events: &mut broadcast::Receiver<CoreEvent>,
    chat_id: &str,
    within: Duration,
) -> Option<Reach> {
    let deadline = Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => return None,
            Ok(Ok(CoreEvent::ReachChanged {
                chat_id: changed,
                reach,
                ..
            })) if changed == chat_id => return Some(reach),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => panic!("the core stopped sending events: {error}"),
        }
    }
}

/// The one relay of the tests, as the snapshot shows it.
fn only_relay(url: &str, status: RelayStatus) -> Vec<Relay> {
    vec![Relay {
        url: url.to_owned(),
        operator: RelayOperator::Independent,
        status,
    }]
}

fn reach(core: &Core, chat_id: &str) -> Option<Reach> {
    let chats = core.snapshot().chats;
    chats
        .iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.reach)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_is_unreachable_while_its_relay_is_blocked() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("reach-ada", &url);
    let bea_phone = Phone::new("reach-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    let mut ada_events = ada.subscribe();
    let mut bea_events = bea.subscribe();
    assert_eq!(reach(&ada, &chat_id), Some(Reach::Internet));
    assert_eq!(
        ada.snapshot().relays,
        only_relay(&url, RelayStatus::Connected)
    );

    // A drop that MDK's next attempt mends changes nothing.
    relay.block();
    tokio::time::sleep(Duration::from_secs(1)).await;
    relay.unblock();
    assert_eq!(
        next_reach(&mut ada_events, &chat_id, DOWN_AFTER + SLACK).await,
        None
    );
    assert_eq!(reach(&ada, &chat_id), Some(Reach::Internet));

    // Blocked for longer, the chat is cut off on both phones once the connections have stayed
    // closed for DOWN_AFTER.
    let blocked = Instant::now();
    relay.block();
    assert_eq!(
        next_reach(&mut ada_events, &chat_id, DOWN_AFTER + SLACK).await,
        Some(Reach::Unreachable)
    );
    let took = blocked.elapsed();
    assert!(took >= DOWN_AFTER, "cut off after {took:?}");
    assert_eq!(reach(&ada, &chat_id), Some(Reach::Unreachable));
    // The relay itself says so too.
    assert_eq!(
        ada.snapshot().relays,
        only_relay(&url, RelayStatus::Unreachable)
    );
    assert_eq!(
        next_reach(&mut bea_events, &chat_id, SLACK).await,
        Some(Reach::Unreachable)
    );

    // What Ada writes now waits.
    let waiting = ada.send_message(&chat_id, "Still there?").await.unwrap();
    assert_eq!(waiting.delivery, Some(DeliveryState::Waiting));

    // Unblocked, the chat is back with MDK's next attempt, and the message goes out.
    relay.unblock();
    assert_eq!(
        next_reach(&mut ada_events, &chat_id, RETRY + SLACK).await,
        Some(Reach::Internet)
    );
    assert_eq!(reach(&ada, &chat_id), Some(Reach::Internet));
    assert_eq!(
        ada.snapshot().relays,
        only_relay(&url, RelayStatus::Connected)
    );
    assert_eq!(
        next_reach(&mut bea_events, &chat_id, RETRY + SLACK).await,
        Some(Reach::Internet)
    );
    until("the relay's acknowledgement", || {
        let chat = ada.snapshot().chats.into_iter().find(|c| c.id == chat_id)?;
        let sent = chat.messages.iter().find(|m| m.id == waiting.id)?;
        (sent.delivery == Some(DeliveryState::Sent)).then_some(())
    })
    .await;
    // Bea can get it later than this test waits: MDK holds what it fetches after reconnecting
    // until it has the relay's NIP-11 limits, and keeps a fetch that failed during the outage for
    // 5 minutes (#78; docs/mdk.md, "After an outage").
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_core_that_starts_without_its_relay_marks_its_chats_unreachable() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("offline-start-ada", &url);
    let bea_phone = Phone::new("offline-start-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    ada.close().await.unwrap();
    bea.close().await.unwrap();

    // Ada's app starts while the relay can't be reached: the chat is on the internet until the
    // connection has had its time, then unreachable.
    relay.block();
    let started = Instant::now();
    let ada = ada_phone.start().await;
    let mut events = ada.subscribe();
    if reach(&ada, &chat_id) == Some(Reach::Internet) {
        assert_eq!(
            next_reach(&mut events, &chat_id, DOWN_AFTER + SLACK).await,
            Some(Reach::Unreachable)
        );
    }
    let took = started.elapsed();
    assert!(took >= DOWN_AFTER, "cut off after {took:?}");

    relay.unblock();
    assert_eq!(
        next_reach(&mut events, &chat_id, RETRY + SLACK).await,
        Some(Reach::Internet)
    );
    ada.close().await.unwrap();
}
