//! Confirmation of radio messages (#41). A message the other radio confirms shows as
//! confirmed. Without a confirmation in time it goes once more, while the chat is still
//! reachable by radio and the hour's airtime allows. A retry that gets through is confirmed too,
//! and the other phone shows the message once. To a radio that is switched off, the message
//! times out after the retry, and nothing more goes out; once a relay is back, the internet
//! carries it. Every try is in the transmission log (#52).

mod support;

use std::sync::Arc;
use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::interface::{
    CoreEvent, DeliveryState, Mesh, Message, RadioIssue, Reach, Transport,
};
use peoples_chat_core::radio::airtime::{meshcore_text_bytes, time_on_air};
use peoples_chat_core::radio::format::OVERHEAD_BYTES;
use peoples_chat_core::radio::{MOCK_SETTINGS, MockMesh, MockRadio, Radio};
use support::{Phone, TestRelay, WAIT, make_contacts, until};
use tokio::time::Instant;

/// How long the tests wait for each confirmation, in place of the radio's estimate.
const CONFIRM_WAIT: Duration = Duration::from_millis(700);

fn messages(core: &Core, chat_id: &str) -> Vec<Message> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| chat.messages)
        .unwrap_or_default()
}

fn delivery(core: &Core, chat_id: &str, id: &str) -> Option<DeliveryState> {
    messages(core, chat_id)
        .into_iter()
        .find(|message| message.id == id)
        .and_then(|message| message.delivery)
}

async fn until_delivery(core: &Core, chat_id: &str, id: &str, expected: DeliveryState) {
    until(&format!("{expected:?}"), || {
        (delivery(core, chat_id, id) == Some(expected)).then_some(())
    })
    .await;
}

/// Why each transmission in the log went out.
fn reasons(core: &Core) -> Vec<String> {
    core.radio_transmissions()
        .into_iter()
        .map(|sent| sent.reason)
        .collect()
}

fn chat_state(core: &Core, chat_id: &str) -> Option<(Reach, Option<RadioIssue>)> {
    core.snapshot()
        .chats
        .into_iter()
        .find(|chat| chat.id == chat_id)
        .map(|chat| (chat.reach, chat.radio_issue))
}

/// How many messages in the chat have `text`.
fn copies(core: &Core, chat_id: &str, text: &str) -> usize {
    messages(core, chat_id)
        .iter()
        .filter(|message| message.text == text)
        .count()
}

/// Two phones in a direct chat, each with a mock radio that knows the other's, and the relay
/// blocked: the chat goes by radio.
struct Bench {
    relay: TestRelay,
    ada_phone: Phone,
    /// Keeps Bea's data folder.
    _bea_phone: Phone,
    ada: Core,
    bea: Core,
    mesh: MockMesh,
    bea_radio: Arc<MockRadio>,
    chat_id: String,
}

