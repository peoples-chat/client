//! Radio keys (#37): two contacts derive the same keys for their chat from its MLS state, with no
//! extra key exchange. The keys stay the same across a restart, and change with the epoch.

mod support;

use std::time::Duration;

use peoples_chat_core::Core;
use peoples_chat_core::radio::RadioKeys;
use support::{Phone, TestRelay, WAIT, make_contacts};
use tokio::time::Instant;

/// MDK changes a chat's epoch on its own schedule: a key update waits for a minute without
/// changes to the chat, plus up to 30 seconds, before it goes out.
const ROTATION_WAIT: Duration = Duration::from_secs(4 * 60);

/// Both phones' keys for the chat, once both are settled in the same epoch, after `after` if
/// given. A phone can be a change ahead or behind until the relay has passed the change on, and
/// has no keys while its own change waits for the relay.
async fn keys_in_step(
    ada: &Core,
    bea: &Core,
    chat_id: &str,
    after: Option<u64>,
    within: Duration,
) -> (RadioKeys, RadioKeys) {
    let deadline = Instant::now() + within;
    loop {
        let ada_keys = ada.radio_keys(chat_id).await;
        let bea_keys = bea.radio_keys(chat_id).await;
        match (ada_keys, bea_keys) {
            (Ok(ada_keys), Ok(bea_keys))
                if ada_keys.epoch == bea_keys.epoch
                    && after.is_none_or(|after| ada_keys.epoch > after) =>
            {
                return (ada_keys, bea_keys);
            }
            (ada_keys, bea_keys) => assert!(
                Instant::now() < deadline,
                "not in step after {after:?}: {ada_keys:?} and {bea_keys:?}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// What one sends with, the other receives with.
fn assert_paired(ada: &RadioKeys, bea: &RadioKeys) {
    assert_eq!(ada.epoch, bea.epoch);
    assert_eq!(*ada.send, *bea.receive);
    assert_eq!(*ada.receive, *bea.send);
    assert_ne!(*ada.send, *ada.receive);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_contacts_derive_the_same_radio_keys() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("keys-ada", &url);
    let bea_phone = Phone::new("keys-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;

    let (ada_keys, bea_keys) = keys_in_step(&ada, &bea, &chat_id, None, WAIT).await;
    assert_paired(&ada_keys, &bea_keys);

    // The same epoch gives the same keys, also after a restart.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    let again = ada.radio_keys(&chat_id).await.unwrap();
    assert_eq!(again.epoch, ada_keys.epoch);
    assert_eq!(*again.send, *ada_keys.send);
    assert_eq!(*again.receive, *ada_keys.receive);

    let error = ada.radio_keys("0123abcd").await.unwrap_err();
    assert_eq!(error.message, "This chat no longer exists.");
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "slow: waits for two of MDK's key updates, about three minutes"]
async fn radio_keys_change_with_the_epoch() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("rotation-ada", &url);
    let bea_phone = Phone::new("rotation-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;
    let joined = Instant::now();
    let (first, _) = keys_in_step(&ada, &bea, &chat_id, None, WAIT).await;
    println!("epoch {} after joining", first.epoch);

    // After joining, MDK updates Bea's key by itself.
    let (ada_keys, bea_keys) =
        keys_in_step(&ada, &bea, &chat_id, Some(first.epoch), ROTATION_WAIT).await;
    println!("epoch {} after {:?}", ada_keys.epoch, joined.elapsed());
    assert_paired(&ada_keys, &bea_keys);
    assert_ne!(*ada_keys.send, *first.send);
    assert_ne!(*ada_keys.receive, *first.receive);

    // Ada asks for an update of her own.
    let asked = Instant::now();
    ada.update_own_key(&chat_id).await.unwrap();
    let (ada_next, bea_next) =
        keys_in_step(&ada, &bea, &chat_id, Some(ada_keys.epoch), ROTATION_WAIT).await;
    println!("epoch {} after {:?}", ada_next.epoch, asked.elapsed());
    assert_paired(&ada_next, &bea_next);
    assert_ne!(*ada_next.send, *ada_keys.send);
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}
