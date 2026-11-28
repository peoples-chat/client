//! Invite links (#19): what the user's QR code and invite link carry, and which links
//! [`crate::Core::add_contact`] reads.
//!
//! Our links are NIP-21 `nostr:nprofile1…` URIs: the account's public key, plus a few of its
//! relays as hints for finding its relay lists and KeyPackage. The QR code encodes the same
//! string. White Noise's phone apps scan these codes too, and ignore the hints.
//!
//! We also read the links other Marmot apps hand out: White Noise's `marmot://profile/<npub>`,
//! and plain `npub`, `nprofile` or hex keys, with or without `nostr:`.
//!
//! The link people share is a web link, `https://peoples.chat/i#nprofile1…` (#72). Tapped on a
//! phone with the app, it opens the app; without the app, it opens a page on peoples.chat. The
//! key is in the fragment, which browsers don't send, so the website never sees it. The QR code
//! keeps the `nostr:` form, which White Noise's scanners read.

use marmot_app::AppError;
use nostr::nips::nip19::{FromBech32, Nip19Profile};

/// At most this many relays go into a link, which keeps the QR code easy to scan.
const MAX_RELAY_HINTS: usize = 3;
/// At most this many relay hints are read from a link that someone else made.
const MAX_RELAY_HINTS_READ: usize = 5;
/// Profile links of White Noise and other Marmot apps, matched without regard to case. The
/// key follows the prefix; a query (`?from=qr`) or fragment after it is ignored.
const PROFILE_LINK_PREFIXES: &[&str] = &["marmot://profile/", "whitenoise://profile/"];
/// Our web links. The key follows the `#`. A personal invitation (#95) adds `/` and its token.
const WEB_LINK: &str = "https://peoples.chat/i#";
const WEB_LINK_PREFIXES: &[&str] = &[
    "https://peoples.chat/i#",
    "https://peoples.chat/i/#",
    "http://peoples.chat/i#",
    "http://peoples.chat/i/#",
    "peoples.chat/i#",
    "peoples.chat/i/#",
];

/// What an invite link says: whom to invite, and where to look for them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The account's public key, in hex.
    pub public_key_hex: String,
    /// Relays where the account publishes, as the link suggests. May be empty.
    pub relays: Vec<String>,
    /// The token of a personal invitation (#95): the inviter's app opens the chat without a
    /// request when it gets it back.
    pub token: Option<String>,
}

/// Why a text is not an invite link, in words an interface shows as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    #[error("Paste an invite link.")]
    Empty,
    #[error("That is your secret key. Never share it.")]
    SecretKey,
    #[error("Finding people by name is not possible yet. Paste an invite link instead.")]
    Name,
    #[error("That is not an invite link.")]
    NotALink,
}

/// The invite link of the account `public_key_hex`, carrying the first relays as hints.
pub fn invite_link(public_key_hex: &str, relays: &[String]) -> Result<String, AppError> {
    let hints = &relays[..relays.len().min(MAX_RELAY_HINTS)];
    let nprofile = marmot_app::nprofile_for_account_id(public_key_hex, hints)?;
    Ok(format!("nostr:{nprofile}"))
}

/// The account `public_key_hex` as a NIP-19 `npub1…`, the form White Noise takes (#129).
/// `None` if it is not a public key.
pub fn npub(public_key_hex: &str) -> Option<String> {
    use nostr::nips::nip19::ToBech32;
    let key = nostr::key::PublicKey::from_hex(public_key_hex).ok()?;
    key.to_bech32().ok()
}

/// The web link for sharing: the invite link's key and hints after `https://peoples.chat/i#`.
pub fn web_link(invite_link: &str) -> String {
    let reference = strip_prefix_ignoring_case(invite_link, "nostr:").unwrap_or(invite_link);
    format!("{WEB_LINK}{reference}")
}

/// A personal invitation's link (#95): the web link with the invitation's token after a `/`.
pub fn personal_link(invite_link: &str, token: &str) -> String {
    format!("{}/{token}", web_link(invite_link))
}

