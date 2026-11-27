//! Issue #128: the radio keys reuse Marmot's group-event secret, which Marmot reserves for the
//! outer encryption of kind-445 events. The decision to keep it for v0 only is written in
//! `docs/radio-format-v0.md`. These tests keep that section and the code in step, so that neither
//! changes without the other.

use peoples_chat_core::radio::RADIO_KEY_LABEL;

const FORMAT: &str = include_str!("../../docs/radio-format-v0.md");
const SPIKE: &str = include_str!("../../docs/spikes/37-radio-keys.md");
const MESSAGING: &str = include_str!("../src/messaging.rs");

/// The section of the format document that records the decision, up to the next section.
fn decision() -> &'static str {
    let start = FORMAT
        .find("\n## Where the keys come from\n")
        .expect("docs/radio-format-v0.md has no section \"Where the keys come from\"");
    let section = &FORMAT[start + 1..];
    let end = section[2..]
        .find("\n## ")
        .map_or(section.len(), |at| at + 2);
    &section[..end]
}

/// The text with every run of whitespace as one space, so a line break in a paragraph or a
/// doc comment doesn't hide a phrase.
fn unwrapped(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_format_document_records_the_decision() {
    let decision = unwrapped(decision());
    for phrase in [
        // The choice, and that it is for v0 only.
        "Decision (#128): v0 keeps the group-event secret",
        "can't be proposed to Marmot, or released, with it",
        // The rule it breaks, and what that costs.
        "It MUST NOT be reused for app payloads, media, stream records, or other feature keys",
        "SC-033",
        "read every radio packet of the chat in that epoch",
        // The other options, and where the target is tracked.
        "An exporter of our own",
        "`safe_export_secret`",
        "#58",
    ] {
        assert!(
            decision.contains(phrase),
            "\"Where the keys come from\" no longer says: {phrase}"
        );
    }
}

#[test]
fn the_decision_names_the_secret_and_the_label_the_core_uses() {
    let decision = decision();
    assert!(
        decision.contains(r#"MLS-Exporter("marmot", "group-event", 32)"#),
        "the decision doesn't name the group-event exporter"
    );
    let label = std::str::from_utf8(RADIO_KEY_LABEL).unwrap();
    assert!(
        decision.contains(&format!("info: \"{label}\"")),
        "the decision doesn't name the core's HKDF label, {label}"
    );
}

/// When the core moves to an exporter of its own, this fails until the decision is rewritten.
#[test]
fn the_core_reads_the_secret_the_decision_names() {
    assert!(
        MESSAGING.contains(r#"const GROUP_EVENT_EXPORTER: &str = "marmot/group-event";"#),
        "the core's exporter changed: update \"Where the keys come from\" and this test"
    );
    let reads = MESSAGING.matches(".exporter_secret(").count();
    assert_eq!(
        reads, 1,
        "the core reads an exporter secret in {reads} places: the decision names one"
    );
    assert!(unwrapped(MESSAGING).contains(
        ".exporter_secret(account_hex, &group_id, GROUP_EVENT_EXPORTER, EXPORTER_BYTES)"
    ));
}

#[test]
fn the_spike_no_longer_calls_the_secret_safe_to_build_on() {
    let spike = unwrapped(SPIKE);
    assert!(!spike.contains("Why the group-event secret is safe to build on"));
    assert!(spike.contains("The group-event secret is not ours to use"));
    assert!(spike.contains("radio-format-v0.md#where-the-keys-come-from"));
}
