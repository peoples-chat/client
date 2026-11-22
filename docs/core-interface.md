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
| `addContact(inviteLink)` | | `Chat` | `chatAdded` | #19 |
| `acceptInvitation(invitationId)` | | `Chat` | `invitationRemoved`, `chatAdded` | #19 |
| `sendMessage(chatId, text)` | | `Message` | `messageAdded`, then `deliveryChanged` | #20 |
| `pairRadio` | | | `radioChanged` | Planned (#24) |

Until a command is done, the interface's bridge refuses it with "… is not available yet."

## Events

| `type` | Fields | When | Status |
| --- | --- | --- | --- |
| `accountCreated` | `account` | The account was created on this device | Done (#16) |
| `accountDeleted` | | The account and its data were removed from this device | Done (#16) |
| `chatAdded` | `chat` | A direct chat starts: the user added a contact or accepted an invitation | #19 |
| `invitationReceived` | `invitation` | Someone who is not a contact invites the user | #19 |
| `invitationRemoved` | `invitationId` | The invitation was accepted or deleted | #19 |
| `messageAdded` | `chatId`, `message` | A message arrived, or the user sent one | #20 |
| `deliveryChanged` | `chatId`, `messageId`, `delivery` | An outgoing message's delivery changed | #20 |
| `transportChanged` | | Whether the internet or the radio can carry messages now | Planned (#48) |
| `radioChanged` | | The paired radio's state | Planned (#24, #48) |

## Types

| Type | Fields |
| --- | --- |
| `Snapshot` | `account` (or `null`), `chats`, `invitations` |
| `Account` | `name`, `publicKey` (hex), `inviteLink` |
| `Contact` | `publicKey`, `name` |
| `Chat` | `id`, `contact`, `messages` (oldest first) |
| `Message` | `id`, `direction` (`in`, `out`), `text`, `sentAt`, `transport` (`internet`, `radio`), `delivery` |
| `Invitation` | `id`, `from`, `receivedAt`, `messages` |

- A message's `id` is stable; the core drops duplicates by it after an outage.
- `delivery` is set on outgoing messages only:
  - `waiting`: not sent yet, for example because there is no path.
  - `sent`: a relay acknowledged it.
  - `confirmed`: the other radio confirmed it. Radio only.
- `inviteLink` is `nostr:npub…` for now; #19 settles the link format.

## Planned: radio and transport

These shapes are sketches. The issues settle them together with their screens.

- **`pairRadio` (#24).** Pairing takes several steps: find radios nearby, choose one, confirm a PIN. It may become a
  few commands, with events for radios found and for the pairing's progress.
- **`transportChanged` (#48).** For example `{ "transport": "internet" | "radio", "state": "available" |
  "unavailable" }`.
- **`radioChanged` (#24, #48).** For example not paired, connecting, connected or disconnected, with the radio's name.

## Running the app with the real core

The interface uses the mock with demo data until the real core covers the whole flow (#20).

**Turning it on**

- Start any app command with `VITE_CORE=rust`, for example `VITE_CORE=rust pnpm tauri dev`. This works the same for
  `pnpm tauri ios dev` and `pnpm tauri android dev`.
- Settings says "demo data" only while the mock is in use.

**Relays**

- MDK needs a relay to set up an account. The relay doesn't need to be reachable: publishing waits until it is.
- Debug builds use `ws://127.0.0.1:7777`. To use other relays, set `PEOPLES_CHAT_RELAYS` (comma-separated) when you
  build.
- To see what gets published:
  1. Run the relay on the Mac with `cargo run -p peoples-chat-core --example local_relay`.
  2. On Android, also run `adb reverse tcp:7777 tcp:7777`.
- Release builds have no relays until #18 sets defaults, so creating an account says so.

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
