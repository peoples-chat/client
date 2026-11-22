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
    /// The account's public key, in hex.
    pub public_key: String,
    /// Link that other people open or scan to start a chat. The user's QR code encodes it. A
    /// NIP-21 `nostr:nprofile1…` URI: the public key and relay hints (#19).
    pub invite_link: String,
}

/// Someone the user can chat with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub public_key: String,
    pub name: String,
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
    /// Not sent yet, for example because there is no path.
    Waiting,
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

/// Everything an interface shows, as the core holds it now. Every later change arrives as a
/// [`CoreEvent`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub account: Option<Account>,
    pub chats: Vec<Chat>,
    pub invitations: Vec<Invitation>,
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
    /// How a chat's messages can travel changed: its relays went away or came back.
    ReachChanged {
        chat_id: String,
        reach: Reach,
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