/// Reads an invite link, a scanned QR code or a pasted key.
///
/// The text may be a secret key pasted by mistake, so it is never logged or echoed.
pub fn parse(text: &str) -> Result<Invite, InviteError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(InviteError::Empty);
    }
    let (reference, token) = without_profile_link(text);
    let reference = strip_prefix_ignoring_case(reference, "nostr:").unwrap_or(reference);
    if marmot_app::is_nostr_secret(reference) {
        return Err(InviteError::SecretKey);
    }

    if strip_prefix_ignoring_case(reference, "nprofile1").is_some() {
        let profile = Nip19Profile::from_bech32(reference).map_err(|_| InviteError::NotALink)?;
        return Ok(Invite {
            public_key_hex: profile.public_key.to_hex(),
            relays: profile
                .relays
                .iter()
                .take(MAX_RELAY_HINTS_READ)
                .map(|relay| relay.as_str().to_owned())
                .collect(),
            token,
        });
    }

    match marmot_app::account_id_hex_from_ref(reference) {
        Ok(public_key_hex) => Ok(Invite {
            public_key_hex,
            relays: Vec::new(),
            token,
        }),
        Err(_) if looks_like_a_name(reference) => Err(InviteError::Name),
        Err(_) => Err(InviteError::NotALink),
    }
}

/// The key in a `marmot://profile/…` link or one of our web links, or the text as it is; and
/// a personal invitation's token, if the web link has one.
fn without_profile_link(text: &str) -> (&str, Option<String>) {
    for prefix in PROFILE_LINK_PREFIXES {
        if let Some(rest) = strip_prefix_ignoring_case(text, prefix) {
            let end = rest.find(['?', '#']).unwrap_or(rest.len());
            return (rest[..end].trim_end_matches('/'), None);
        }
    }
    for prefix in WEB_LINK_PREFIXES {
        if let Some(rest) = strip_prefix_ignoring_case(text, prefix) {
            let end = rest.find(['/', '?', '&']).unwrap_or(rest.len());
            let token = rest[end..]
                .strip_prefix('/')
                .map(|after| after.split(['/', '?', '&']).next().unwrap_or_default())
                .filter(|token| is_token(token))
                .map(str::to_ascii_lowercase);
            return (&rest[..end], token);
        }
    }
    (text, None)
}

