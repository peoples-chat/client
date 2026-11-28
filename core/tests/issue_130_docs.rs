//! Issue #130: what the docs and code comments say about Marmot and White Noise.
//!
//! Each test reads the text as it is checked in, so a stale statement can't come back unnoticed.

const SPEC: &str = include_str!("../../docs/spec.md");
const CORE_INTERFACE: &str = include_str!("../../docs/core-interface.md");
const MDK: &str = include_str!("../../docs/mdk.md");
const MESSAGING: &str = include_str!("../src/messaging.rs");

/// The section of `text` that starts at the heading `heading` and ends at the next heading of any
/// level.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("no heading {heading:?}"));
    let body = &text[start + heading.len()..];
    let end = body.find("\n#").unwrap_or(body.len());
    &body[..end]
}

/// `text` with every run of whitespace, line breaks included, as one space.
fn flat(text: &str) -> String {
    text.split_whitespace()
        .map(|word| word.trim_start_matches("///"))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn issue_130_spec_does_not_call_radio_a_marmot_transport() {
    let upstream = flat(section(SPEC, "## Upstream work"));
    assert!(!upstream.contains("good fit"), "{upstream}");
    assert!(!upstream.contains("carries only part of the protocol"));
    assert!(!upstream.contains("| Radio transport |"));
    assert!(upstream.contains("is not a Marmot transport"));
    assert!(
        upstream.contains(
            "marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/transports/README.md#L5"
        )
    );
    assert!(upstream.contains("its own key schedule (#128)"));
}

#[test]
fn issue_130_spec_puts_the_radio_address_record_in_chat_state() {
    let record = flat(section(SPEC, "### Radio address record"));
    assert!(!record.contains("ordinary message"), "{record}");
    assert!(record.contains("app component"));
    assert!(record.contains("(#119)"));
}

#[test]
fn issue_130_invite_token_is_shown_by_white_noise() {
    let interface = flat(section(CORE_INTERFACE, "**How they work** (#95)"));
    let comment = flat(&MESSAGING[..MESSAGING.find("pub const INVITE_TOKEN_KIND").unwrap()]);
    for text in [&interface, &comment] {
        assert!(!text.contains("White Noise doesn't show it"), "{text}");
        assert!(text.contains("bubble of its raw content"));
        assert!(text.contains("\"Unsupported event\" card (#123)"));
    }
}

#[test]
fn issue_130_default_relays_do_not_claim_white_noise_finds_us() {
    let comment = flat(&MESSAGING[..MESSAGING.find("pub const DEFAULT_RELAYS").unwrap()]);
    assert!(!comment.contains("White Noise users can find accounts made here"));
    assert!(!comment.contains("White Noise searches for relay lists"));
    assert!(comment.contains("only on its own relays (#22)"));
}

#[test]
fn issue_130_mdk_names_white_noise_lookup_relays() {
    let relays = flat(section(MDK, "## Relays"));
    assert!(
        !relays.contains("White Noise's discovery relays are"),
        "{relays}"
    );
    assert!(relays.contains(
        "White Noise looks a contact up on its own two relays only, \
         `wss://relay.us.whitenoise.chat` and `wss://relay.eu.whitenoise.chat` (#22, #76)"
    ));
    assert!(relays.contains("for its own user's lists at sign-in, not for contacts"));
}