async fn bench(name: &str) -> Bench {
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new(&format!("{name}-ada"), &url);
    let bea_phone = Phone::new(&format!("{name}-bea"), &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    until("Bea's chat", || chat_state(&bea, &chat_id)).await;
    settled(&ada, &bea, &chat_id).await;
    let mesh = MockMesh::new();
    let ada_radio = Arc::new(mesh.radio("Ada's radio"));
    let bea_radio = Arc::new(mesh.radio("Bea's radio"));
    ada.attach_radio(ada_radio).await.unwrap();
    bea.attach_radio(bea_radio.clone()).await.unwrap();
    until("Bea's radio", || {
        ada.snapshot()
            .chats
            .iter()
            .any(|chat| chat.id == chat_id && chat.contact.meshes == vec![Mesh::MeshCore])
            .then_some(())
    })
    .await;
    ada.set_radio_confirm_wait(CONFIRM_WAIT);

    relay.block();
    until("Ada's chat cut off", || {
        (chat_state(&ada, &chat_id).map(|(reach, _)| reach) == Some(Reach::Unreachable))
            .then_some(())
    })
    .await;
    bea_radio.advertise();
    until("Ada's chat by radio", || {
        (chat_state(&ada, &chat_id) == Some((Reach::Radio, None))).then_some(())
    })
    .await;
    Bench {
        relay,
        ada_phone,
        _bea_phone: bea_phone,
        ada,
        bea,
        mesh,
        bea_radio,
        chat_id,
    }
}

async fn settled(one: &Core, other: &Core, chat_id: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        if let (Ok(a), Ok(b)) = (
            one.chat_membership(chat_id).await,
            other.chat_membership(chat_id).await,
        ) && a.epoch == b.epoch
        {
            return;
        }
        assert!(Instant::now() < deadline, "the chat didn't settle");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_is_confirmed_or_times_out_after_one_retry() {
    let _turn = support::take_turn().await;
    let Bench {
        relay,
        ada_phone,
        _bea_phone,
        ada,
        bea,
        mesh,
        bea_radio,
        chat_id,
    } = bench("confirm").await;

    // 1. Bea's radio is on and in range: it confirms the message.
    let first = ada
        .send_message(&chat_id, "Can you hear me?")
        .await
        .unwrap();
    assert_eq!(first.transport, Transport::Radio);
    until_delivery(&ada, &chat_id, &first.id, DeliveryState::Confirmed).await;
    assert_eq!(reasons(&ada), ["message"]);

    // 2. The first try is lost, and the retry gets through: its confirmation counts, and Bea
    //    has the message once.
    mesh.set_in_range(bea_radio.public_key(), false);
    let second = ada.send_message(&chat_id, "Hello again").await.unwrap();
    assert_eq!(second.delivery, Some(DeliveryState::Waiting));
    mesh.set_in_range(bea_radio.public_key(), true);
    until_delivery(&ada, &chat_id, &second.id, DeliveryState::Confirmed).await;
    until("the retry at Bea", || {
        (copies(&bea, &chat_id, "Hello again") == 1).then_some(())
    })
    .await;
    assert_eq!(reasons(&ada), ["message", "message", "retry"]);

    // 3. Bea switches her radio off. Her radio was heard just now, so the chat still goes by
    //    radio: the message goes, goes once more, and times out. That is said once, and nothing
    //    more goes out.
    bea_radio.disconnect().await.unwrap();
    let mut events = ada.subscribe();
    let lost = ada.send_message(&chat_id, "Are you there?").await.unwrap();
    assert_eq!(
        (lost.transport, lost.delivery),
        (Transport::Radio, Some(DeliveryState::Waiting))
    );
    until_delivery(&ada, &chat_id, &lost.id, DeliveryState::TimedOut).await;
    tokio::time::sleep(CONFIRM_WAIT * 3).await;
    assert_eq!(
        reasons(&ada),
        ["message", "message", "retry", "message", "retry"]
    );
    let mut changes = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::DeliveryChanged {
            message_id,
            delivery,
            ..
        } = event
            && message_id == lost.id
        {
            changes.push(delivery);
        }
    }
    assert_eq!(changes, [DeliveryState::TimedOut]);

    // 4. Across a restart, it stays timed out, and the others stay confirmed.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    until_delivery(&ada, &chat_id, &lost.id, DeliveryState::TimedOut).await;
    assert_eq!(
        delivery(&ada, &chat_id, &second.id),
        Some(DeliveryState::Confirmed)
    );

    // 5. Once a relay is back, the internet carries it: a relay acknowledges it, and Bea has
    //    each message once.
    relay.unblock();
    until_delivery(&ada, &chat_id, &lost.id, DeliveryState::Sent).await;
    until("the message at Bea", || {
        (copies(&bea, &chat_id, "Are you there?") == 1).then_some(())
    })
    .await;
    for text in ["Can you hear me?", "Hello again", "Are you there?"] {
        assert_eq!(copies(&bea, &chat_id, text), 1, "{text}");
    }
    assert_eq!(
        delivery(&ada, &chat_id, &first.id),
        Some(DeliveryState::Confirmed)
    );

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_retry_once_radio_cannot_reach_the_chat() {
    let _turn = support::take_turn().await;
    let Bench {
        relay: _relay,
        // The phones keep their data folders.
        ada_phone: _ada_phone,
        _bea_phone,
        ada,
        bea,
        mesh,
        bea_radio,
        chat_id,
    } = bench("no-retry").await;

    // Bea's radio was heard just now, and then not again: by the time a retry would go, it
    // counts as gone. The message times out after one try.
    ada.set_radio_heard_window(Duration::from_secs(3));
    ada.set_radio_confirm_wait(Duration::from_secs(6));
    bea_radio.advertise();
    mesh.set_in_range(bea_radio.public_key(), false);
    let unheard = ada.send_message(&chat_id, "Short 1").await.unwrap();
    assert_eq!(unheard.transport, Transport::Radio);
    until_delivery(&ada, &chat_id, &unheard.id, DeliveryState::TimedOut).await;
    assert_eq!(
        chat_state(&ada, &chat_id),
        Some((Reach::Unreachable, Some(RadioIssue::NotHeard)))
    );
    assert_eq!(reasons(&ada), ["message"]);

    // The hour's airtime has room for one more message, but not for its retry.
    let each = time_on_air(
        &MOCK_SETTINGS,
        meshcore_text_bytes((4 * (OVERHEAD_BYTES + "Short 2".len())).div_ceil(3)),
    );
    ada.set_radio_hourly_airtime(each * 2 + Duration::from_millis(10));
    ada.set_radio_heard_window(Duration::from_secs(60 * 60));
    ada.set_radio_confirm_wait(CONFIRM_WAIT);
    mesh.set_in_range(bea_radio.public_key(), true);
    bea_radio.advertise();
    until("Ada's chat by radio again", || {
        (chat_state(&ada, &chat_id) == Some((Reach::Radio, None))).then_some(())
    })
    .await;
    mesh.set_in_range(bea_radio.public_key(), false);
    let capped = ada.send_message(&chat_id, "Short 2").await.unwrap();
    assert_eq!(capped.transport, Transport::Radio);
    until_delivery(&ada, &chat_id, &capped.id, DeliveryState::TimedOut).await;
    assert_eq!(reasons(&ada), ["message", "message"]);

    ada.close().await.unwrap();
    bea.close().await.unwrap();
}
