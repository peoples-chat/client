//! The boundary between the core and an interface (#16).
//!
//! An interface starts from a [`Snapshot`], sends commands to [`crate::Core`] and follows its
//! [`CoreEvent`]s. The app shell hands all of it to the web view as JSON, where
//! `src/core/types.ts` mirrors these types. `docs/core-interface.md` describes the contract, and
//! `core/tests/fixtures/interface.json` pins the JSON for both sides.
//!
//! No secret key appears here. Public keys and invite links are opaque strings to an interface.

use serde::{Deserialize, Serialize};

/// The user's account on this device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// The profile name.
    pub name: String,
    /// A few words about the user, from the profile. Empty when there are none (#93).
    #[serde(default)]
    pub about: String,
    /// The profile photo, small and square, as a `data:image/jpeg` URL (#94).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<String>,
    /// The account's public key, in hex.
    pub public_key: String,
    /// Link that other people open or scan to start a chat. The user's QR code encodes it. A
    /// NIP-21 `nostr:nprofile1…` URI: the public key and relay hints (#19).
    pub invite_link: String,
    /// The same as a web link, `https://peoples.chat/i#nprofile1…`, for sharing: tapped on a
    /// phone, it opens the app (#72).
    pub share_link: String,
}

/// Someone the user can chat with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub public_key: String,
    pub name: String,
    /// Their profile photo, small and square, as a `data:image/jpeg` URL (#94).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<String>,
    /// The meshes their radios run, from their radio address records (#34).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meshes: Vec<Mesh>,
}

/// A mesh network that radios run (#34).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mesh {
    MeshCore,
}

/// How a message travelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    Internet,
    Radio,
}

/// Delivery of an outgoing message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryState {
    /// Not sent yet, for example because there is no path. A radio message is `waiting` from
    /// when the radio sends it until the other radio confirms it.
    Waiting,
    /// The radio sent it, but the other radio didn't confirm it in time, also after a retry
    /// (#41). It still goes by internet once a relay can be reached. Radio only.
    TimedOut,
    /// A relay acknowledged it.
    Sent,
    /// The other radio confirmed it. Radio only.
    Confirmed,
}

/// Whether a message came in or went out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    In,
    Out,
}

/// One message in a chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Stable identifier: MDK's id for the message, the same on both phones and across restarts
    /// (#20). The core drops duplicates by it after an outage.
    pub id: String,
    pub direction: Direction,
    pub text: String,
    /// Milliseconds since the Unix epoch.
    pub sent_at: u64,
    pub transport: Transport,
    /// Outgoing messages only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryState>,
    /// The message waits for a path, and the user can still cancel it (#46). Its id is
    /// provisional: once it goes, `messageRemoved` takes it away and `messageAdded` brings it
    /// with MDK's id.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancellable: bool,
    /// When the message disappears, in milliseconds since the Unix epoch, for a message sent
    /// while the chat's messages disappear (#124). It counts from the sender's clock. The core
    /// takes the message away then with `messageRemoved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// How a chat's messages can travel now (#43).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    /// At least one of the chat's relays is connected.
    #[default]
    Internet,
    /// No relay is reachable, but the contact's radio has been heard (#44).
    Radio,
    /// Neither path is available. Messages wait.
    Unreachable,
}

/// Why a chat can't be reached by radio, while no relay can reach it either (#44).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RadioIssue {
    /// The user has no radio, or its link is down.
    NoRadio,
    /// The contact hasn't shared a radio.
    NoContactRadio,
    /// The contact's radio runs another mesh.
    OtherMesh,
    /// The contact's radio uses another region or preset.
    OtherPreset,
    /// The contact's radio hasn't been heard lately.
    NotHeard,
    /// The user's radio has used the airtime the app allows it for this hour (#51).
    AirtimeUsed,
}

/// A direct chat with one contact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub contact: Contact,
    /// Oldest first.
    pub messages: Vec<Message>,
    #[serde(default)]
    pub reach: Reach,
    /// Why the chat isn't reachable by radio, when it is `unreachable` (#44).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio_issue: Option<RadioIssue>,
    /// The user started the chat, and the invitation hasn't reached the contact yet: they don't
    /// have the chat (#126). The core keeps sending it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undelivered: Option<UndeliveredInvitation>,
    /// After how many seconds new messages in the chat disappear, when someone in it turned
    /// disappearing messages on (#124). Absent when they are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disappear_after: Option<u64>,
}

/// An invitation to a chat that the core couldn't deliver to the contact yet (#126).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndeliveredInvitation {
    /// When delivery first failed, in milliseconds since the Unix epoch.
    pub since: u64,
    /// Why, when MDK knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UndeliveredReason>,
}

/// Why an invitation couldn't be delivered (#126).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UndeliveredReason {
    /// A relay refused it as too large.
    TooLarge,
}

/// One of the relays the account's messages travel through (#88).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relay {
    /// The relay's address, such as `wss://nos.lol`.
    pub url: String,
    pub operator: RelayOperator,
    pub status: RelayStatus,
}

/// Who runs a relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelayOperator {
    /// The project runs it.
    PeoplesChat,
    /// Anyone else.
    Independent,
}

/// Whether the account's connection to a relay is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelayStatus {
    Connected,
    /// Not connected, but not long enough to count as gone: opening, or a short drop.
    Connecting,
    /// Closed for long enough to count as gone (#43).
    Unreachable,
}

/// An invitation to a direct chat from someone who is not a contact yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    /// The id of the chat that accepting the invitation opens.
    pub id: String,
    pub from: Contact,
    /// Milliseconds since the Unix epoch.
    pub received_at: u64,
    /// Messages that came with the invitation, shown before the user accepts.
    pub messages: Vec<Message>,
}

