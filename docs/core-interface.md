# The interface between the core and the web view

The web view sends commands and receives events. It never holds protocol logic or key material. This document is the
contract between the two sides (#16).

## Principles

- **The core owns storage and every key.** Account keys go to the OS key store through MDK. Only public keys cross the
  boundary, and to the interface they are opaque strings.
- **The core runs without a web view.**
  - The app shell starts the core when the app launches, whether or not a web view is open.
  - A web view attaches by listening for events and asking for a snapshot.
  - Background work, such as radio messages while the phone is locked, needs this.
- **The boundary is plain Rust.** `Core` (`core/src/app.rs`) offers async commands, a channel of events and a snapshot.
  - Tauri is one shell around it.
  - If Tauri falls short on phones, a native shell can wrap the same `Core`, as White Noise wraps MDK.

## How it works

1. The interface listens for the Tauri event `core-event`.
2. Then it asks for the snapshot (`core_snapshot`).
3. Every later change arrives as an event. Events that arrive before the snapshot are applied after it.

Consequences:

- Applying an event twice, or one that the snapshot already contains, changes nothing.
- A command resolves once the core has taken it. Its changes arrive as events too, also in the web view that sent it.
- A reloaded web view starts again at step 1.
- If the web view falls more than 64 events behind, it misses some. The shell logs that, and a reload recovers.

## Wire format

- **Commands** are Tauri commands. Arguments and results are JSON.
- **A failed command** rejects with `{ "message": "…" }`. The message is readable English that the interface shows as
  it is. Details go to the log.
- **Events** are the Tauri event `core-event`. The payload's `type` names the event.
- **Fields** are camelCase. Times are milliseconds since the Unix epoch.

`core/tests/fixtures/interface.json` holds one example of each event, a snapshot and an error. The Rust tests and the
interface's tests both read it, so the two sides cannot drift apart unnoticed.

## Commands

| Interface (`Core` in `src/core/types.ts`) | Tauri command | Result | Events | Status |
| --- | --- | --- | --- | --- |
| `load()` | `core_snapshot` | `Snapshot` | | Done (#16) |
| `createAccount(name)` | `create_account` | `Account` | `accountCreated` | Done (#16) |
| `deleteAccount()` | `delete_account` | | `accountDeleted` | Done (#16) |
| `addContact(inviteLink)` | `add_contact` | `Chat` | `chatAdded` | Done (#19) |
| `acceptInvitation(invitationId)` | `accept_invitation` | `Chat` | `invitationRemoved`, `chatAdded` | Done (#19) |
| `sendMessage(chatId, text)` | `send_message` | `Message` | `messageAdded`, then `deliveryChanged` | Done (#20) |
| `pairRadio` | | | `radioChanged` | Planned (#24) |

## Events

| `type` | Fields | When | Status |
| --- | --- | --- | --- |
| `accountCreated` | `account` | The account was created on this device | Done (#16) |
| `accountDeleted` | | The account and its data were removed from this device | Done (#16) |
| `chatAdded` | `chat` | A direct chat starts: the user added a contact or accepted an invitation | Done (#19) |
| `invitationReceived` | `invitation` | Someone who is not a contact invites the user | Done (#19) |
| `invitationRemoved` | `invitationId` | The invitation was accepted, or withdrawn by its sender | Done (#19) |
| `messageAdded` | `chatId`, `message` | A message arrived, or the user sent one. `chatId` can be an invitation's id | Done (#20) |
| `deliveryChanged` | `chatId`, `messageId`, `delivery` | An outgoing message's delivery changed | Done (#20) |
| `reachChanged` | `chatId`, `reach` | A chat's relays went away or came back | Done (#43) |
| `transportChanged` | | Whether the internet or the radio can carry messages now | Planned (#48) |
| `radioChanged` | | The paired radio's state | Planned (#24, #48) |

## Types

| Type | Fields |
| --- | --- |
| `Snapshot` | `account` (or `null`), `chats`, `invitations` |
| `Account` | `name`, `publicKey` (hex), `inviteLink` |
| `Contact` | `publicKey`, `name` |
| `Chat` | `id`, `contact`, `messages` (oldest first), `reach` (`internet`, `radio`, `unreachable`) |
| `Message` | `id`, `direction` (`in`, `out`), `text`, `sentAt`, `transport` (`internet`, `radio`), `delivery` |
| `Invitation` | `id`, `from`, `receivedAt`, `messages` |

- A message's `id` is stable: both phones know the message by it, across restarts. The core drops duplicates by it after
  an outage.
- `delivery` is set on outgoing messages only:
  - `waiting`: not sent yet, for example because there is no path.
  - `sent`: a relay acknowledged it.
  - `confirmed`: the other radio confirmed it. Radio only.
- `reach` says how the chat's messages can travel now; see [Reach](#reach).
- `inviteLink` is a `nostr:nprofile1…` URI; see [Contacts and invitations](#contacts-and-invitations).
- An invitation's `id` is the id of the chat that accepting it opens.

## Contacts and invitations

**The invite link** is a NIP-21 `nostr:nprofile1…` URI. It carries the account's public key, plus up to three relays
where the account publishes, as hints for finding it. The QR code encodes the same string. White Noise's phone apps
scan it too. [mdk.md](mdk.md#contacts-and-invitations) explains the choice.

**`addContact` reads** the forms Marmot apps hand out:

- our links, and `nprofile`, `npub` or hex keys, with or without `nostr:`;
- White Noise's `marmot://profile/<key>` (also `whitenoise://`), with anything after `?` or `#` ignored.

It refuses the following with a message to show. The text is never logged, since it may be a secret key pasted by
mistake.

| Input | Message |
| --- | --- |
| A secret key (`nsec…`) | "That is your secret key. Never share it." |
| A name or `name@domain` | "Finding people by name is not possible yet. Paste an invite link instead." |
| Anything else | "That is not an invite link." |
| The user's own link | "That is your own invite link." |

**Adding a contact:**

- **The chat opens at once**, with `chatAdded`. The other person gets an invitation in the background.
- **With a contact the user already has**, it returns their chat.
- **When that person has already invited the user**, it accepts the invitation instead, so two people who scan each
  other's codes end up in one chat.
- **When their account can't be found on the relays**, it says "Could not find this person's account. Try again
  later." The lookup can take up to 50 seconds.

**Receiving and accepting:**

- **Invitations arrive while the app runs**, and ones sent while it was closed arrive when it starts.
- **Accepting** turns the invitation into the chat with the same id: `invitationRemoved`, then `chatAdded`. The
  sender isn't told; they are in the chat already.
- **Contact names** come from the person's profile on the relays. If it can't be found, the name is a pseudonym
  derived from the key ("Brave Otter"), the same one White Noise shows. A name found later doesn't reach the interface
  yet.

**Not built yet:**

- deleting or blocking an invitation;
- group chats: the core shows direct chats only;
- opening an invite link tapped in another app. The link has to be pasted into New chat.

## Messages

**Sending** (`sendMessage`):

- The core stores the message before the command returns, and `messageAdded` follows at once.
- The message is `waiting` until a relay acknowledges it; `deliveryChanged` then makes it `sent`.
- While no relay can be reached, the core keeps the message and tries again, also after the app restarts.
- Sending the same text twice within a second makes two messages; the second goes out a second later.
- It refuses empty text ("Write a message first.") and an unknown chat ("This chat no longer exists.").

**Receiving.** Messages arrive while the app runs, and ones sent while it was closed arrive when it starts. A message
that comes before the user accepts an invitation is added to the invitation: `messageAdded` with the invitation's id.

**History.**

- The snapshot holds each chat's latest 200 messages, oldest first. Older ones aren't paged in yet.
- `sentAt` is the sender's clock, in whole seconds. After a restart, messages sent within the same second can come back
  in another order.

**Not built yet:**

- a failed state: a message that every relay refuses stays `waiting` (#48 shows delivery);
- editing, deleting and reactions. Messages from other apps that use them, and media or stickers, aren't shown.

## Reach

Each chat says how its messages can travel now (#43):

- `internet`: at least one of the chat's relays is connected.
- `radio`: no relay is, but the contact's radio has been heard. Comes with #44.
- `unreachable`: neither. Messages wait, and go out when a relay is back.

**When it changes** (`reachChanged`):

- A chat becomes `unreachable` once its relay connections have stayed closed for 10 seconds.
- It is back to `internet` as soon as one of them connects again. The core tries every 5 seconds.
- A drop shorter than that doesn't show, so a flapping connection doesn't make the chat flicker.
- When the app starts without a connection, its chats start as `internet` and become `unreachable` 10 seconds later.
- The core goes by the relays, not by the phone's online flag. [mdk.md](mdk.md#relay-reachability) has the details,
  including connections that go silent without closing.

**When the relays are back**, the messages that waited go out within a second. The other phone sometimes shows them
much later, up to several minutes in the tests (#78; [mdk.md](mdk.md#relay-reachability) explains why).

**Trying it without the internet.** To take the relays away while the phone keeps its network and Bluetooth, use the
relay on the Mac (below) and type `block` in its window; `unblock` brings it back. With public relays, turning off
Wi-Fi and mobile data works too. Airplane mode may also turn off Bluetooth, which the radio needs.

**What the chat shows** (#48):

- **The header:** "Reachable by radio" in the radio colour, or "Not reachable". On the internet it shows nothing extra.
- **A notice above the field** says where messages go: "Messages now go by radio", or "Messages wait until it is
  back". It opens Radio and privacy, which says what radio protects and what it doesn't.
- **Each radio message** is marked "Radio", in the radio colour, next to its time and delivery.
- **In radio mode, the field** takes short text only. A counter shows UTF-8 bytes against `RADIO_TEXT_LIMIT` (96, in
  `src/core/types.ts`), and Send stays off past it.

Only the mock reaches a chat by radio for now; the core's radio comes with #40, #44 and #45.

## Planned: radio and transport

These shapes are sketches. The issues settle them together with their screens.

- **`pairRadio` (#24).** Pairing takes several steps: find radios nearby, choose one, confirm a PIN. It may become a
  few commands, with events for radios found and for the pairing's progress.
- **`transportChanged` (#48).** For example `{ "transport": "internet" | "radio", "state": "available" |
  "unavailable" }`. Each chat's `reach` (#43) may already be enough.
- **`radioChanged` (#24, #48).** For example not paired, connecting, connected or disconnected, with the radio's name.

## Running the app with the real core

The app uses the real core since it covers the whole flow (#20): creating the account, adding contacts and chatting.

**The mock**

- The browser preview (`pnpm dev`) always uses the mock with demo data. Its demo has a chat reached by radio, to show
  the radio interface: what it sends there goes by radio, and the other radio confirms it.
- To see the demo data in the app, start it with `VITE_CORE=mock`, for example `VITE_CORE=mock pnpm tauri dev`. This
  works the same for `pnpm tauri ios dev` and `pnpm tauri android dev`.
- Settings says "demo data" only while the mock is in use.

**Relays**

- New accounts publish to three public relays ([mdk.md](mdk.md#relays)), so a test account's name and public key end
  up on them.
- To use other relays, set `TAURI_PEOPLES_CHAT_RELAYS` (comma-separated) when you build. The name starts with `TAURI`
  because the Tauri CLI passes only such variables on to the iOS and Android builds.
- To see what gets published, or to take the relay away, use the relay on the Mac:
  1. Run it with `cargo run -p peoples-chat-core --example local_relay`. It prints what the apps publish.
  2. Build the app with `TAURI_PEOPLES_CHAT_RELAYS=ws://127.0.0.1:7777`. Only debug builds accept it.
  3. On Android, also run `adb reverse tcp:7777 tcp:7777`.
  4. Type `block` and Enter to cut the apps off, and `unblock` to let them back in ([Reach](#reach)).
- MDK needs a relay to set up an account. The relay doesn't need to be reachable: publishing waits until it is.

**Keys and data**

- Keys are stored in the platform's key store under the service `chat.peoples.client`. On a Mac that is your login
  keychain. Where they live on phones, and how signing works: [mdk.md](mdk.md#the-account-key).
- To remove a test account, use Settings → Account → Delete account.
- Data lives in the app's data folder, under `core/`.

## Lifecycle

- The shell starts the core when the app launches (`src-tauri/src/bridge.rs`). A command that arrives earlier waits for
  it.
- If the core cannot start, the interface shows why, and the next command tries again.
- Suspending, resuming and running in the background come with the radio and push work.

## Where the code is

| File | What it holds |
| --- | --- |
| `core/src/interface.rs` | The types, with their JSON form |
| `core/src/app.rs` | `Core`: snapshot, commands, events |
| `src-tauri/src/bridge.rs` | The Tauri commands, and forwarding events to the web view |
| `src/core/types.ts` | The interface's copy of the types |
| `src/core/tauriCore.ts` | The interface's side of the bridge |
| `src/core/mockCore.ts` | The mock with demo data |
| `core/tests/fixtures/interface.json` | The JSON both sides are tested against |

## Changing the interface

1. Change `core/src/interface.rs` and `src/core/types.ts` together.
2. Add an example to `core/tests/fixtures/interface.json`.
   - A new event doesn't compile in `core/tests/core_interface.rs` until it has a name there.
   - The interface's test needs a check for it.
3. Update the tables above.
