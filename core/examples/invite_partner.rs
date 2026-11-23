//! An inviter on this machine, for trying personal invitations (#95) with an app in a simulator or
//! an emulator: it creates an account on a relay, makes an invitation, prints its link, and then
//! prints what happens, such as the invited person's chat opening without a request.
//!
//! The account is throwaway: its key stays in memory and its data in a temporary folder, and it
//! is deleted when the example stops (Enter, or after an hour). Use it with the relay of
//! `local_relay.rs`, so that nothing reaches a public relay.
//!
//! Usage: `cargo run -p peoples-chat-core --example invite_partner -- [relay] [name] [for whom]`
//! (`ws://127.0.0.1:7777`, `Emil Dahl` and `Sofie` by default).

use std::time::Duration;

use peoples_chat_core::interface::CoreEvent;
use peoples_chat_core::{Core, MessagingOptions};

/// As in the app (`src-tauri/src/lib.rs`): MDK can outgrow the default 2 MiB stack.
const WORKER_STACK: usize = 8 * 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_STACK)
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let relay = args
        .next()
        .unwrap_or_else(|| "ws://127.0.0.1:7777".to_owned());
    let name = args.next().unwrap_or_else(|| "Emil Dahl".to_owned());
    let for_whom = args.next().unwrap_or_else(|| "Sofie".to_owned());

    // Keys go to an in-memory keychain, never to the machine's own.
    keyring_core::set_default_store(keyring_core::mock::Store::new()?);
    let data = tempfile::tempdir()?;
    let core = Core::start(MessagingOptions {
        data_dir: data.path().to_path_buf(),
        relays: vec![relay.clone()],
        keychain_service: "chat.peoples.client.invite-partner".to_owned(),
        keys_in_file: false,
        allow_loopback: true,
        media_server: None,
        profile_copies: Vec::new(),
    })
    .await?;
    let mut events = core.subscribe();
    core.create_account(&name).await?;
    core.wait_until_published(Duration::from_secs(60)).await?;
    let invite = core.create_invite(&for_whom).await?;
    println!("{name} invites {for_whom} on {relay}:");
    println!("{}", invite.link);
    println!("press Enter to delete the account and stop");

    // A line on stdin, or its end, stops the example.
    let (enter, mut entered) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = enter.send(());
    });
    let stop = tokio::time::sleep(Duration::from_secs(60 * 60));
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = &mut entered => break,
            event = events.recv() => match event {
                Ok(CoreEvent::InvitationReceived { invitation }) => {
                    println!("request from {}", invitation.from.name)
                }
                Ok(CoreEvent::InvitationRemoved { .. }) => println!("the request was accepted"),
                Ok(CoreEvent::ChatAdded { chat }) => println!("chat with {} opened", chat.contact.name),
                Ok(CoreEvent::InviteChanged { invite }) if invite.joined_chat_id.is_some() => {
                    println!("{} used the invitation", invite.name)
                }
                Ok(CoreEvent::MessageAdded { message, .. }) => println!("message: {}", message.text),
                Ok(_) => {}
                Err(_) => break,
            },
        }
    }
    core.delete_account().await?;
    core.close().await?;
    println!("account deleted");
    Ok(())
}
