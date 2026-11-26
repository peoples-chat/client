//! Issue #132: the core's MeshCore radio against a pair of fake companion radios over TCP.

use std::time::Duration;

use meshcore_rs::MeshCore;
use peoples_chat_core::radio::{MeshCoreRadio, Radio, RadioEvent};
use peoples_chat_fake_radio::{Config, FakeRadio, pair};
use tokio::sync::broadcast;

/// The core's radio over a TCP link to a fake. The link must be kept as long as the radio.
async fn connect(fake: &FakeRadio) -> (MeshCore, MeshCoreRadio) {
    let addr = fake.addr();
    let link = MeshCore::tcp(&addr.ip().to_string(), addr.port())
        .await
        .unwrap();
    let radio = MeshCoreRadio::over(link.commands().clone(), link.dispatcher())
        .await
        .unwrap();
    (link, radio)
}

async fn next_event(events: &mut broadcast::Receiver<RadioEvent>) -> RadioEvent {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("no radio event in time")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_core_sends_receives_and_gets_confirmation_over_fake_radios() {
    let (ada, bea) = pair(Config::default(), Config::default()).await.unwrap();
    let (_ada_link, ada_radio) = connect(&ada).await;
    let (_bea_link, bea_radio) = connect(&bea).await;
    let mut ada_events = ada_radio.events();
    let mut bea_events = bea_radio.events();

    let sent = ada_radio
        .send_text(&hex::encode(bea.public_key()), "Storm tonight")
        .await
        .unwrap();
    match next_event(&mut bea_events).await {
        RadioEvent::TextReceived { text, from, .. } => {
            assert_eq!(text, "Storm tonight");
            assert_eq!(from, Some(hex::encode(ada.public_key())));
        }
        other => panic!("expected a message, got {other:?}"),
    }
    assert_eq!(
        next_event(&mut ada_events).await,
        RadioEvent::Delivered { ack: sent.ack }
    );
}