/// An invitation the user sent: a personal link that works once and for 7 days (#95). The
/// person who uses it lands in a chat with the user without a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentInvite {
    /// Identifies the invitation.
    pub id: String,
    /// Whom it is for. Only the user sees it.
    pub name: String,
    /// The link to share: `https://peoples.chat/i#nprofile1…/<token>`.
    pub link: String,
    /// Milliseconds since the Unix epoch.
    pub created_at: u64,
    pub expires_at: u64,
    /// The chat that opened when the person joined, once they have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joined_chat_id: Option<String>,
}

/// The radio, as an interface shows it (#40).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioStatus {
    /// The radio the core uses: the one paired (#24), or one a test handed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired: Option<PairedRadio>,
    /// Every message goes by radio: a switch for testing, until the core chooses the transport
    /// by itself (#45).
    #[serde(default)]
    pub forced: bool,
    /// This device can pair a radio: over USB on computers (#24). Phones pair over Bluetooth
    /// later (#23).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_pair: bool,
}

/// The radio the core uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedRadio {
    /// The name the radio announces.
    pub name: String,
    /// The radio's public key, in hex: its address on the mesh.
    pub public_key: String,
    /// Whether the link to the radio is open (#24).
    pub connection: RadioConnection,
    /// Where the radio is plugged in, for one paired over USB (#24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The radio's LoRa settings, as it reported them last (#24).
    pub preset: RadioPreset,
}

/// Whether the link to the paired radio is open (#24).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RadioConnection {
    Connected,
    /// The core is opening it again: at start, or after the radio restarted or was unplugged.
    Connecting,
    Disconnected,
}

/// A radio's LoRa settings: its region and preset. Radios hear each other only with the same
/// ones (#24).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioPreset {
    pub frequency_khz: u32,
    pub bandwidth_hz: u32,
    pub spreading_factor: u8,
    /// The coding rate, as the 8 of 4/8.
    pub coding_rate: u8,
    /// MeshCore's EU/UK (Narrow) preset, which the app expects: 869.618 MHz, 62.5 kHz,
    /// spreading factor 8, coding rate 4/8. Otherwise an interface warns that the radio reaches
    /// only radios with its own settings.
    pub expected: bool,
}

/// A port a radio may be plugged into, for pairing (#24).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioPort {
    /// `/dev/cu.usbmodem1101` on a Mac, `/dev/ttyACM0` on Linux, `COM3` on Windows.
    pub path: String,
    /// What the device on it says it is, such as its USB product name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Everything an interface shows, as the core holds it now. Every later change arrives as a
/// [`CoreEvent`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub account: Option<Account>,
    pub chats: Vec<Chat>,
    pub invitations: Vec<Invitation>,
    /// The account's relays: the project's first, then the others, each by address. Empty
    /// without an account.
    #[serde(default)]
    pub relays: Vec<Relay>,
    /// The invitations the user sent, oldest first (#95).
    #[serde(default)]
    pub invites: Vec<SentInvite>,
    #[serde(default)]
    pub radio: RadioStatus,
}

/// A change in the core. Applying an event twice, or one that a snapshot already contains,
/// changes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CoreEvent {
    AccountCreated {
        account: Account,
    },
    AccountDeleted,
    ChatAdded {
        chat: Chat,
    },
    InvitationReceived {
        invitation: Invitation,
    },
    InvitationRemoved {
        invitation_id: String,
    },
    /// A received message, or one of the user's own sent messages. `chat_id` can also be an
    /// invitation's id: messages that arrive before the user accepts it.
    MessageAdded {
        chat_id: String,
        message: Message,
    },
    DeliveryChanged {
        chat_id: String,
        message_id: String,
        delivery: DeliveryState,
    },
    /// A message is gone: the user cancelled it while it waited, or it went, and came back
    /// under MDK's id (#46), or it disappeared once its time was up (#124).
    MessageRemoved {
        chat_id: String,
        message_id: String,
    },
    /// How a chat's messages can travel changed: its relays went away or came back, or the
    /// contact's radio was heard or not any more (#44).
    ReachChanged {
        chat_id: String,
        reach: Reach,
        /// Why the chat isn't reachable by radio, when it is `unreachable`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radio_issue: Option<RadioIssue>,
    },
    /// The invitation to a chat the user started couldn't be delivered, or now has been (#126).
    /// Without `undelivered`, the contact has it.
    InvitationDeliveryChanged {
        chat_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undelivered: Option<UndeliveredInvitation>,
    },
    /// Someone in the chat turned disappearing messages on or off, or changed after how long
    /// they disappear (#124). Without `disappear_after`, they are off.
    DisappearingChanged {
        chat_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disappear_after: Option<u64>,
    },
    /// The account's relays changed, or their connections did. Carries all of them.
    RelaysChanged {
        relays: Vec<Relay>,
    },
    /// The user changed their profile (#93).
    AccountChanged {
        account: Account,
    },
    /// A contact's profile changed, for example their name. Applies to their chats and
    /// invitations alike (#93).
    ContactChanged {
        contact: Contact,
    },
    /// The user sent an invitation, or the person used it (#95).
    InviteChanged {
        invite: SentInvite,
    },
    /// The radio was paired, connected or disconnected, or the switch that forces it changed
    /// (#40).
    RadioChanged {
        radio: RadioStatus,
    },
}

/// Why a command failed, in words an interface shows as they are. Details go to the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct CommandError {
    pub message: String,
}

impl CommandError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
