//! Issue #135: a fake radio adds a sender it doesn't hold when it hears that sender announce
//! itself, as `verify:radio` relies on to make two radios contacts. Adverts from contacts keep
//! the rules of #134.

use std::time::{Duration, Instant};

use meshcore_rs::MeshCore;
use peoples_chat_fake_radio::{Config, FakeRadio, put_in_range};

async fn connect(radio: &FakeRadio) -> MeshCore {
    let addr = radio.addr();
    let app = MeshCore::tcp(&addr.ip().to_string(), addr.port())
        .await
        .expect("meshcore-rs didn't connect");
    app.commands().lock().await.send_appstart().await.unwrap();
    app
}

/// The keys of the contacts a radio holds, with the advert time held for each.
async fn contacts(app: &MeshCore) -> Vec<([u8; 32], u32)> {
    let contacts = app.commands().lock().await.get_contacts(0).await.unwrap();
    contacts
        .iter()
        .map(|contact| (contact.public_key, contact.last_advert))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_135_an_advert_makes_its_sender_a_contact_stamped_with_the_senders_clock() {
    let one = FakeRadio::start(Config::default()).await.unwrap();
    let two = FakeRadio::start(Config::default()).await.unwrap();
    put_in_range(&one, &two);
    // A clock that is behind: the time held is the sender's, not the receiver's.
    one.set_clock(1_715_824_653);
    let (first, second) = (connect(&one).await, connect(&two).await);
    assert!(contacts(&second).await.is_empty());

    first
        .commands()
        .lock()
        .await
        .send_advert(false)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let held = loop {
        let held = contacts(&second).await;
        if !held.is_empty() {
            break held;
        }
        assert!(Instant::now() < deadline, "the sender was never added");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, one.public_key());
    assert!(held[0].1.abs_diff(1_715_824_653) < 10, "{}", held[0].1);
    // Hearing it doesn't make the sender hold the receiver.
    assert!(contacts(&first).await.is_empty());
}
