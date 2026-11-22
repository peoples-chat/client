//! Profile photos (#94): a photo is made square and loses its metadata before it is uploaded, and
//! a contact's core shows it once it fetches the profiles again. Both keep it across a restart.

mod support;

use image::codecs::jpeg::JpegEncoder;
use image::{GenericImageView, Rgb, RgbImage};
use peoples_chat_core::interface::CoreEvent;
use support::{Phone, TestBlossom, TestRelay, make_contacts, next_event};

/// A photo as a phone camera takes it: large, and with an Exif segment, which is where the
/// location goes.
fn camera_photo() -> Vec<u8> {
    let image = RgbImage::from_fn(1600, 1200, |x, y| Rgb([(x / 7) as u8, (y / 5) as u8, 128]));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&image)
        .unwrap();
    let exif = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\0\0\0\0\0GPS 56.15N 10.21E";
    let mut photo = jpeg[..2].to_vec();
    photo.extend_from_slice(&[0xFF, 0xE1]);
    photo.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    photo.extend_from_slice(exif);
    photo.extend_from_slice(&jpeg[2..]);
    photo
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn is_avatar(photo: Option<&str>) -> bool {
    photo.is_some_and(|photo| photo.starts_with("data:image/jpeg;base64,"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_photo_is_cleaned_uploaded_and_shown_to_a_contact() {
    let _turn = support::take_turn().await;
    let relay = TestRelay::start().await;
    let media = TestBlossom::start().await;
    let url = relay.url();
    let ada_phone = Phone::new("photo-ada", &url).with_media_server(&media.url());
    let bea_phone = Phone::new("photo-bea", &url);
    let ada = ada_phone.start().await;
    let bea = bea_phone.start().await;
    ada.create_account("Ada Lund").await.unwrap();
    bea.create_account("Bea Holm").await.unwrap();
    let chat_id = make_contacts(&ada, &bea).await;

    // Ada sets a photo straight from her camera.
    let photo = camera_photo();
    assert!(contains(&photo, b"GPS"));
    let mut ada_events = ada.subscribe();
    let account = ada.set_profile_photo(&photo).await.unwrap();
    assert!(is_avatar(account.photo.as_deref()));
    assert_eq!(
        next_event(&mut ada_events).await,
        CoreEvent::AccountChanged {
            account: account.clone()
        }
    );

    // What reached the media server is a 512-pixel square without the metadata.
    let blobs = media.blobs();
    assert_eq!(blobs.len(), 1);
    assert!(!contains(&blobs[0], b"Exif"));
    assert!(!contains(&blobs[0], b"GPS"));
    let uploaded = image::load_from_memory(&blobs[0]).unwrap();
    assert_eq!(uploaded.dimensions(), (512, 512));

    // Bea's core shows the photo once it fetches the profiles again.
    let mut bea_events = bea.subscribe();
    bea.refresh_contacts().await;
    let CoreEvent::ContactChanged { contact } = next_event(&mut bea_events).await else {
        panic!("expected Ada's new photo");
    };
    assert_eq!(contact.public_key, account.public_key);
    assert_eq!(contact.name, "Ada Lund");
    assert!(is_avatar(contact.photo.as_deref()));
    let chat = bea.snapshot().chats.into_iter().find(|c| c.id == chat_id);
    assert_eq!(chat.unwrap().contact, contact);

    // Both keep it across a restart, without downloading it again.
    ada.close().await.unwrap();
    let ada = ada_phone.start().await;
    assert_eq!(ada.snapshot().account.unwrap().photo, account.photo);
    bea.close().await.unwrap();
    let bea = bea_phone.start().await;
    let chat = bea.snapshot().chats.into_iter().find(|c| c.id == chat_id);
    assert_eq!(chat.unwrap().contact.photo, contact.photo);
    assert!(bea_phone.data_dir().join("avatars").is_dir());

    // Without the photo, Bea's core drops it too.
    let removed = ada.remove_profile_photo().await.unwrap();
    assert_eq!(removed.photo, None);
    let mut bea_events = bea.subscribe();
    bea.refresh_contacts().await;
    let CoreEvent::ContactChanged { contact } = next_event(&mut bea_events).await else {
        panic!("expected Ada without a photo");
    };
    assert_eq!(contact.photo, None);

    // What isn't a photo is refused, before anything is uploaded.
    let error = ada.set_profile_photo(b"not a photo").await.unwrap_err();
    assert_eq!(error.message, "This photo can't be used. Try another one.");
    assert_eq!(media.blobs().len(), 1);

    // Deleting the account deletes the avatars this device made.
    bea.delete_account().await.unwrap();
    assert!(!bea_phone.data_dir().join("avatars").exists());
    ada.close().await.unwrap();
    bea.close().await.unwrap();
}
