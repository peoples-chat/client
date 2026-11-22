# Test #22: messages with White Noise in both directions

**Result: passes one way as shipped; passes both ways once White Noise can find our accounts.** Tested 22 November 2026.
This is input to the Tauri go/no-go gate (#33).

| Direction | Result |
| --- | --- |
| peoples.chat starts the chat with White Noise | **Pass.** Invitation, acceptance and text both ways |
| White Noise starts the chat with peoples.chat, as shipped | **Fail.** White Noise finds the account but can't start the chat: "We couldn't find a usable chat key" |
| The same, with our account's relay lists also on a White Noise relay | **Pass.** Invitation with a first message, acceptance and text both ways |

The failing direction comes down to where our accounts publish their relay lists, not to the protocol, MDK's version or
Tauri. See [Why White Noise couldn't start the chat](#why-white-noise-couldnt-start-the-chat).

## Setup

| | peoples.chat | White Noise |
| --- | --- | --- |
| Build | Debug, from this branch (`89edbfe`, with #73 and #74), real core on its default relays | Android 2026.11.17, version code 27, commit `045fd9be`. The arm64 APK from the GitHub release, checksum checked |
| MDK | v0.18.0 (`34646a57`) | MarmotKit 0.17.0 (MDK `4df811ae`) |
| Device | iPhone 17 simulator, iOS 26.5 | Pixel 10 Pro emulator, Android 17 (API 37) |
| Driven by | A page script through the app's screens | `uiautomator` taps and text input |

White Noise ran with usage sharing and diagnostic logs off, as its sign-up offered them.

## What happened

**peoples.chat to White Noise.**

1. White Noise's Share & Connect screen shows the QR code `marmot://profile/<npub>?from=qr`. Pasted into New chat,
   peoples.chat read it and opened a chat with "Wren Test", the name from White Noise's profile.
2. peoples.chat sent "Hello White Noise, this is Ada on peoples.chat.", and it reached "Sent".
3. White Noise listed the chat with that message as its preview and "Invitation pending". After Accept, it replied.
4. peoples.chat showed the reply.

**White Noise to peoples.chat.**

1. White Noise's New Message → Connect with QR → Scan QR Code read our `nostr:nprofile1…` code through the emulator's
   camera. It found the account and showed its name.
2. Starting the chat failed: "We couldn't find a usable chat key for Cleo Test. Ask them to install or update White
   Noise and open it, then retry." Retrying didn't help. Its log says `failure=missing_key_package`.
3. With a build whose default relays also included `wss://relay.us.whitenoise.chat` (a local, uncommitted change for
   this test), a new account's relay lists landed there too. White Noise then started the chat and sent "Hello Dana,
   Wren here on White Noise."
4. peoples.chat received the invitation with that message already in it. After Accept, it replied, and the reply
   reached "Sent" and showed in White Noise.

## Why White Noise couldn't start the chat

White Noise 2026.11.17 looks up a new contact's relay lists (kinds 10002 and 10050) only on its own two relays,
`wss://relay.us.whitenoise.chat` and `wss://relay.eu.whitenoise.chat`. MDK's directory searches the relays the app
configures, and White Noise configures only those two. Our accounts publish their relay lists to our default relays
alone, so White Noise found no relay list. It then looked for the KeyPackage on its own relays, found nothing, and
reported it missing.

How we know:

- **White Noise's relays held nothing** for our account, but every event of White Noise's own test account.
- **A KeyPackage format problem would show differently.** MDK reports it as an invalid KeyPackage, not a missing one,
  and the parsing code is identical in MDK 0.17.0 and 0.18.0.
- **Not MDK 0.18.0's new capability.** MDK 0.18.0 advertises one more app component in KeyPackages (`0x800e`, the
  device roster, for multi-device). An MDK 0.18.0 account without it failed the same way.
- **The remedy works.** An MDK 0.18.0 account that also published its relay lists to `wss://relay.us.whitenoise.chat`
  was found, and White Noise started the chat.

White Noise's main branch moved to MarmotKit 0.18.0, the same MDK commit we pin, on 22 November. Its relay
configuration is unchanged, so its next release will likely behave the same way.

What would fix it is a choice (#76):

- our accounts publish copies of their relay lists to White Noise's relays (MDK's bootstrap relays allow exactly this);
- or White Noise searches more widely for relay lists;
- or our default relays include one of White Noise's.

## What each side displays

- **Names:** each side shows the other's profile name and key correctly.
- **White Noise's chat list:** an invitation shows "Invitation pending" with the first message as the preview.
- **Messages sent before accepting** arrive with the invitation in peoples.chat (#20).
- **Deleting an account in peoples.chat** shows as "Dana Test left" in White Noise, which then titles the chat
  "Unnamed group".
- **Sign-out:** White Noise offers to remove its invitation keys from the relays. The relays then hold a deletion in
  place of the KeyPackage, as with ours.

## Found along the way

- **`PEOPLES_CHAT_RELAYS` never reached phone builds** (#18). The Tauri CLI passes only variables starting with
  `TAURI`, `WRY`, `CARGO_` or `RUST_` to the builds that Xcode and Gradle run. It is now `TAURI_PEOPLES_CHAT_RELAYS`,
  and both phone builds were checked to pick it up.
- **Two throwaway accounts from MDK's `wn` remain on public relays.** They are named "Mighty Alpaca" and "Kind Llama",
  and they hold a pseudonym profile, relay lists and a KeyPackage. Their local keys were deleted before their deletions
  were published. The KeyPackages expire after 30 days.

## Repeating this

1. Install White Noise's release APK in the Android emulator and sign up.
2. Run peoples.chat in the iOS simulator.
3. Go one way:
   - Share & Connect in White Noise shows its link: paste it into New chat in peoples.chat;
   - or scan our code with White Noise. The emulator's virtual camera can show it:
     1. start the emulator with `-camera-back virtualscene`;
     2. put a screenshot of the code on its wall with `adb emu virtualscene-image wall <png>`;
     3. with the scanner open, run `adb emu automation play <sdk>/emulator/resources/macros/Walk_to_image_room`, which
        walks the camera past the wall.
4. Check the other way with MDK's `wn`:
   - `wn keys check <key>` (in `docs/mdk.md`) shows whether an account's KeyPackage is fetchable;
   - a query for kinds 10002 and 10050 on White Noise's relays shows whether White Noise can find it.
