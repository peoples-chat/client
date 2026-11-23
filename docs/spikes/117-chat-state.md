# Spike #117: the radio address record as chat state

**Result: works with MDK v0.18.0 as it is, and White Noise's MDK 0.17 accepts it without showing anything.** Tested 23
November 2026.

The radio address record (#34) travels today as an app event of kind 9911 in the direct chat. MDK gives that event a row
in the chat, and White Noise on Android shows it as a bubble of raw data. MDK also lets an app keep state of its own in a
chat: an MLS app component, in the group's `app_data_dictionary`. The record can live there instead:

- Only the chat's members can read it, as before. It never reaches a public profile.
- It is not a message. Neither app shows anything when it changes: no row, no unread count, no move in the chat list.
- Each person's record has a component of its own. MDK settles races between the two by itself.
- `core/tests/chat_state_spike.rs` drives MDK directly, with two accounts on a test relay. Its ignored test
  `against_another_client` was the check against White Noise's MDK.

## What MDK offers

These calls are public in `marmot-app` 0.18:

| Call | What it does |
| --- | --- |
| `runtime.group_app_component(account, group, id)` | Reads the component `id` as this device's copy of the chat has it. `None` when absent. |
| `runtime.update_app_component(account, group, id, data)` | Replaces it, by a commit. Admins only. |
| `runtime.subscribe()` | Events. `GroupEvent(EpochChanged)` for the chat tells the app to read again. |

**Limits:**

- Ids from `0xf000` up belong to apps; the protocol never allocates there.
- A component holds up to 4096 bytes. A group holds up to 32 app components and 8192 bytes of them in all.
- An id that the group marks as required can't be written. Nothing marks ours.

## What we saw

| Question | What we saw |
| --- | --- |
| Can both write? | Yes. MDK founds a direct chat with both people as admins (`initial_admins` in `start_direct_conversation`). Bea, who was invited, wrote hers. |
| Does the other side learn of it? | Yes, through `GroupEvent(EpochChanged)` for the chat, not `GroupStateUpdated`. The write returned in 0.13 s, and Bea's copy held the record 1.1 s after it began (three runs). |
| Any trace in the chats? | None, on either side. The same rows, unread count, chat-list order and last message as before. |
| Both at once, online? | MDK fetches what the relay has before it sends, so the second change went after the first. Both records stayed (four runs). |
| Both at once, offline? | A true race: both changes were made in the same epoch. When the relay came back, MDK rolled one back and sent it again by itself ("reissued"), and both records stayed. The apps wrote nothing more (three runs). |
| One component for both? | The same race lost a record. Both changes replaced the same component, so MDK dropped one ("conflict"), as it documents (two runs). Hence one component each. |
| After a restart? | The records stay. They are part of the chat's MLS state, which MDK stores. |
| Offline? | The write returns at once, and the chat waits with the change in flight (`PendingPublish`), still showing the old record. MDK published it by itself 27 s after the relay came back (two runs). |
| Size? | A record is about 140 bytes. |

**Each write is one commit.** Every write started a new epoch on both sides, and the chat went on: messages sent after it
arrived as before. Writing the same record again sends nothing, because the app compares first.

## White Noise

White Noise's apps run MDK 0.17.0: MarmotKit 0.17.0 in both the iOS app (2026.11.16) and the Android app (2026.11.17), as
their repositories stood on 16 and 17 November. MDK 0.17's own command-line client, `wn`, stood in for them:

- **Setup:** a relay on this machine. `wn` had one change: MDK's neutral configuration instead of White Noise's, so that
  it used only that relay.
- **What our side did:** it started a chat with `wn`'s account, sent "Before the record", wrote the record (epoch 1 → 2),
  and sent "After the record".
- **What `wn` showed:** running as a daemon (`wnd`), a live MDK runtime like the apps, it reached epoch 2 and decrypted
  both messages. Its timeline held the two messages and nothing else. Its chat list showed "After the record" as the last
  message, with nothing unread.
- **One-shot mode doesn't apply commits.** `wn sync` fetched the commit but stayed in epoch 1: MDK holds an incoming
  commit for a settlement window, and only a live runtime comes back to apply it. That is a property of `wn`'s one-shot
  mode, not of the record.

**What MDK's code says, in both 0.17 and 0.18:**

- An app component the engine doesn't know is opaque, and accepted ("Unknown optional components remain opaque and are
  intentionally accepted").
- Only an admin may change one. OpenMLS checks only how the proposals are ordered and combined.
- A change to app state gets no system row: MDK writes rows only for members, admins, the name, the photo, disappearing
  messages and disbanding.

White Noise's unreleased iOS code shows event kinds it doesn't know as "Unsupported event" cards. A record that is no event
avoids those too.

## Costs and limits

1. **A change takes a commit.** It needs a relay, and it starts a new epoch, so the radio keys change (#37).
   - MDK already starts epochs on its own: a key update every 24 to 36 days, and one after joining. Record changes add
     one per pairing or radio change.
   - Rule 4 of #37 applies to these too. A contact who hasn't fetched the change when an outage starts can't read our
     radio messages until the internet is back: they go out under the newest key. A kind-9911 message never changes the
     epoch.
   - A fix in the radio layer would help with every commit, not only these: send the retry of an unconfirmed message
     under the previous epoch's key. The PoC doesn't need it.
2. **Admins only.** Chats that MDK 0.18 starts have two admins. In a chat another app founded with itself as the only
   admin, MDK refuses our write, and the app shares nothing there.
3. **Direct chats only.** The two-component rule needs exactly two people. Data that each member keeps for themselves
   belongs in their own MLS leaf (LeafNode app data), with no admin needed, but MDK offers that only to the protocol.
4. **Provisional ids.** `0xf910` and `0xf911` are in the app range, which the protocol never reaches. Another app could
   pick the same ids, so the record keeps its version field, and the app ignores what doesn't parse.

## Recommendation

Move the record into chat state in direct chats, as its own issue. The rules for the build:

1. **Components.** `0xf910` holds the record of the account whose public key (hex) sorts first, `0xf911` the other's. The
   data is the record as it is now (`core/src/radio/address.rs`).
2. **Writing.** Whenever the chat is settled and the app's own component differs from the current record, write it:
   after attaching a radio, when a chat starts, and at start. Never while a change is in flight: MDK sends a waiting
   change by itself.
3. **Reading.** Read the contact's component at start and on every `EpochChanged` for the chat.
4. **Kind 9911.** Stop sending it, and stop reading it. Only test phones have records as messages, and they share them
   again as state.
5. **Refused writes.** If MDK refuses the write because the app isn't an admin, share nothing in that chat.

**For Marmot (#56), if and when you want to reach out:** ask for member-owned app data in MLS leaves, which would also fit
groups, and say which app ids peoples.chat uses.

## Answers to the issue

- **Can both people write it?** Yes, in chats MDK 0.18 starts: both are admins.
- **Does the other side learn of a change, and how fast?** Yes, through `EpochChanged`, about a second later on a local
  relay.
- **Does a change leave any trace?** No row, unread count, chat-list move or notification, in our app or White Noise's
  MDK.
- **What happens when both write at once?** With a component each, MDK settles it by itself. With one for both, one record
  is lost.
- **Does it stay across a restart?** Yes.
- **What does White Noise do with it?** MDK 0.17 applies the change, keeps the chat working and shows nothing.
- **What else changes?** Each change is a commit: it needs a relay, and it starts an epoch, with new radio keys. Offline,
  MDK keeps the change and sends it later by itself. A record is about 140 bytes, well within the limits.
