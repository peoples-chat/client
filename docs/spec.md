# Resilient messenger: spec v0.2

Nov 22, 2026

## Summary

The app is an everyday encrypted messenger that keeps working over LoRa radio when the internet is gone, with one identity and one contact list for both.

It adds no new network or identity system. Its own additions are small: a compact radio message format and guardian recovery. It builds on:

- **Identity:** a Nostr key pair, created in the app.
- **Messaging:** the Marmot protocol (MLS encryption) over public Nostr relays.
- **Radio:** MeshCore first and Meshtastic second, through the user's own paired radio.
- **Recovery:** Marmot device linking and backups, plus key shares held by trusted contacts.

Decisions already made:

| Decision | Choice | Reason |
| --- | --- | --- |
| Base protocol | Nostr with Marmot | Portable identity ranks above reach |
| Radio scope | Text-only fallback | A MeshCore packet carries at most 184 bytes ([packet format](https://docs.meshcore.io/packet_format/)) |
| Lost identity | Recover the same key, never reissue | A Nostr key cannot be rotated or revoked |
| Recovery helpers | Family and friends, not servers | Keeps recovery client-only |
| Daily driver | Internet first, radio as silent fallback | People only keep an app installed if they use it |

Status: draft for discussion. Open decisions are listed in the last section.

## Goals and principles

The two core values are resilience and sovereignty. Every design choice below is tested against them.

**Goals**

- Match mainstream messengers for daily use over the internet: direct chats, groups, media, multiple devices.
- Keep text messaging alive over LoRa with the same people and groups, without the user doing anything.
- Build the radio contact list silently during daily use, so it exists before an outage.
- Let a user recover a lost identity with help from people they trust, with no company in the loop.
- Stay interoperable with other Marmot clients for internet messaging.

**Non-goals**

- Feature parity on radio. Calls, media and busy groups do not fit.
- A new protocol, relay network or mesh firmware.
- Custodial identity. No provider ever holds a user's whole key.
- Reissuing identities. The account key stays the same for life.

**Principles**

1. **Use what exists.** Prefer the Marmot spec, its development kit (MDK) and stock radio firmware over custom parts.
2. **Be honest on radio.** Show who is reachable and what will not send. Never imply internet-grade security or delivery.
3. **Be a good guest.** Community meshes are shared. The app must never flood them.
4. **Propose upstream.** Chat over radio and guardian recovery should become shared Marmot features, not private extensions.

## Core user flows

Five flows define the product. None of them should require technical knowledge.

**First launch**

1. The user opens the app and picks a name. The app creates the account key silently.
2. The app publishes relay lists and a key package, so that others can invite the user.
3. The app asks for a recovery passphrase and guardians. The user can postpone this, with reminders.

**Adding a contact**

1. The user scans a QR code, opens an invite link or looks up a name.
2. The app fetches the contact's relay list and key package, then sends an encrypted invitation.
3. The chat opens. If both people have paired radios, their apps exchange radio addresses inside the chat.

**Pairing a radio**

1. The user connects a MeshCore or Meshtastic radio over Bluetooth or USB.
2. The app adopts the radio's key, or creates one for a new radio, and keeps a copy in its backup.
3. The app shows the radio's existing mesh contacts and channels beside the user's other chats.
4. The app shares the radio address with the user's direct contacts and loads their addresses onto the radio.

**Losing the internet**

1. The app notices that relays are unreachable and marks each chat as internet, radio or unreachable.
2. Text to radio-reachable contacts goes out over LoRa. Everything else waits in a queue.
3. When the internet returns, the queue sends and both histories merge without duplicates.

**Losing the phone**

1. On a new phone, the user chooses Recover and reaches guardians in person, by call or over radio.
2. Each guardian confirms it is really the user, and their app releases a share.
3. With three shares and the passphrase, the app rebuilds the account key and the backup words.
4. The app signs in as a new device and restores history. Guardians re-invite the user to shared chats.

## Architecture

All protocol logic lives in one Rust core, which reaches people through existing internet services or through the user's paired radio.

&#91;embedded content: architecture · interface, shared core, two transports\]

The interface holds no protocol logic, so it can be replaced without touching the core. Everything below the core already exists. The project itself runs only a push server and, optionally, default relays and a media server.

## Identity and keys

A user's identity is one Nostr key pair. Every other secret exists to use it safely or to get it back.

| Secret | Created | Lives | Purpose | If lost |
| --- | --- | --- | --- | --- |
| Account key | By the app at first launch | Phone secure storage, or an external signer app | The identity. Signs key packages, invitations and device proofs | Rebuilt from guardian shares and passphrase |
| Device keys | By MDK, per device | That device only, never copied | Encrypt and sign everyday messages | Link a new device and remove the old one |
| Radio key | Adopted from the radio, or by the app | The radio, with a copy in the app backup | The user's address on the mesh | Load the copy onto a replacement radio |
| Recovery words | By MDK when backups are turned on | Shown once, then inside the guardian bundle | Open the encrypted history backups | Replace from any linked device |
| Recovery passphrase | Chosen by the user | Memory or paper, never stored | Unlocks the guardian bundle | Guardian recovery fails |
| Guardian shares | By the app | One on each of five contacts' devices | Any three rebuild the encrypted bundle | Re-split while the phone still works |

**Rules**

- The app reaches the account key only through a signer interface. This keeps external signers and threshold signing possible later without redesign.
- Device keys, not the account key, protect everyday messages. Radio use therefore never needs the account key.
- Marmot binds each device to the account with a signed proof ([Marmot spec](https://github.com/marmot-protocol/marmot)). The app adds nothing to this.
- Readable names such as name@domain are optional pointers to a key. Contacts verified by QR in person are marked as verified.
- The app prompts for the passphrase occasionally, so the user does not forget it.

**Key export**

- A user can export the account key at any time. It is for people who want to hold their identity themselves.
- The default export is password-protected and uses the Nostr standard for encrypted keys ([NIP-49](https://github.com/nostr-protocol/nips/blob/master/49.md)), as a file or a QR code. It opens in other Nostr apps and signers.
- A plain, unprotected key is a separate and explicit choice.
- Export needs the device unlock. The app warns that a copied key can never be changed or revoked.
- The exported key is the identity only. It can sign in on a new device, but chat history also needs the recovery words.
- A key held in an external signer is exported from that signer, not from the app.

## Internet messaging

Internet messaging is standard Marmot over Nostr relays, built on [MDK](https://github.com/marmot-protocol/mdk), so the app interoperates with other Marmot clients such as White Noise.

**How it works**

- There is no home server. Each user publishes signed relay lists, and each chat carries its own relay list.
- The app ships with default relays, which users can change. Invite links and QR codes carry relay hints.
- Group messages are published under throwaway keys, so relays cannot see who sent them.
- Media is encrypted on the device and stored on Blossom media servers.
- Notifications use Marmot's push design, which sends content-free wake-ups. Android can also hold a background connection instead.

**Services the project operates**

| Service | Needed | Software | Note |
| --- | --- | --- | --- |
| Push server | Yes, for iOS | [Transponder](https://github.com/marmot-protocol/transponder), open source | Requires the app's own Apple and Google credentials |
| Default relays | Recommended | Any Nostr relay | Public relays may refuse or drop messages |
| Default media server | Recommended | Any Blossom server | Stores encrypted files only |

None of these holds identities or readable content, and users can replace the relays and media server.

**Feature scope**

- **MVP:** direct chats, groups, media, replies, reactions, edits, disappearing messages, notifications.
- **Now that MDK has them:** linked devices and encrypted history backups. MDK 0.18.0 shipped both on 20 November 2026 ([release notes](https://github.com/marmot-protocol/mdk/blob/v0.18.0/docs/release/0.18.0.md)).
- **Later:** voice and video calls, and contact discovery beyond QR codes and links.

Interoperability with White Noise is a release test for every version.

## Radio fallback

Radio is a text-only fallback that reuses each chat's existing keys and members. It works with stock radio firmware and needs no internet at any step.

| Capability | Internet | Radio |
| --- | --- | --- |
| Text | Yes | Yes, short |
| Delivery confirmation | Yes | Yes |
| Reactions, location | Yes | Yes |
| Media, voice notes, calls | Yes | No |
| Edits and deletions | Yes | Queued until online |
| Typing and read indicators | Optional | Never |
| Adding or removing members | Yes | No |
| New contacts | Yes | In person, by QR code |

### Radios and pairing

- MeshCore is supported first. Meshtastic follows after the MVP. They cannot talk to each other, and a radio runs only one of them at a time, so each is a separate transport. A user who wants both pairs two radios.
- The app connects over Bluetooth or USB and uses the radio's documented app protocol ([MeshCore companion protocol](https://docs.meshcore.io/companion_protocol/)).
- The app is a complete client for the paired mesh. It shows mesh contacts and public channels, and plain mesh users appear as radio-only contacts.
- The app never changes frequency or region settings without asking.

### Radio address record

Each paired radio produces one small record: mesh type, radio public key, region and preset, and a timestamp. A user with two radios has two records.

- It lives in the state of the user's one-to-one chats, never of group chats, so MLS already authenticates it. Each person writes their own record into the chat's state as an MLS app component, so no client shows it as a message and it comes with the chat to a new device (#119). Until then it travels as an app event of the app's own kind, which other clients may show as an unknown event.
- It is shared with all of the user's direct contacts by default. A direct contact is someone the user has a one-to-one chat with, so people met only in groups never receive it. The user can exclude a contact or turn sharing off. It is never published, because radios announce their keys and sometimes their position over the air.
- It is re-sent when the radio or its settings change.

### Address book and radio cache

The phone holds every contact's radio address. The radio holds only a working set, because its memory is small: 120 nodes on most Meshtastic hardware ([Meshtastic docs](https://meshtastic.org/docs/about/overview/encryption/limitations/)) and a few hundred contacts on MeshCore ([meshcore-cli](https://pypi.org/project/meshcore-cli/1.3.4/)).

- The app keeps favourites and recent chats loaded and swaps others in when needed.
- A chat is marked reachable only when that contact's radio has actually been heard. The app does not probe.

### Mesh type and convergence

Every contact shows which mesh they can be reached on, so a community can see which one it is converging on.

- **Per contact:** a badge for MeshCore, Meshtastic or both, taken from the radio address records that contact has shared.
- **Per group:** a count of members on each mesh, among those who are also direct contacts.
- **Across all contacts:** a summary on the radio screen, split by mesh, region and preset. A different preset divides people as much as different firmware does.
- **When pairing a radio:** the app shows what the user's contacts use, before the user chooses or flashes firmware.

The counts are computed on the phone from records that contacts share. No server collects them, so each user sees the consensus of their own circle.

A contact on another mesh or preset is shown as not reachable by radio, with the reason. Later, a user with both radios could relay a group's radio messages between the two meshes.

### Message format on radio

- Messages use a compact binary format, not Nostr events. The target is 100 to 150 bytes of text per packet, to be measured in the proof of concept.
- Encryption keys are derived from the chat's current MLS state, so no extra key exchange is needed.
- Membership changes, invitations and key updates never travel by radio. Chats stay on the last state synced over the internet.
- Radio messages prove that the sender is a chat member, not which member. In a two-person chat that is equivalent.
- When the internet returns, the sender's app publishes the same messages to the relays. That copy is the permanent record, and receivers drop duplicates by message identifier.
- Group chats use radio only if the user turns it on for that group, because group traffic costs shared airtime. Radio groups come after the MVP.

### Choosing a transport

1. If relays are reachable, send over the internet.
2. If not, and the recipient's radio has been heard on a mesh and preset that one of the user's radios shares, send over that radio.
3. Otherwise queue the message and show it as waiting.

A message never goes out on both paths at once, and each message shows how it travelled.

### Mesh etiquette

- The app caps radio messages per user per hour, below the legal airtime limit of the region.
- It sends no automatic traffic: no typing indicators, read receipts, presence or reachability probes.
- It limits message length on radio and warns before sending to a large group.

## Recovery

Recovery always returns the same identity. Marmot covers devices and history, and this app adds recovery of the account key through guardians.

| Situation | Path | Result | Provided by |
| --- | --- | --- | --- |
| One device lost, another still linked | Link the new device from the old one, remove the lost one | Access continues | Marmot, in MDK since 0.18.0 ([release notes](https://github.com/marmot-protocol/mdk/blob/v0.18.0/docs/release/0.18.0.md)) |
| Every device lost | Guardian recovery, then restore from backup | Same identity, old history read-only, chats rejoined by re-invite | This app, plus Marmot backups |
| Radio lost | Load the saved radio key onto a new radio | Same mesh address | This app |
| Passphrase forgotten, phone still works | Choose a new passphrase and re-split | Fresh shares | This app |

### Guardian recovery

**Setup**

1. The app builds a bundle holding the account key and the Marmot recovery words.
2. It encrypts the bundle with a key derived from the passphrase. The app proposes six random words as the passphrase.
3. It splits the encrypted bundle into five shares, any three of which rebuild it. The user can choose other sizes.
4. It sends one share to each guardian inside their existing chat. Guardians accept the role and do not learn who the others are.

**Upkeep**

- The app checks silently each month that every guardian still holds a share.
- It asks the user to replace a guardian who has been unreachable for 60 days.
- It offers a printable sheet naming the guardians. The sheet holds no secrets.

**Recovery**

1. The new phone shows a short code and a temporary key.
2. Each guardian confirms the person by voice or in person, enters the code, and their app releases the share to that key.
3. A share fits a QR code or a single radio packet, so this works face to face or over the mesh.
4. With three shares and the passphrase, the app rebuilds the bundle and signs in as a new device.
5. MDK restores history with the recovery words. Guardians' apps offer to re-invite the user to shared chats.

### Limits

- **Theft is not covered.** If someone extracts the account key from a stolen phone, the identity is compromised for good.
- **Old shares stay valid.** A removed guardian keeps a usable share, so the passphrase is the real safeguard.
- **Collusion plus a weak passphrase breaks it.** Three guardians together can guess passphrases without limit, so the app enforces strength.
- **Rejoining chats needs other people.** Restored history is read-only, and Marmot plans automatic re-add requests later ([account-sync proposal](https://github.com/marmot-protocol/marmot/pull/437)).

Threshold signing with an always-on co-signer is the later answer to theft. The signer interface keeps that path open.

## Security and privacy

Radio protects message content as well as the internet path does, but it protects less of everything else. The app must say so plainly wherever radio is used.

| Property | Over the internet | Over radio |
| --- | --- | --- |
| Message content | End-to-end encrypted by MLS | End-to-end encrypted with keys derived from MLS state |
| Proof of sender | Individual device | Chat member only |
| Forward secrecy | Per message and per key update | Weaker: keys stay fixed until the next internet sync |
| Healing after a device compromise | At the next key update | Only once back online |
| Who sees that you communicate | Relays see encrypted events under throwaway keys | Anyone in range sees radio addresses and timing |
| Location exposure | Network address, to relays | A transmitter can be located by direction finding |

**Threats and responses**

| Threat | Response |
| --- | --- |
| Lost or stolen phone | Device lock and secure storage. Remove the device from the account. Key extraction remains unrecoverable |
| Hostile relay | It can drop or delay but not read. The app uses several relays at once |
| Radio eavesdropper | Content stays private. Metadata does not, and the app says so |
| Linking a person to a radio | Radio addresses go only to the user's direct contacts, inside encrypted chats, and are never published |
| Colluding guardians | Three-of-five threshold plus a strong passphrase |
| Impostor asking guardians for shares | Human check by voice or in person, plus a code bound to the new phone |
| Replayed or forged radio packets | Message identifiers, a replay window and member-level authentication |
| Flooding a community mesh | Hard send caps and no automatic traffic |
| A leaked key export | Password protection by default, and a device unlock before export. A leaked key stays unrecoverable |

**Not promised**

- Anonymity on radio.
- Protection after an attacker has copied the account key.
- Delivery guarantees on radio. Messages can be lost, and the app shows what was confirmed.

The radio message format and guardian recovery are new designs. Both need an independent security review before public release.

## Platform and stack

Recommendation: Tauri 2 with all protocol logic in a shared Rust core, and the web view used only for the interface. The deciding fact is that Marmot's engine is written in Rust.

| Criterion | Tauri 2 | Capacitor plus Electron |
| --- | --- | --- |
| Marmot engine (MDK, Rust) | Linked directly into the app core, through MDK's app runtime (`marmot-app`) | MDK's MarmotKit bindings (Swift, Kotlin) on mobile, and its C interface in a Node add-on on desktop |
| Radio libraries | Rust crates exist for MeshCore ([meshcore-rs](https://docs.rs/crate/meshcore-rs/latest/source/README.md)) and Meshtastic | JavaScript libraries exist for both |
| Desktop and mobile | One shell for all five platforms | Two shells |
| Bluetooth | Community plugin ([tauri-plugin-blec](https://docs.rs/crate/tauri-plugin-blec/latest)) | Community plugin, longer track record ([bluetooth-le](https://github.com/capacitor-community/bluetooth-le)) |
| Push notifications | Community plugins, with iOS gaps reported in some | Official plugin |
| Radio messages while the phone is locked | Rust core can handle them without the web view, to be verified | Needs native plugin code per platform |
| Desktop footprint | Small | Large |

**Why not a TypeScript core.** The TypeScript Marmot library has lagged the current protocol version ([marmot-ts issue](https://github.com/marmot-protocol/marmot-ts/issues/79)). A web-view core would also stop running when the phone suspends the app, which is exactly when radio messages arrive.

**Stack**

- **Rust core:** MDK for Marmot and relays, radio transports, transport selection, guardian recovery, the signer interface and encrypted storage. MDK comes from a pinned release tag on GitHub, because the `mdk-core` crate on crates.io is stale (0.8.0, May 2026).
- **Interface:** a web front end in the Tauri web view, built with React and TypeScript.
- **Native pieces:** an iOS notification extension that decrypts push wake-ups, an Android foreground service for the radio link, and the platform key stores.

**The hedge.** The Rust core exposes a narrow interface and holds every hard part. If Tauri falls short on mobile, native shells can wrap the same core, as White Noise does with MDK.

**Proof-of-concept tests that decide this**

1. A paired MeshCore radio delivers a message to a locked iPhone and a locked Android phone, and a notification appears.
2. The Tauri app exchanges messages with White Noise in both directions.
3. A push wake-up through the project's own notification server reaches the iOS app.

## Milestones

Work runs in three stages. The proof of concept shows one chat surviving an outage, the MVP makes the app usable every day, and wider reach follows.

&#91;embedded content: roadmap · 3 stages, 2 gates\]

The proof of concept is a thin slice, not a finished messenger. It is one direct chat between two phones with two radios, on the Danish MeshCore network around Aarhus. Its first step doubles as the Tauri go or no-go test. Each gate must pass before the next stage starts, and no dates are set yet. Recovery in the MVP builds on MDK's linked devices and backups, which shipped in MDK 0.18.0. The chat-over-radio proposal to Marmot goes out after the mesh gate.

## Upstream work

Three parts of this design are new and should be proposed to Marmot, so that other clients can adopt them. One may need a small addition to MeshCore.

| Proposal | To | Content | Timing |
| --- | --- | --- | --- |
| Chat over radio | Marmot, starting in its ideas folder | A feature that carries chat text over radio under keys of its own, not MLS messages. Invitations and membership changes stay on relays | After the proof of concept passes the mesh gate |
| Radio address record | Marmot | A shared format for the record in the chat's state, so any client can keep it even without radio support | During the proof of concept |
| Guardian recovery | Marmot, ideas folder | Share messages and the release flow, so a guardian can use any Marmot client | With recovery in the MVP |
| App payloads in direct messages | MeshCore | A binary payload type for direct messages. Channels gained one in firmware v1.15.0, with app data types registered with MeshCore ([pull request](https://github.com/meshcore-dev/MeshCore/pull/1928)); direct messages have none in v1.18.0 | Only if the proof of concept shows a need |

Marmot's ideas folder takes ideas, not specifications: an idea document defines no wire bytes and assigns no identifiers, such as component ids, event kinds or exporter labels ([ideas README](https://github.com/marmot-protocol/marmot/blob/master/ideas/README.md)). So the chat-over-radio and guardian recovery proposals describe behaviour there, and leave byte formats and identifiers to a change to the specification itself.

The radio proposal is not a Marmot transport. A transport spec "describe[s] how a network carries Marmot MLS bytes" ([transports README](https://github.com/marmot-protocol/marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/transports/README.md#L5)). The radio format carries no MLS message: it carries chat text under keys of its own. So it reads more like a feature with its own key schedule (#128) than a binding in the transports section.

Radio is a second path for the same chat: each radio message is also published to relays once the internet returns.

Until these proposals are accepted, guardians and radio peers must use this app. Internet chats interoperate from the start.

## Risks and open questions

The largest risks are dependencies that are still moving, not the design itself.

| Risk | Why it matters | Response |
| --- | --- | --- |
| Marmot and MDK are changing fast | The spec was replaced in July 2026, multi-device entered it in late October, and MDK shipped multi-device in November | Pin MDK versions, follow its change log, run interoperability tests every release |
| Tauri is unproven here on mobile | Bluetooth and background handling decide whether radio works on a locked phone | Proof-of-concept tests, with native shells as the fallback |
| MeshCore path for app data in direct messages is unclear | It decides packet size and routing cost | Settle in the proof of concept, propose upstream if needed |
| Mesh rules and settings are fragmenting | US communities are changing bandwidth after an FCC compliance dispute ([Hackaday](https://hackaday.com/2026/09/17/fcc-ism-rules-may-shatter-lora-mesh-communities/)) | Pluggable transports, presets carried in the address record, no silent setting changes |
| The app could overload shared meshes | One popular app with automatic fallback multiplies traffic | Hard caps, opt-in groups, a heads-up to mesh communities before the field test |
| Daily use is the precondition | Radio contacts only build up if people use the app every day | Interoperate with White Noise from day one, and lead with radio and recovery as the reasons to switch |
| A copied account key cannot be revoked | Theft ends the identity | Signer interface now, threshold signing later |
| App stores gate distribution | Review rules, push credentials and background Bluetooth approval | Direct Android downloads as a second channel |

**Open questions**

- [x] What is the product called?

  Decided: peoples.chat
- [x] Which region and radio preset are tested first?

  Decided: the Aarhus area, Denmark. The Danish MeshCore network uses the EU/UK (Narrow) preset, so that is tested first: 869.618 MHz, 62.5 kHz bandwidth, spreading factor 8 ([Meshguide Danmark](https://meshguide.dk/)).
- [x] Is the radio address shared with all contacts by default, or only chosen ones?

  Decided: all direct contacts by default, not group members the user has no one-to-one chat with. The user can exclude a contact or turn sharing off.
- [x] Which front-end framework?

  Decided: React with TypeScript.
- [ ] Is three of five the right default for guardians, with two of three for small circles?
- [ ] Does a radio group use one mesh channel, or separate direct messages to each member?
- [ ] Does the app keep two radios connected at once, or switch between them?
- [ ] Where is the copy of the radio key kept: in the guardian bundle, or encrypted on relays?
- [ ] Who runs the default relays, media server and push server, and how are they funded?
- [ ] Which licence, given the licences of MDK and the other dependencies?
- [ ] How does key export work if threshold signing later replaces the single account key?

## Sources

Pages opened for this draft, as of 21 November 2026:

- [Marmot protocol specification](https://github.com/marmot-protocol/marmot)
- [MeshCore packet format](https://docs.meshcore.io/packet_format/)
- [Meshtastic encryption limitations](https://meshtastic.org/docs/about/overview/encryption/limitations/)
- [Hackaday on FCC rules and LoRa meshes](https://hackaday.com/2026/09/17/fcc-ism-rules-may-shatter-lora-mesh-communities/)

Re-checked on 22 November 2026, for the corrections in the upstream work, recovery, stack and risks:

- Marmot's [transport checklist](https://github.com/marmot-protocol/marmot/blob/master/transports/README.md), its retired [QUIC binding](https://github.com/marmot-protocol/marmot/blob/master/transports/quic.md) and the [pull request that retired it](https://github.com/marmot-protocol/marmot/pull/513)
- Marmot's [ideas folder rules](https://github.com/marmot-protocol/marmot/blob/master/ideas/README.md)
- The [multi-device pull request](https://github.com/marmot-protocol/marmot/pull/442) in the Marmot spec, and [MDK's multi-device issue](https://github.com/marmot-protocol/mdk/issues/2628), still open although MDK 0.18.0 ships the feature
- The [MDK 0.18.0 release notes](https://github.com/marmot-protocol/mdk/blob/v0.18.0/docs/release/0.18.0.md), and [mdk-core on crates.io](https://crates.io/crates/mdk-core)
- MeshCore's [binary channel messages pull request](https://github.com/meshcore-dev/MeshCore/pull/1530), closed without merging, and the [group data pull request](https://github.com/meshcore-dev/MeshCore/pull/1928) that replaced it in firmware v1.15.0
- The [MeshCore companion firmware v1.18.0](https://github.com/meshcore-dev/MeshCore/tree/companion-v1.18.0) source, and [meshcore-rs](https://github.com/andrewdavidmackenzie/meshcore-rs): its 0.2.0 release has no channel data, and its main branch adds `send_channel_data()`

Re-checked on 27 November 2026, for the correction in the upstream work (#130):

- Marmot's [transports README](https://github.com/marmot-protocol/marmot/blob/36757fed3c5053aa3c194cc6c26f6985b344af51/transports/README.md#L5) at commit `36757fe`: a transport carries Marmot MLS bytes

Other links in the text point to pages found by search and read only in excerpt.
