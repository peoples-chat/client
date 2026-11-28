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
| `updateProfile(name, about)` | `update_profile` | `Account` | `accountChanged` | Done (#93) |
| `setProfilePhoto(photo)` | `set_profile_photo` (raw body) | `Account` | `accountChanged` | Done (#94) |
| `removeProfilePhoto()` | `remove_profile_photo` | `Account` | `accountChanged` | Done (#94) |
| `deleteAccount()` | `delete_account` | | `accountDeleted` | Done (#16) |
| `addContact(inviteLink)` | `add_contact` | `Chat` | `chatAdded` | Done (#19) |
| `acceptInvitation(invitationId)` | `accept_invitation` | `Chat` | `invitationRemoved`, `chatAdded` | Done (#19) |
| `createInvite(name)` | `create_invite` | `SentInvite` | `inviteChanged` | Done (#95) |
| `previewInvite(inviteLink)` | `preview_invite` | `Contact` | | Done (#95) |
| `sendMessage(chatId, text)` | `send_message` | `Message` | `messageAdded`, then `deliveryChanged` | Done (#20; by radio #40, #45) |
| `cancelMessage(chatId, messageId)` | `cancel_message` | | `messageRemoved` | Done (#46) |
| `markRead(chatId)` | `mark_read` | | | Done (#124) |
| `setForceRadio(forced)` | `set_force_radio` | `RadioStatus` | `radioChanged` | Done (#40) |
| `exportAccountKey(copy)` | `export_account_key` | `string` | | Done (#86) |
| `radioPorts()` | `radio_ports` (computers only) | `RadioPort[]` | | Done (#24) |
| `pairRadio(port)` | `pair_radio` (computers only) | `RadioStatus` | `radioChanged` | Done (#24) |
| `forgetRadio()` | `forget_radio` (computers only) | `RadioStatus` | `radioChanged` | Done (#24) |

## Events

| `type` | Fields | When | Status |
| --- | --- | --- | --- |
| `accountCreated` | `account` | The account was created on this device | Done (#16) |
| `accountDeleted` | | The account and its data were removed from this device | Done (#16) |
| `chatAdded` | `chat` | A direct chat starts: the user added a contact or accepted an invitation | Done (#19) |
| `chatChanged` | `chat` | A chat stopped being direct, or ended: the contact left or deleted their account, it got a name or another member, or the user is no longer in it. The whole chat replaces the one with its id | Done (#125) |
| `invitationReceived` | `invitation` | Someone who is not a contact invites the user | Done (#19) |
| `invitationRemoved` | `invitationId` | The invitation was accepted, or withdrawn by its sender | Done (#19) |
| `messageAdded` | `chatId`, `message` | A message arrived, or the user sent one. `chatId` can be an invitation's id | Done (#20) |
| `deliveryChanged` | `chatId`, `messageId`, `delivery` | An outgoing message's delivery changed | Done (#20; `timedOut` #41) |
| `messageRemoved` | `chatId`, `messageId` | A waiting message was cancelled, or went and came back under the core's id, or a message disappeared. `chatId` can be an invitation's id | Done (#46, #124) |
| `disappearingChanged` | `chatId`, `disappearAfter` (optional) | Someone in the chat turned disappearing messages on or off, or changed the timer | Done (#124) |
| `reachChanged` | `chatId`, `reach`, `radioIssue` (optional) | A chat's relays went away or came back, or the contact's radio was heard or not any more | Done (#43, #44) |
| `relaysChanged` | `relays` | The account's relays changed, or their connections did. Carries all of them | Done (#88) |
| `accountChanged` | `account` | The user changed their profile | Done (#93) |
| `contactChanged` | `contact` | A contact's name, photo or radio changed | Done (#93, #94, #34) |
| `inviteChanged` | `invite` | The user sent an invitation, or the person used it | Done (#95) |
| `radioChanged` | `radio` | The radio was paired or forgotten, its link opened or dropped, or the switch that forces it changed | Done (#40, #24) |
| `transportChanged` | | Whether the internet or the radio can carry messages now | Planned (#48) |

## Types

| Type | Fields |
| --- | --- |
| `Snapshot` | `account` (or `null`), `chats`, `invitations`, `relays`, `invites`, `radio` |
| `Account` | `name`, `about`, `photo` (optional), `publicKey` (hex), `inviteLink`, `shareLink` |
| `Contact` | `publicKey`, `name`, `photo` (optional), `meshes` (optional: `meshCore`, from their radio address records, #34) |
| `Chat` | `id`, `contact`, `messages` (oldest first), `reach` (`internet`, `radio`, `unreachable`), `radioIssue` (optional), `disappearAfter` (optional, seconds, #124), `state` (optional: `contactLeft`, `group`, `removed`; left out while `direct`, #125) |
| `Message` | `id`, `direction` (`in`, `out`), `text`, `sentAt`, `transport` (`internet`, `radio`), `delivery`, `cancellable` (optional), `expiresAt` (optional, #124), `sender` (optional: `publicKey`, `name`; incoming messages in a `group` chat, #125) |
| `Invitation` | `id`, `from`, `receivedAt`, `messages` |
| `SentInvite` | `id`, `name`, `link`, `createdAt`, `expiresAt`, `joinedChatId` (once used) |
| `Relay` | `url`, `operator` (`peoplesChat`, `independent`), `status` (`connected`, `connecting`, `unreachable`) |
| `RadioStatus` | `paired` (optional: `name`, `publicKey`, `connection`, `port` (optional), `preset`), `forced`, `canPair` (optional) |
| `RadioPreset` | `frequencyKhz`, `bandwidthHz`, `spreadingFactor`, `codingRate`, `expected` |
| `RadioPort` | `path`, `description` (optional) |

- A message's `id` is stable: both phones know the message by it, across restarts. The core drops duplicates by it after
  an outage.
- `delivery` is set on outgoing messages only. It only moves forward, in this order:
  - `waiting`: not sent yet, for example because there is no path. A radio message is `waiting` from when the radio
    sends it until the other radio confirms it.
  - `timedOut`: the radio sent it, but the other radio didn't confirm it in time, also after a retry (#41). It still
    goes by internet once a relay is back. Radio only.
  - `sent`: a relay acknowledged it.
  - `confirmed`: the other radio confirmed it. Radio only.
- `reach` says how the chat's messages can travel now; see [Reach](#reach).
- `relays` are the account's relays; see [Relays](#relays).
- `inviteLink` is a `nostr:nprofile1…` URI; see [Contacts and invitations](#contacts-and-invitations).
- An invitation's `id` is the id of the chat that accepting it opens.

## Contacts and invitations

**The invite link** is a NIP-21 `nostr:nprofile1…` URI. It carries the account's public key, plus up to three relays
where the account publishes, as hints for finding it. The QR code encodes the same string. White Noise's phone apps
scan it too. [mdk.md](mdk.md#contacts-and-invitations) explains the choice. Shared, it travels as a web link that opens
the app ([Links that open the app](#links-that-open-the-app)).

**`addContact` reads** the forms Marmot apps hand out:

- our links, both forms, and `nprofile`, `npub` or hex keys, with or without `nostr:`;
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
  derived from the key ("Brave Otter"), the same one White Noise shows. A name found later, or a new one, arrives as
  `contactChanged` ([Profile](#profile)).

**When a chat stops being direct (#125):**

MDK counts a chat as direct while it has no name and two accounts. The core keeps a chat it has shown, by its id,
after that stops: `chatChanged` brings the whole chat with its new `state`, and it stays after a restart (the core
keeps which chats were direct, and with whom, in `chats.json`).

- **`contactLeft`**: the contact left the chat or deleted their account. The chat says so, and `sendMessage` refuses
  with "Bea Holm left this chat. Messages can't be sent here any more." Waiting messages stay until cancelled.
- **`removed`**: the user left the chat or was removed, in another app. Nothing can be sent.
- **`group`**: the chat got a name or a third member, in another app. Incoming messages carry `sender`, since anyone
  in it can have sent them. Messages go by internet only: radio keys are for the two people of a direct chat.
- Adding the contact again starts a new direct chat; the old one stays in the list.

**Not built yet:**

- deleting or blocking an invitation;
- group chats: the core shows direct chats, and chats that were direct (#125), only;
- opening an invite link tapped in another app on an iPhone: that needs the Apple developer account (#61). See [Links that
  open the app](#links-that-open-the-app).

## Profile

**Changing it** (`updateProfile`, #93):

- The name is required. Both fields are trimmed.
  - The name can have up to 64 characters (`MAX_NAME`) and about up to 280 (`MAX_ABOUT`), counted as characters,
    not bytes.
  - The same limit holds for `createAccount`.
- The core publishes the profile at once, and `accountChanged` follows. The photo, and anything other apps put in the
  profile, stay as they are.
- It also copies the profile to White Noise's relays, where White Noise looks up its users' contacts' profiles (#113).
  A build with relays of its own copies nothing.
- Without a connected relay nothing changes. The command fails with "Could not reach the relays. Try again when you
  are online."
- Sending the profile as it is changes nothing and publishes nothing.
- It refuses:
  - an empty name ("Enter a name.");
  - a name that is too long ("Your name can have up to 64 characters.");
  - an about text that is too long ("About can have up to 280 characters.").

**Contacts' profiles.** MDK doesn't follow other people's profiles. So the core fetches its contacts' profiles when it
starts following the account and every 5 minutes after. A new name updates the contact's chats and invitations through
`contactChanged`.

**What the interface shows.** Profile has Name and About. They save when the user leaves a field or presses Enter, and
say "Saved" or why not.

**Photos** (#94):

- `setProfilePhoto(photo)` takes the photo's bytes as the request's raw body, not as JSON.
- The core decodes the photo and turns it upright. It cuts a centred square, shrinks it to 512 pixels and saves it
  again as JPEG. Saving it again keeps the pixels only, so a phone photo's location doesn't travel with it; MDK would
  upload the file as it is.
- It uploads the photo to the media server, blossom.primal.net for the PoC, and publishes its address in the profile,
  copied to White Noise's relays like any profile change. Then `accountChanged` follows. Removing the photo takes the address out of the profile; the file stays on the server.
- Errors:
  - A file that isn't a photo, or is larger than 10 MB: "This photo can't be used. Try another one."
  - A failed upload: "Could not upload your photo. Try again."
  - Without a relay, the profile can't be published: the offline message above.
- `photo`, on the account and on contacts, is a small square JPEG as a `data:` URL. The core makes it, so the web view
  never contacts a media server. A contact's photo comes with their profile: when a chat starts, and with the refresh
  every 5 minutes. The core keeps each one in the data folder, and deletes them with the account.
- **The interface:** Profile's photo button opens the phone's picker, which offers the library and the camera. It hands
  the core the photo as JPEG, at most 1024 pixels on a side; drawing it first turns an iPhone's HEIC into JPEG. "Remove
  photo" appears once there is one. A photo chosen on Create profile is set once the account exists.

**Usernames are left out for now** (decided 22 November 2026). The design's Username, `emil@peoples.chat`, is a
Nostr address (NIP-05). It would need a name service at peoples.chat and rules for claiming names. People connect by
link and QR code instead. The name stays free text: not unique, and changeable.

## Links that open the app

**The shared link** is a web link, `shareLink`: `https://peoples.chat/i#nprofile1…`, the same key and hints as
`inviteLink` (#72). Profile's Share and Copy use it. The QR code keeps `inviteLink`, the `nostr:` form, which White
Noise's scanners read.

- **With the app,** tapping the link opens the app: Android App Links and iOS universal links, through Tauri's
  deep-link plugin. The app opens New chat with the link filled in, so the user confirms before a chat starts. This
  works whether the app was closed or running.
- **Before there is an account,** the link waits, and New chat opens with it once the account exists.
- **Without the app,** the link opens the page `peoples.chat/i` (the `peoples-chat/www` repository). It says how to
  get the app and shows the key to paste. The key comes after the `#`, which browsers don't send.
- **A phone opens the app only for a domain that vouches for it:**
  - Android: `peoples.chat/.well-known/assetlinks.json`, with the certificate the app is signed with.
  - iOS: `apple-app-site-association`, which names the app by its Apple team ID, so it waits for #61.
  - Until the site serves the Android file, an emulator can approve the domain by hand: `adb shell pm
    set-app-links-user-selection --user cur --package chat.peoples.client true peoples.chat`.
  - `adb shell am start -a android.intent.action.VIEW -d 'https://peoples.chat/i#…'` then opens a link as a tap would.
- **After the `#`, the key may be followed by `/` and a token:** a personal invitation's
  ([Personal invitations](#personal-invitations)).

## Personal invitations

**How they work** (#95). A personal invitation lands the person in a chat with the user, without a request, and needs
no server.

1. `createInvite(name)` makes one: `https://peoples.chat/i#nprofile1…/<token>`, with a random token. `name` is whom it
   is for, and only the user sees it. The core keeps it in `invites.json` in the data folder; nothing else knows of it.
2. Someone without an account opens the link. `previewInvite(link)` gives the inviter's name and photo from their
   profile; it needs no account.
3. They create an account, and `addContact(link)` starts the chat. Their core then sends the token in the chat, as an
   app event of the app's own kind (9910). peoples.chat doesn't show it. White Noise would show a kind it doesn't know,
   for Android as a bubble of its raw content and for iOS as an "Unsupported event" card (#123), but that does no harm
   here: the token goes only to the inviter, whose app is peoples.chat.
4. The inviter's core gets the token while the chat is still a request. If the token is the user's, unused and less
   than 7 days old, the core marks the invitation as used (`inviteChanged`, with `joinedChatId`) and accepts the chat
   (`invitationRemoved`, then `chatAdded`). A token that came while the app was closed counts when it starts.

**The limits are the inviter's app's.** A token that was used, has expired or isn't the user's opens nothing: that
chat stays an ordinary request. Anyone can still send a request with the key in the link, as with the QR code.

**What the interface shows:**

- **Bring your people** (Settings → Invite friends, or the chat list's banner): Invite one person, and Meet in person
  (the QR code on Profile). Bring a whole group waits for group chats.
- **Invite one person:** whom it is for, the message, and the link shared with it or shown as a QR code. The
  invitation is made when it is first shared or shown. Asking the person to be a guardian waits for recovery.
- **The chat list in the first days,** until there are five chats or while invitations are out: the Bring your people
  banner with how many have joined, and each invitation still out with Remind, which shares the link again. A chat from
  an invitation reads "Joined from your invite. Say hi." until the first message.
- **The invited person:** a link that opens the app without an account shows "Emil Dahl invited you". A name, and
  Start chatting, make the account and open the chat. "This invitation is not for me" goes to Welcome.

**Not built yet:** group invitations; guardians; the invited chat's suggested first messages and its "Emil invited you
to peoples.chat" header.

**Trying it:** `cargo run -p peoples-chat-core --example invite_partner` runs an inviter on the Mac against the local
relay. It prints an invitation's link and then what happens to it. A phone without an account opens the link: on the
Android emulator, `adb shell am start -a android.intent.action.VIEW -d '<link>'`.

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

**Disappearing messages** (#124, `core/src/app/expiry.rs`). peoples.chat doesn't offer the timer, but anyone in a
chat can set it: Marmot lets any admin ([message-retention-v1](https://github.com/marmot-protocol/marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/app-components/message-retention-v1.md#L68-L70)),
both people are admins in a chat MDK starts, and White Noise for Android offers it in direct chats.

- **The chat's timer** is `disappearAfter`, in seconds; `disappearingChanged` follows it. The chat says "Messages
  disappear 1 week after they are sent." under its header while it is on.
- **Each message sent while it is on** has `expiresAt`: the sender's clock when it was sent, plus the timer then. A
  later change to the timer doesn't change it. Expiry is advisory, since it counts from the sender's clock.
- **When its time is up**, the core takes the message away with `messageRemoved`, also from an invitation, and the
  snapshot no longer has it. A radio message loses its text in `radio.json` then; its id stays, for the replay check.
- **From disk.** MDK deletes expired messages only when asked (`sweep_expired_retention`), and holds back one the user
  hasn't read, or a whole chat while a message in it expired less than 5 seconds ago. The core asks soon after start, 6
  seconds after the latest expiry, after `markRead`, and every minute.
- **`markRead(chatId)`** says the user has seen the chat up to its newest message. The interface calls it while it
  shows the chat. MDK's read mark stays on this device: nothing is sent, so the other person isn't told.
- A message that waits for a path (#46) gets its time once it goes.

**Not built yet:**

- turning disappearing messages on from peoples.chat;
- a failed state: a message that every relay refuses stays `waiting` (#48 shows delivery);
- editing, deleting and reactions. Messages from other apps that use them, and media or stickers, aren't shown.

## Reach

Each chat says how its messages can travel now (#43):

- `internet`: at least one of the chat's relays is connected.
- `radio`: no relay is, but the contact's radio has been heard lately, on the mesh and preset of the user's radio (#44).
- `unreachable`: neither. Messages wait, and go out when a relay is back. `radioIssue` says why radio can't help:
  - `noRadio`: the user has no radio, or its link is down;
  - `noContactRadio`: the contact hasn't shared a radio;
  - `otherMesh` or `otherPreset`: the contact's radio runs another mesh, or another region or preset;
  - `notHeard`: the contact's radio hasn't been heard lately;
  - `airtimeUsed`: the user's radio has used the airtime the app allows it for this hour (#51; see
    [radio.md](radio.md)).

**Hearing a radio** (#44). The app sends nothing to find out who can be reached. A contact's radio counts as heard for
an hour (`HEARD_WINDOW` in `core/src/reach.rs`, provisional until hardware day) after:

- an advert it sends anyway;
- a message from it;
- its confirmation of one of the user's messages.

Heard times live in memory, so after a restart a contact counts as reachable by radio once their radio is heard again.

**When it changes** (`reachChanged`):

- A chat becomes `unreachable` once its relay connections have stayed closed for 10 seconds.
- It is back to `internet` as soon as one of them connects again. The core tries every 5 seconds.
- A drop shorter than that doesn't show, so a flapping connection doesn't make the chat flicker.
- When the app starts without a connection, its chats start as `internet` and become `unreachable` 10 seconds later.
- The core goes by the relays, not by the phone's online flag. [mdk.md](mdk.md#relay-reachability) has the details,
  including connections that go silent without closing.

**When the relays are back**, the messages that waited go out as soon as a relay is connected (#46). The other phone
showed them about 5 seconds after the relay's return in the tests. Messages MDK took just before an outage was noticed
can take much longer, up to several minutes in earlier tests (#78; [mdk.md](mdk.md#relay-reachability) explains why).

**Trying it without the internet.** To take the relays away while the phone keeps its network and Bluetooth, use the
relay on the Mac (below) and type `block` in its window; `unblock` brings it back. With public relays, turning off
Wi-Fi and mobile data works too. Airplane mode may also turn off Bluetooth, which the radio needs.

**What the chat shows** (#48):

- **The header:** "Reachable by radio" in the radio colour, or "Not reachable". On the internet it shows nothing extra.
- **A notice above the field** says where messages go: "Messages now go by radio", or "Messages wait until it is
  back", with the reason radio can't help, such as "Astrid's radio uses another preset." It opens Radio and privacy,
  which says what radio protects and what it doesn't.
- **Each radio message** is marked "Radio", in the radio colour, next to its time and delivery.
- **In radio mode, the field** counts UTF-8 bytes against `RADIO_TEXT_LIMIT` (96, in `src/core/types.ts`). Past it,
  the counter says "Waits for internet" and Send sends the message to wait (#46); with radio forced, Send stays off.
- **A waiting message** has Cancel under it, and "Too long for radio. It waits for the internet." when that's why.
- **Delivery** shows as a clock while a message waits, or waits for the other radio to confirm it; a check once a relay
  acknowledged it or the other radio confirmed it; and a question mark when the other radio didn't confirm it in time,
  with "Not confirmed by radio. It goes by internet once a relay is back." under it (#41). The labels for screen
  readers say which.

The core marks a chat `radio` when the contact's radio has been heard (#44), and sends there by radio by itself (#45).
The mock's demo has a chat reached by radio.

## Radio

The snapshot's `radio` says which radio the core uses, and `radioChanged` carries it whenever it changes (#40):

- `paired`: the radio the user paired (#24), or one the core's tests handed it:
  - its `name`, and its `publicKey`: its address on the mesh;
  - `connection`: `connected`; `connecting` while the core opens its link again, at start or after the radio restarted
    or was unplugged; `disconnected` otherwise, as before the shell gives the core its ports;
  - `port`: where it is plugged in, for a radio paired over USB;
  - `preset`: its LoRa settings as it reported them last, with frequency in kHz and bandwidth in Hz. `expected` says
    whether they are MeshCore's EU/UK (Narrow) preset (869.618 MHz, 62.5 kHz, spreading factor 8, coding rate 4/8);
    radios on other settings can't hear each other.
- `forced`: the switch that sends every message by radio, for testing until the core chooses the transport (#45). It
  stays across restarts. `setForceRadio(forced)` turns it on or off.
- `canPair`: the device can pair a radio. Computers pair over USB; phones will pair over Bluetooth (#23), so the
  phone builds don't have the three pairing commands.

**Pairing** (#24, `core/src/app/pairing.rs`). The shell gives the core the computer's USB serial ports at start:

- `radioPorts()` lists the ports a radio may be on: USB serial ports, each with its `path` and, where the device says,
  a `description` such as its product name.
- `pairRadio(port)` opens the radio there, reads its key, name and settings, remembers it in `radio.json`, and uses it
  in place of any radio before. Its key becomes the user's address on the mesh, shared with every contact (#34).
  Pairing the radio in use again changes nothing. It fails with "The device on /dev/cu.usbserial-0001 didn't answer. Is
  it a MeshCore radio with companion firmware for USB?", "Could not open /dev/cu.usbmodem1101. Another app may be using
  it." or "That port is gone. Plug the radio in and look again."
- **The core opens the paired radio again by itself:** at start, and when its link drops because the radio restarted
  or was unplugged. It looks on the radio's port, or on another with the same USB serial number, first after a second,
  then waiting up to 10 seconds between looks. Another radio on that port isn't taken for it.
- `forgetRadio()` closes the link and forgets the radio. Deleting the account forgets it too; `radioChanged` then
  follows `accountDeleted`, with `canPair` still set.
- **The core never changes the radio's settings,** such as its frequency or region. It only reads them.

**Choosing the path** (#45). `sendMessage` follows the spec's three rules:

1. the internet while one of the chat's relays is reachable;
2. otherwise radio, when the contact's radio has been heard on the mesh and preset of the user's radio (the chat's
   `reach` is `radio`);
3. otherwise the message waits (#46).

The switch sends by radio before any rule, for testing. `core/src/transport.rs` holds the rules, with a table of every
combination in its tests.

**Waiting messages** (#46). A message with no path, or too long for the only path (radio), waits in the core
(`core/src/outbox.rs`, `outbox.json`), not in MDK, so that it can still be cancelled:

- It is `waiting` and `cancellable`, under a provisional id. `cancelMessage` takes it away with `messageRemoved`; a
  message MDK has gives "This message is on its way already."
- It stays across a restart, in the order it was written.
- **When a path returns,** the chat's waiting messages go in that order, by the rules above: over the internet once a
  relay is connected, else by radio. One too long for radio keeps waiting for the internet, while shorter ones after it
  go by radio. Each comes back under the core's id with `messageAdded`, and `messageRemoved` takes the waiting one away.

**Sending by radio.** `sendMessage` checks first. With the switch on, it says why it can't send; chosen by the rules,
a message the radio can't take waits for the internet instead:

- "That is too long for radio. Keep it to 96 bytes."
- "Pair a radio first."
- "Your radio isn't connected.": it is paired, but the core is looking for it.
- "Bea Holm's radio isn't known yet.": the contact's app hasn't sent a radio address record (#34); see
  [radio.md](radio.md).
- "Could not send by radio: the chat's keys aren't ready. Try again."

Then MDK keeps the message as usual, and the radio carries it too. MDK publishes it once a relay can be reached, and
that copy is the permanent record (#47). With the switch on and the relays up, the message goes both ways at once; the
other phone keeps one copy. So does a message whose radio send is still going when the relays come back: it stays
`radio`, the way it went first. If the radio fails after the checks, the message waits for the internet.

**Confirmation** (#41). The message is `radio`, and `confirmed` once the other radio confirms it. Without a
confirmation in time, the core sends it once more, while the chat is still reachable by radio and the hour's airtime
allows. Without one after that either, it is `timedOut`, and stays so across a restart. A confirmation that comes
later still makes it `confirmed`, and a relay acknowledging its copy makes it `sent`. The details are in
[radio.md](radio.md#confirmation).

**What arrives.** On the other phone the message arrives as `radio` too, once, also when a retry got through as
well; its relay copy later changes nothing. Messages that came by radio stay across a restart, also before their
relay copies arrive. The radio parts are in [radio-format-v0.md](radio-format-v0.md#in-the-core).

**After the outage** (#47). MDK publishes each radio message once a relay is back, and the waiting messages go. Both
phones then have the same history: each message once, where it was, the radio ones still `radio`.
`core/tests/republish.rs` checks this in both directions, with messages waiting on both phones, and that the relay
copies are really in MDK. The other phone can take minutes to get what went over the relays after an outage (#78; see
[mdk.md](mdk.md#relay-reachability)).

**What the interface shows.** Settings → Radio shows:

- the paired radio, or "No radio paired": its name, "Connected by USB", "Looking for it. Check that it's plugged in."
  or "Not connected", its preset ("EU/UK (Narrow)", or the settings themselves) and its port;
- a warning for a radio on another preset: it reaches only radios with its own settings, and the app never changes
  them;
- "Pair a radio" ("Pair another radio" once one is), which opens Pair a radio: the USB ports, "Look again", and the
  core's error under them. Without `canPair` it is greyed out with "Over USB on a computer, for now";
- "Forget this radio", which asks once more;
- how many contacts have a MeshCore radio, under "Your contacts by mesh";
- the switch "Send everything by radio" under Testing. With it on, a chat on the internet says "Testing: every
  message goes by radio." above the field, which opens Radio, and the field takes short text as in radio mode.

The design has a Radio screen but no pairing screen, so Pair a radio follows the settings screens. The mock lists three
demo ports: a radio on the expected preset, one on another preset, and a device that isn't a radio.

## Relays

The snapshot lists the account's relays, and `relaysChanged` carries all of them whenever one changes (#88):

- `url`: the relay's address, such as `wss://nos.lol`.
- `operator`: `peoplesChat` for a relay the project runs, `independent` for any other. The project runs none yet (#11),
  so every relay is `independent` for now. The core's list of them is `PROJECT_RELAYS` in `core/src/messaging.rs`.
- `status`:
  - `connected`;
  - `connecting` while the connection opens, or during a drop shorter than 10 seconds;
  - `unreachable` once it has stayed closed for 10 seconds, the same wait as a chat's [reach](#reach).

The project's relays come first, then the others, each by address. Without an account the list is empty.

**What the interface shows.** Settings → Privacy → Relays and servers lists each relay's address without `wss://`, who
runs it and its status, with "Not reachable" in red. Blocking the relay on the Mac (below) shows it. Adding a relay, and
the media and notification servers, aren't built yet; the mock shows the design's three relays.

## Key export

`exportAccountKey` returns a copy of the account key for the user to keep (spec v0.2, Key export):

- `{ kind: "protected", password }`: a NIP-49 `ncryptsec1…` that other Nostr apps and signers open with the password.
  The password needs at least 12 characters (`MIN_EXPORT_PASSWORD`); a shorter one gives "Use a password of at least 12
  characters."
- `{ kind: "plain" }`: the `nsec1…` itself, as an explicit choice. From then on, protected copies record that the key
  was once shown plain.

The shell asks for the device unlock before every export, and "Unlock your device to export the key." means the person
didn't pass it. The interface keeps the result only while it shows it. The Export account key screen saves a protected
copy as a file through the share sheet, or shows it as a QR code; it shows a plain key as text. The mock returns copies
that look the same and open nowhere. MDK's side is in [mdk.md](mdk.md#the-account-key).

## Planned: radio and transport

These shapes are sketches. The issues settle them together with their screens.

- **Pairing over Bluetooth (#23).** On phones, pairing takes more steps: find radios nearby, choose one, confirm a PIN.
  It may need events for radios found and for the pairing's progress, next to the USB commands above.
- **`transportChanged` (#48).** For example `{ "transport": "internet" | "radio", "state": "available" |
  "unavailable" }`. Each chat's `reach` (#43) may already be enough.

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
- Profile photos go to blossom.primal.net. To keep them on the Mac too, run `cargo run -p peoples-chat-core --example
  local_media` and build with `TAURI_PEOPLES_CHAT_MEDIA_SERVER=http://127.0.0.1:7778` (on Android also `adb reverse
  tcp:7778 tcp:7778`). Only debug builds accept it.

**Keys and data**

- Keys are stored in the platform's key store under the service `chat.peoples.client`. On a Mac that is your login
  keychain. Development builds on a computer keep them in a file in the data folder instead, unless built with
  `TAURI_PEOPLES_CHAT_KEYCHAIN=1` (#115). Where they live on phones, and how signing works:
  [mdk.md](mdk.md#the-account-key).
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
