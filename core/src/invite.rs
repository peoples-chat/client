//! Invite links (#19): what the user's QR code and invite link carry, and which links
//! [`crate::Core::add_contact`] reads.
//!
//! Our links are NIP-21 `nostr:nprofile1…` URIs: the account's public key, plus a few of its
//! relays as hints for finding its relay lists and KeyPackage. The QR code encodes the same
//! string. White Noise's phone apps scan these codes too, and ignore the hints.
//!
//! We also read the links other Marmot apps hand out: White Noise's `marmot://profile/<npub>`,
//! and plain `npub`, `nprofile` or hex keys, with or without `nostr:`.

use marmot_app::AppError;
use nostr::nips::nip19::{FromBech32, Nip19Profile};

/// At most this many relays go into a link, which keeps the QR code easy to scan.
const MAX_RELAY_HINTS: usize = 3;
/// At most this many relay hints are read from a link that someone else made.
const MAX_RELAY_HINTS_READ: usize = 5;
/// Profile links of White Noise and other Marmot apps, matched without regard to case. The
/// key follows the prefix; a query (`?from=qr`) or fragment after it is ignored.
const PROFILE_LINK_PREFIXES: &[&str] = &["marmot://profile/", "whitenoise://profile/"];

/// What an invite link says: whom to invite, and where to look for them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The account's public key, in hex.
    pub public_key_hex: String,
    /// Relays where the account publishes, as the link suggests. May be empty.
    pub relays: Vec<String>,
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

/// Reads an invite link, a scanned QR code or a pasted key.
///
/// The text may be a secret key pasted by mistake, so it is never logged or echoed.
pub fn parse(text: &str) -> Result<Invite, InviteError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(InviteError::Empty);
    }
    let reference = without_profile_link(text);
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
        });
    }

    match marmot_app::account_id_hex_from_ref(reference) {
        Ok(public_key_hex) => Ok(Invite {
            public_key_hex,
            relays: Vec::new(),
        }),
        Err(_) if looks_like_a_name(reference) => Err(InviteError::Name),
        Err(_) => Err(InviteError::NotALink),
    }
}

/// The key in a `marmot://profile/…` link, or the text as it is.
fn without_profile_link(text: &str) -> &str {
    for prefix in PROFILE_LINK_PREFIXES {
        if let Some(rest) = strip_prefix_ignoring_case(text, prefix) {
            let end = rest.find(['?', '#']).unwrap_or(rest.len());
            return rest[..end].trim_end_matches('/');
        }
    }
    text
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
