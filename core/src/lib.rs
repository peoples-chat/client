//! Protocol core of peoples.chat.
//!
//! All protocol logic lives here: identity, Marmot messaging, radio transports, transport
//! selection and recovery. The crate must not depend on Tauri or on any interface code, so
//! that the same core can run without a web view (background radio and push handling) and
//! can be wrapped by a native shell if needed.

mod app;
pub mod interface;
mod invite;
mod invites;
mod keys;
mod messaging;
mod outbox;
mod photo;
pub mod radio;
mod radio_store;
mod reach;
mod transport;

pub use app::{Core, MAX_ABOUT_CHARS, MAX_NAME_CHARS, MIN_EXPORT_PASSWORD_CHARS};
pub use messaging::{
    CHAT_MEDIA_SERVERS, CLIENT_NAME, ChatItem, ChatItems, DEFAULT_RELAYS, DirectChat, DirectChats,
    ExistingAccount, INVITE_TOKEN_KIND, InviteToken, Messaging, MessagingError, MessagingOptions,
    NewAccount, PROJECT_RELAYS, RADIO_ADDRESS_KIND, RadioAddressItem, RelayLink, RelayLinks,
    TextMessage, WHITE_NOISE_RELAYS, chat_media_servers, lookup_relays, profile_copy_relays,
};
pub use radio_store::Transmission;
pub use reach::DOWN_AFTER;

/// Version of the core, shown by the shell to prove that the core is linked in.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_the_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}
