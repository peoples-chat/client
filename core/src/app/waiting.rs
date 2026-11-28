//! Messages that wait for a path (#46). When `send_message` finds no path, or only a radio that
//! can't take the text, the core keeps the message in its [`Outbox`](crate::outbox::Outbox) and
//! shows it as waiting, cancellable. When a path returns, the chat's waiting messages go in the
//! order they were written, by the rules of #45: the internet first, else radio. Text too long
//! for radio waits for the internet, while shorter messages after it go by radio.
//!
//! A message that goes gets MDK's id: `messageAdded` brings it under that id, and
//! `messageRemoved` takes away the waiting one.

use super::{Core, failed, now_ms};
use crate::interface::{CommandError, CoreEvent, DeliveryState, Direction, Message, Transport};
use crate::outbox::Queued;
use crate::radio::format;
use crate::transport::{Route, route};

impl Core {
    /// Keeps `text` for the chat `chat_id` until a path returns, and shows it as waiting.
    pub(super) fn queue_message(&self, chat_id: &str, text: &str) -> Result<Message, CommandError> {
        let queued = self
            .inner
            .outbox
            .add(chat_id, text, now_ms())
            .map_err(|error| failed("keep the message", error))?;
        let view = queued_view(&queued);
        let mut state = self.state();
        if let Some(chat) = state.chats.iter_mut().find(|chat| chat.id == chat_id) {
            chat.messages.push(view.clone());
        }
        self.emit(CoreEvent::MessageAdded {
            chat_id: chat_id.to_owned(),
            message: view.clone(),
        });
        Ok(view)
    }

    /// Cancels the message `message_id` in the chat `chat_id`, which waits for a path. Emits
    /// [`CoreEvent::MessageRemoved`]. A message that is on its way can't be cancelled.
    pub async fn cancel_message(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<(), CommandError> {
        self.account()?;
        let waiting = self
            .inner
            .outbox
            .in_chat(chat_id)
            .iter()
            .any(|queued| queued.id == message_id);
        // Taken, so that it can't go while it is cancelled.
        let taken = if waiting {
            self.inner
                .outbox
                .take(message_id)
                .map_err(|error| failed("cancel the message", error))?
        } else {
            None
        };
        if taken.is_some() {
            self.remove_message(chat_id, message_id);
            return Ok(());
        }
        let known = self
            .chat(chat_id)
            .is_some_and(|chat| chat.messages.iter().any(|message| message.id == message_id));
        Err(CommandError::new(if known {
            "This message is on its way already."
        } else {
            "This message no longer exists."
        }))
    }

    /// The chat's waiting messages, as its history shows them.
    pub(super) fn queued_messages(&self, chat_id: &str) -> Vec<Message> {
        self.inner
            .outbox
            .in_chat(chat_id)
            .iter()
            .map(queued_view)
            .collect()
    }

    /// Starts sending the chat's waiting messages, unless it has none or they are being sent.
    pub(super) fn send_waiting_soon(&self, chat_id: &str) {
        if !self.inner.outbox.has(chat_id) || !self.flushing().insert(chat_id.to_owned()) {
            return;
        }
        let core = self.clone();
        let chat_id = chat_id.to_owned();
        tokio::spawn(async move {
            core.send_waiting(&chat_id).await;
            core.flushing().remove(&chat_id);
        });
    }

    /// Sends the chat's waiting messages, oldest first, while there is a path.
    async fn send_waiting(&self, chat_id: &str) {
        let Ok(account) = self.account() else { return };
        let Some(chat) = self.chat(chat_id) else {
            return;
        };
        for queued in self.inner.outbox.in_chat(chat_id) {
            // A relay that is connected, not one that may be: after a start without the
            // internet, relays count as reachable for a while before they count as gone.
            let relays_up = self.reach().connected(chat_id);
            let radio = self.contact_radio_reach(&chat.contact.public_key);
            let by = route(false, relays_up, radio);
            let ready = match by {
                Route::Wait => return,
                Route::Internet => None,
                // Too long for radio: it waits for the internet, and the next may go.
                Route::Radio if !format::fits(&queued.text) => continue,
                Route::Radio => match self.radio_ready(chat_id, &queued.text).await {
                    Ok(ready) => Some(ready),
                    Err(error) => {
                        log::info!("waiting messages wait on: {}", error.message);
                        return;
                    }
                },
            };
            // Taken first, so that a cancel from now on finds it gone.
            let queued = match self.inner.outbox.take(&queued.id) {
                Ok(Some(queued)) => queued,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!("could not take a waiting message: {error}");
                    return;
                }
            };
            let sent = match ready {
                Some(ready) => {
                    self.send_by_radio(&account, chat_id, &queued.text, ready)
                        .await
                }
                None => self.send_by_internet(&account, chat_id, &queued.text).await,
            };
            match sent {
                Ok(_) => self.remove_message(chat_id, &queued.id),
                Err(error) => {
                    log::warn!("a waiting message couldn't go: {}", error.message);
                    if let Err(error) = self.inner.outbox.put_back(queued) {
                        log::warn!("could not put a waiting message back: {error}");
                    }
                    return;
                }
            }
        }
    }

    /// Takes the message out of the chat, and emits [`CoreEvent::MessageRemoved`] if it was
    /// there.
    fn remove_message(&self, chat_id: &str, message_id: &str) {
        let mut state = self.state();
        let Some(chat) = state.chats.iter_mut().find(|chat| chat.id == chat_id) else {
            return;
        };
        let before = chat.messages.len();
        chat.messages.retain(|message| message.id != message_id);
        if chat.messages.len() != before {
            self.emit(CoreEvent::MessageRemoved {
                chat_id: chat_id.to_owned(),
                message_id: message_id.to_owned(),
            });
        }
    }

    fn flushing(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<String>> {
        self.inner
            .flushing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A waiting message as an interface sees it.
fn queued_view(queued: &Queued) -> Message {
    Message {
        id: queued.id.clone(),
        direction: Direction::Out,
        text: queued.text.clone(),
        sent_at: queued.written_at,
        transport: Transport::Internet,
        delivery: Some(DeliveryState::Waiting),
        cancellable: true,
        edited: false,
        deleted: false,
        withdrawn: false,
        // Its time starts when it goes (#124).
        expires_at: None,
    }
}
