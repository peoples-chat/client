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
mod messaging;
mod photo;
pub mod radio;
mod radio_store;
mod reach;

pub use app::{Core, MAX_ABOUT_CHARS, MAX_NAME_CHARS, MIN_EXPORT_PASSWORD_CHARS};
pub use messaging::{
    ChatItem, ChatItems, DEFAULT_RELAYS, DirectChat, DirectChats, ExistingAccount, InviteToken,
    Messaging, MessagingError, MessagingOptions, NewAccount, PROJECT_RELAYS, RadioAddressItem,
    RelayLink, RelayLinks, TextMessage,
};
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