/// What a token can look like: a few letters and digits. Anything else after the key is not a
/// token, and the link still works without it.
fn is_token(text: &str) -> bool {
    (8..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Text that is neither a link nor a key: a name, or a name@domain address.
fn looks_like_a_name(text: &str) -> bool {
    let key_like = ["npub1", "nprofile1"]
        .iter()
        .any(|prefix| strip_prefix_ignoring_case(text, prefix).is_some())
        || text.chars().all(|c| c.is_ascii_hexdigit());
    !key_like && !text.contains(':') && !text.contains('/')
}

fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::{Keys, ToBech32};

    /// A key and the matching NIP-19 forms.
    struct Key {
        hex: String,
        npub: String,
    }

    fn new_key() -> Key {
        let keys = Keys::generate();
        Key {
            hex: keys.public_key().to_hex(),
            npub: keys.public_key().to_bech32().unwrap(),
        }
    }

    fn relays(urls: &[&str]) -> Vec<String> {
        urls.iter().map(|url| (*url).to_owned()).collect()
    }

    #[test]
    fn our_link_carries_the_key_and_three_relays() {
        let key = new_key();
        let link = invite_link(
            &key.hex,
            &relays(&[
                "wss://one.example",
                "wss://two.example",
                "wss://three.example",
                "wss://four.example",
            ]),
        )
        .unwrap();
        assert!(link.starts_with("nostr:nprofile1"), "{link}");

        let invite = parse(&link).unwrap();
        assert_eq!(invite.public_key_hex, key.hex);
        assert_eq!(
            invite.relays,
            relays(&[
                "wss://one.example",
                "wss://two.example",
                "wss://three.example"
            ])
        );
    }

    #[test]
    fn our_web_link_carries_the_same_key_and_relays() {
        let key = new_key();
        let link = invite_link(
            &key.hex,
            &relays(&["wss://one.example", "wss://two.example"]),
        )
        .unwrap();
        let web = web_link(&link);
        assert_eq!(
            web,
            link.replace("nostr:", "https://peoples.chat/i#"),
            "{web}"
        );
        assert_eq!(parse(&web).unwrap(), parse(&link).unwrap());

        // As messengers and people pass it on, and with what #95 may add after the key.
        let nprofile = link.trim_start_matches("nostr:");
        for form in [
            format!("http://peoples.chat/i#{nprofile}"),
            format!("https://peoples.chat/i/#{nprofile}"),
            format!("HTTPS://PEOPLES.CHAT/i#{nprofile}"),
            format!("peoples.chat/i#{nprofile}"),
            format!(" https://peoples.chat/i#{nprofile}/7QK2M9 "),
            format!("https://peoples.chat/i#{}", key.npub),
        ] {
            let invite = parse(&form).unwrap_or_else(|error| panic!("{form}: {error}"));
            assert_eq!(invite.public_key_hex, key.hex, "{form}");
        }
        // A secret key in a web link is still recognised as one.
        let secret = Keys::generate().secret_key().to_bech32().unwrap();
        assert_eq!(
            parse(&format!("https://peoples.chat/i#{secret}")),
            Err(InviteError::SecretKey)
        );
        assert_eq!(parse("https://peoples.chat/i#"), Err(InviteError::NotALink));
    }

    #[test]
    fn reads_the_nip_19_example_profile() {
        // The nprofile example from NIP-19.
        let invite = parse("nprofile1qqsrhuxx8l9ex335q7he0f09aej04zpazpl0ne2cgukyawd24mayt8gpp4mhxue69uhhytnc9e3k7mgpz4mhxue69uhkg6nzv9ejuumpv34kytnrdaksjlyr9p").unwrap();
        assert_eq!(
            invite.public_key_hex,
            "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d"
        );
        assert_eq!(
            invite.relays,
            relays(&["wss://r.x.com", "wss://djbas.sadkb.com"])
        );
    }

    /// Issue #129: the npub shown on Profile, the form White Noise takes.
    #[test]
    fn issue_129_npub_is_the_nip_19_form_and_reads_back() {
        // The npub example from NIP-19.
        let hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let shown = npub(hex).unwrap();
        assert_eq!(
            shown,
            "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
        );
        assert_eq!(parse(&shown).unwrap().public_key_hex, hex);
        assert_eq!(
            parse(&format!("marmot://profile/{shown}"))
                .unwrap()
                .public_key_hex,
            hex
        );
        let key = new_key();
        assert_eq!(npub(&key.hex).unwrap(), key.npub);
        assert_eq!(npub("not a key"), None);
    }

    #[test]
    fn reads_keys_and_the_links_white_noise_shares() {
        let key = new_key();
        let nprofile = invite_link(&key.hex, &relays(&["wss://relay.example"]))
            .unwrap()
            .trim_start_matches("nostr:")
            .to_owned();
        let forms = [
            key.hex.clone(),
            key.hex.to_uppercase(),
            key.npub.clone(),
            format!("nostr:{}", key.npub),
            format!("NOSTR:{}", key.npub),
            nprofile.clone(),
            format!("  nostr:{nprofile}\n"),
            format!("marmot://profile/{}", key.npub),
            format!("marmot://profile/{}?from=qr", key.npub),
            format!("Marmot://Profile/{}/#share", key.npub),
            format!("marmot://profile/{nprofile}"),
            format!("whitenoise://profile/{}", key.npub),
        ];
        for form in forms {
            let invite = parse(&form).unwrap_or_else(|error| panic!("{form}: {error}"));
            assert_eq!(invite.public_key_hex, key.hex, "{form}");
        }
    }

    #[test]
    fn refuses_secret_keys_names_and_other_text() {
        let secret = Keys::generate().secret_key().to_bech32().unwrap();
        let secret_uri = format!("nostr:{secret}");
        let secret_link = format!("marmot://profile/{secret}");
        let cases: [(&str, InviteError); 12] = [
            ("", InviteError::Empty),
            ("   ", InviteError::Empty),
            (&secret, InviteError::SecretKey),
            (&secret_uri, InviteError::SecretKey),
            (&secret_link, InviteError::SecretKey),
            ("astrid@example.org", InviteError::Name),
            ("Astrid Holm", InviteError::Name),
            ("https://example.org/astrid", InviteError::NotALink),
            ("npub1notakey", InviteError::NotALink),
            ("nprofile1notakey", InviteError::NotALink),
            ("abc123", InviteError::NotALink),
            ("marmot://chat/abc123", InviteError::NotALink),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(text), Err(expected), "{text:?}");
        }
    }
}
