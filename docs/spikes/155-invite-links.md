# Spike #155: invite links and QR codes on a short peoples.chat address

**Result: a recommendation, no code.** Researched 27 November 2026.

A short link such as `https://on.peoples.chat/@emil` can open the app, show a profile page to people without it, and
hand White Noise users something their app understands. It needs one small service that the project runs. Messaging
never depends on that service: it only turns short names into public keys and renders pages.

The recommendation is in [Recommendation](#recommendation). The work is split into
[follow-up tasks](#follow-up-tasks), and what only Brian can decide is in [Decisions for Brian](#decisions-for-brian).

## Method and sources

- **Code** was read in the reference checkouts, at these commits. Paths in this document are relative to each
  repository.

  | Repository | Commit | Dated |
  | --- | --- | --- |
  | `marmot-protocol/marmot` (the spec) | `677a4b5c2f56a44b12b590ff0ecd9506b9abebbf` | 25 Nov 2026 |
  | `marmot-protocol/mdk` | `5d7e1b014c34e2b20ccbb861a90fd14cb9539982` | 23 Nov 2026 |
  | `marmot-protocol/whitenoise-android` | `fc08eebe912765e3dee50561910e276011017e23` | 23 Nov 2026 |
  | `marmot-protocol/whitenoise-ios` | `d7037f1477c25c33924e2ec8bfa8597546096e68` | 23 Nov 2026 |
  | `marmot-protocol/whitenoise-linux` | `d559c9173906631e8c3c8ee56c92aff44ff769ac` | 23 Nov 2026 |
  | `marmot-protocol/wn-tui` | `5191bc92f5da0532ebc469e908f6b33ae760b6c9` | 28 Jul 2026 |
  | `peoples-chat/www` | `8e1ea4c7f71dbb28bbca31ca5c6f78c1917492ec` | 24 Nov 2026 |
  | This repository | `95bf7efb14ca3c2df712f92d489ac95f924fc6dc` | |

- **Web pages** were all read on 27 November 2026. Each claim links to the page it comes from.
- **How far to trust it:**
  - The White Noise and Marmot claims the recommendation rests on were read line by line for this document: the files
    marked ✔ in [question 6](#6-white-noise-and-other-marmot-apps) and the spec lines in
    [question 7](#7-groups). The other White Noise citations come from a first pass that had no full-text search in
    the checkouts, so "not found" there means "not in the files read".
  - Web pages were read through a tool that returns page text, sometimes shortened. Quotes are as the tool gave them.
    Check a quote against its page before reusing it elsewhere.
  - **Unverified** marks what no primary source confirmed. Nothing in the recommendation depends on an unverified
    claim without saying so.

## The brief, point by point

| The brief asks for | Recommended answer | Where |
| --- | --- | --- |
| A QR code, and a URL inside it, on `https://<sub>.peoples.chat/<identifier>` | `https://on.peoples.chat/<identifier>`; the QR code carries the key form of it | [1](#1-domain-and-url-shape), [5](#5-shortkeys) |
| With the app: opens on the profile page | Universal links and App Links on `on.peoples.chat`, into a profile sheet with Start chat | [2](#2-opening-the-app) |
| Without it: a profile page with directions | A page the service renders, with three ways on | [3](#3-the-web-profile-page) |
| Works alongside White Noise and other Marmot apps | The page opens White Noise through `marmot://profile/<npub>`; handles are NIP-05 addresses that White Noise resolves and verifies; a second QR code its scanners read | [6](#6-white-noise-and-other-marmot-apps) |
| Shortkey | 8 characters, made by the service, resettable | [5](#5-shortkeys) |
| Handle | `/@emil`, which is also `emil@peoples.chat` (NIP-05) | [4](#4-handles) |
| Pubkey | `/npub1…`, which needs nothing stored | [1](#1-domain-and-url-shape) |
| Groups as well as individuals | The same link shapes; a group link asks an admin's app to add you | [7](#7-groups) |

## 1. Domain and URL shape

### What similar products do

| Product | Link | Identifier | What the link's server learns | Groups |
| --- | --- | --- | --- | --- |
| Signal | `signal.me/#eu/<64 characters>` | A random key and a server handle; the username is not in the link | A username hash and an encrypted blob; never the username | `signal.group/#…`, with optional admin approval and reset |
| Telegram | `t.me/<username>`, `t.me/+<hash>` | A chosen name, or a random hash | Everything | Invite links with expiry, a use limit and join requests |
| WhatsApp | `wa.me/<number>`, `wa.me/message/<code>` | A phone number, or a random code | Everything | `chat.whatsapp.com/<id>`, reset and approval |
| Matrix | `matrix.to/#/@user:server` | The user or room address | Nothing: all of it is after the `#` | Room aliases, same shape |
| Nostr (njump) | `njump.me/<npub or name@domain>` | The public key, or a NIP-05 address | The identifier; it then asks relays | NIP-29 groups: `naddr…?invite=<code>` |
| Threema | `threema.id/<8-character ID>` | A random ID | The ID | Not looked at |
| SimpleX | `<server>/a#<key>` | A queue address; no user ID | Encrypted link data; never the key | `/g#…` |

Sources:

- **Signal.**
  - A username is 3 to 32 characters of `a-z`, `0-9` and `_`, must end in at least two digits, and can be changed or
    deleted ([support article](https://support.signal.org/api/v2/help_center/en-us/articles/6712070553754.json)). The
    digits are there "to help keep usernames egalitarian and minimize spoofing"
    ([Signal blog](https://signal.org/blog/phone-number-privacy-usernames/)).
  - The link holds 32 random bytes and a server UUID after the `#`
    ([`UsernameRepository.kt`](https://raw.githubusercontent.com/signalapp/Signal-Android/main/app/src/main/java/org/thoughtcrime/securesms/profiles/manage/UsernameRepository.kt)).
    The server keeps the username's hash, an encrypted copy and that UUID
    ([`AccountController.java`](https://raw.githubusercontent.com/signalapp/Signal-Server/main/service/src/main/java/org/whispersystems/textsecuregcm/controllers/AccountController.java)).
    The link and QR code can be reset without changing the username (same blog post).
  - Group links: "The information needed to join the group is embedded in the link itself"
    ([Signal blog](https://signal.org/blog/group-links/)). "You can also require admin approval if someone requests to
    join your group via a link", and the link can be reset
    ([support article](https://support.signal.org/api/v2/help_center/en-us/articles/360051086971.json)).
- **Telegram.** `t.me/<username>`, `t.me/+<hash>` and the reserved paths are in
  [the links reference](https://core.telegram.org/api/links). Usernames: "a-z (case-insensitive), 0-9 and underscores.
  Length: 5-32 characters" ([`account.updateUsername`](https://core.telegram.org/method/account.updateUsername)).
  Invite links take an expiry, a use limit and admin approval ([invites](https://core.telegram.org/api/invites)).
- **WhatsApp.** Short links "reduce the syntax of the URL to a random code"
  ([Meta, QR codes and short links](https://developers.facebook.com/docs/whatsapp/business-management-api/qr-codes)).
  Group links begin with `https://chat.whatsapp.com/`, resetting one makes the old ones invalid, and joining is
  `auto_approve` or `approval_required`
  ([Meta, groups reference](https://developers.facebook.com/documentation/business-messaging/whatsapp/groups/reference.md)).
  The `wa.me/<number>` help page would not load, so that form is **unverified** here.
- **Matrix.** The link's details sit behind `#/` so browsers don't send them to the server
  ([matrix.to README](https://raw.githubusercontent.com/matrix-org/matrix.to/main/README.md)). The proposal for a
  `matrix:` scheme criticises that design: "it requires a web browser to run JavaScript code that resolves
  identifiers" ([MSC2312](https://raw.githubusercontent.com/matrix-org/matrix-spec-proposals/main/proposals/2312-matrix-uri.md)).
- **njump** takes `njump.me/<NIP-19 entity>` and `njump.me/<name@domain>`
  ([README](https://raw.githubusercontent.com/fiatjaf/njump/master/README.md)). It fetches the profile from relays on
  the server, with a 5-second limit
  ([`render_profile.go`](https://raw.githubusercontent.com/fiatjaf/njump/master/render_profile.go)), writes Open Graph
  tags ([`opengraph.templ`](https://raw.githubusercontent.com/fiatjaf/njump/master/opengraph.templ)) and offers a list
  of apps to open the profile in ([`clients.json`](https://raw.githubusercontent.com/fiatjaf/njump/master/clients.json)).
- **Threema.** `https://threema.id/ABCDEFGH` ([URL actions](https://threema.com/en/faq/url-actions)); without the app
  the link opens a page with a QR code and store links ([share link](https://threema.com/en/faq/share-link)).
- **SimpleX.** "the link key in the URI fragment … is never sent to the router"
  ([agent protocol](https://raw.githubusercontent.com/simplex-chat/simplexmq/stable/protocol/agent-protocol.md)).
- **Bluesky** handles are domain names, verified by a DNS record or a `/.well-known/atproto-did` file
  ([handle spec](https://atproto.com/specs/handle)). They are a model for handles ([question 4](#4-handles)), not for
  links.
- **Nostr has no link format for inviting a person.** No NIP in
  [the index](https://raw.githubusercontent.com/nostr-protocol/nips/master/README.md) defines one. The Marmot spec
  defines none either: "Human formats such as `npub` are presentation formats and do not appear in Marmot protocol
  bytes" (`foundation/identity.md:9-10`).

Two families show. Telegram, WhatsApp, Threema and njump put a short identifier in the path, and the server resolves
it. Signal, Matrix and SimpleX put everything after the `#`, and the server learns little or nothing. Only the first
family gives short links, a page per person and link previews. Brian's brief asks for those, so the recommendation
follows the first family and keeps the `#` for secrets.

### Candidate domains

Every product above uses a host of its own for links, apart from its marketing site.

| Candidate | Example | For | Against |
| --- | --- | --- | --- |
| `on.peoples.chat` | `on.peoples.chat/@emil` | Reads as a sentence for a person and for a group ("Book club on peoples.chat"). Two letters | None found |
| `to.peoples.chat` | `to.peoples.chat/@emil` | Two letters | Reads oddly before a name |
| `go.peoples.chat` | `go.peoples.chat/@emil` | Two letters, familiar | Widely used for marketing redirects, so it says little |
| `join.peoples.chat` | `join.peoples.chat/@emil` | Clear for groups and invitations | Wrong for a profile: you don't join a person. Four letters |
| `p.peoples.chat` | `p.peoples.chat/@emil` | Shortest | Hard to say aloud, and means nothing |
| `peoples.chat` itself | `peoples.chat/@emil` | No subdomain at all | The site is static files on a CDN (`peoples-chat/www` `README.md:62-70`), so every unknown path would have to be routed to a service. The app would claim links on the whole marketing site |
| A separate short domain | `<short>/@emil` | Shortest overall | A second name for people to learn to trust, to renew and to protect. Not searched for: availability is unknown |

A subdomain costs nothing and is set up with one DNS record. On an iPhone it has a second advantage: a link from
`peoples.chat` to `on.peoples.chat` counts as another domain, so tapping it on the website opens the app
([question 2](#2-opening-the-app)).

### How paths tell the identifiers apart

```text
https://on.peoples.chat/@emil            a handle
https://on.peoples.chat/k7m2p9qx         a shortkey: 8 characters
https://on.peoples.chat/npub1…           a public key, as White Noise shares it
https://on.peoples.chat/nprofile1…       a public key with relay hints
https://on.peoples.chat/<any of these>#<token>   a personal or group invitation's secret
```

- **`@` marks a handle,** as on Telegram, Mastodon and Bluesky profile pages. A handle can then never collide with a
  shortkey or a reserved path.
- **Keys name themselves:** `npub1` and `nprofile1` are NIP-19 prefixes
  ([NIP-19](https://raw.githubusercontent.com/nostr-protocol/nips/master/19.md)). njump uses the same shape.
- **A shortkey is anything else of 8 characters** from an alphabet without the letters i, l, o and u
  ([question 5](#5-shortkeys)). No shortkey can start with `npub1` or `nprofile1`, since both contain a `u` or an `o`.
- **Groups get no prefix.** The record behind a handle or a shortkey says whether it is a person or a group, so
  `/g/…` would only make links longer. The page and the app learn the kind when they resolve the link.
- **The secret part of an invitation goes after the `#`.** Browsers and link previews never send it
  ([RFC 3986, section 3.5](https://www.rfc-editor.org/rfc/rfc3986.html)). Today's personal invitations already work
  this way (`docs/core-interface.md`, Personal invitations).
- **Reserved:** `/.well-known/`, `/api/` and `/assets/`.

## 2. Opening the app

### iOS: universal links

- **The file.** The site serves `/.well-known/apple-app-site-association`, over HTTPS, "with a valid certificate and
  with no redirects". It names the app as `<team ID>.<bundle ID>` and lists which paths open it
  ([Supporting associated domains](https://developer.apple.com/documentation/xcode/supporting-associated-domains)).
  Rules can match the path, the query and the fragment, and can exclude
  ([components](https://developer.apple.com/documentation/bundleresources/applinks/details-swift.dictionary/components-swift.dictionary)).
- **Each subdomain is on its own.** "each requires its own entry in the Associated Domains entitlement, and each must
  serve its own `apple-app-site-association` file" (same page). So `on.peoples.chat` serves its own file, and the app
  lists `applinks:on.peoples.chat`.
- **Apple fetches the file, not the phone.** "Apple's content delivery network requests the
  `apple-app-site-association` file for your domain within 24 hours. Devices check for updates approximately once per
  week after app installation" (same page). A mistake in the file takes days to fix on phones.
- **It needs the developer account.** The file names the app by its team ID. That is #61 and #99, both open.
- **When it doesn't open the app** ([TN3155](https://developer.apple.com/documentation/technotes/tn3155-debugging-universal-links),
  [Allowing apps and websites to link to your content](https://developer.apple.com/documentation/xcode/allowing-apps-and-websites-to-link-to-your-content)):
  - A typed or pasted address: "Entering the URL directly into the web browser's address bar will never open the app".
  - A link to the same domain as the page it is on: "the system opens that link in Safari". A different subdomain
    counts as another domain and does open the app.
  - Without the app: "the system opens the URL in their default web browser".
  - In other browsers and in apps' own browsers: "It is up to third-party web browsers to enable universal links
    functionality."

### Android: App Links

- **The file.** `/.well-known/assetlinks.json` names the package and the SHA-256 fingerprint of its signing
  certificate, served as `application/json` without redirects, on each host
  ([Configure website associations](https://developer.android.com/training/app-links/configure-assetlinks)). With Play
  App Signing the fingerprint is Google's, from the Play Console (same page).
- **Each subdomain is on its own** here too ([Add App Links](https://developer.android.com/training/app-links/add-applinks)).
- **Unverified links open the browser.** From Android 12, "a generic web intent resolves to an activity in your app
  only if your app is approved for the specific domain"
  ([Android 12 behavior changes](https://developer.android.com/about/versions/12/behavior-changes-all)).
- **Timing.** The phone checks the file when the app is installed; Android 15 and later check again by themselves, and
  a change "can take up to seven days" ([Verify App Links](https://developer.android.com/training/app-links/verify-applinks)).
- **In other apps' browsers** it depends on that app: a web view with its own client loads every link itself
  ([WebView](https://developer.android.com/develop/ui/views/layout/webapps/webview)).

### Apps' own browsers, and the way around them

No primary source lists which apps' browsers pass links on. Branch, a link vendor, reports that on iOS Messages, Mail
and WhatsApp always do, that Safari, Chrome, Facebook and LinkedIn do under conditions, and that Instagram, Pinterest
and Telegram don't ([Branch](https://help.branch.io/developer-hub/docs/ios-universal-links), a secondary source).

The fix is the same everywhere: the link lands on the web page, and the page has a button the person taps.

- The button can't be a plain link to the page's own address, because of the same-domain rule above.
- **A link scheme of the app's own** works from any browser: `peopleschat://…`. Tauri's plugin registers one on phones
  with `{"scheme": ["peopleschat"], "appLink": false}` ([Tauri deep linking](https://v2.tauri.app/plugin/deep-linking/)).
- **On Android, an `intent://` link** opens the app and falls back to a web address when the app is missing:
  `S.browser_fallback_url` ([Chrome, Android intents](https://developer.chrome.com/docs/android/intents)).
- **In Safari, a Smart App Banner** offers the app or the App Store:
  `<meta name="apple-itunes-app" content="app-id=…, app-argument=…">`
  ([Smart App Banners](https://developer.apple.com/documentation/webkit/promoting-apps-with-smart-app-banners)). It
  needs the app's App Store ID, so it waits for a store listing.

### A link that survives installing the app

- **Android:** the Play Store passes a `referrer` of up to 512 characters to the app's first start
  ([Play Install Referrer](https://developer.android.com/google/play/installreferrer/library),
  [link format](https://developer.android.com/games/playgames/user-acquisition)). It works only for installs from
  Play.
- **iOS:** Apple's documentation offers no equivalent. Reading the clipboard asks the user since iOS 16
  ([UIPasteControl](https://developer.apple.com/documentation/uikit/uipastecontrol)).
- **Firebase Dynamic Links,** the usual free answer, shut down on 25 August 2025
  ([Firebase FAQ](https://firebase.google.com/support/dynamic-links-faq)).
- So for now: the page says "Install the app, then tap this link again". The app already keeps a link that arrives
  before there is an account (#72).

### How it fits the Tauri deep-link plugin (#72)

Today `src-tauri/tauri.conf.json:36-50` has one entry: `https`, host `peoples.chat`, path prefix `/i`.

- **More hosts are more entries** in `plugins.deep-link.mobile`
  ([plugin README](https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/deep-link/README.md)).
- **What the plugin writes:** an Android intent filter per entry, with `autoVerify` for web links, and one
  `applinks:<host>` per entry into the iOS entitlements
  ([`build.rs`](https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/deep-link/build.rs)). On
  iOS, which paths open the app is decided by the file on the server alone.
- **The fragment reaches the app on Android:** the plugin passes the link on unchanged
  ([`DeepLinkPlugin.kt`](https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/deep-link/android/src/main/java/DeepLinkPlugin.kt)),
  and #72 was checked on the emulator with links whose key is after the `#`. **On iOS it is unverified** until #99 can
  be tested on an iPhone. Personal invitations depend on it today as well.
- **Desktop.** The plugin documents link schemes of the app's own on desktop, set in `desktop.schemes`; on macOS they
  work only for the bundled app in `/Applications` ([Tauri deep linking](https://v2.tauri.app/plugin/deep-linking/)).
  Universal links on macOS are possible in Tauri's window library since
  [tao 0.34.2](https://v2.tauri.app/release/tao/v0.34.2/), but the plugin's documentation doesn't mention them:
  **unverified**, and a small spike of its own.
- **`src/platform.ts:89-94`** already hands every opened link to the interface, whatever its host.

### Where the link should land in the app

The brief says "opens the app on the profile page". Today a link opens New chat with the link filled in, so the user
confirms before a chat starts (`docs/core-interface.md`, Links that open the app). `previewInvite` already fetches a
name and photo for a link. So the opened link can show the person's profile, with Start chat, and keep the
confirmation.

## 3. The web profile page

### Where the data comes from

| Option | Link previews | Short links and handles | What it costs |
| --- | --- | --- | --- |
| A static page; the browser asks relays itself | None per person | No | Nothing: fits the website as it is |
| **A service renders the page, and asks relays itself** | Yes | Yes | One small service |
| The same, on a CDN's edge functions | Yes | Yes, with a hosted database | A second vendor's runtime and database |

- **Previews need a server.** The apps that draw a preview fetch the page themselves and run no script. Apple: "Link
  previews do not follow `meta` redirects, nor run JavaScript; metadata must be available directly on the linked page"
  ([TN3156](https://developer.apple.com/documentation/technotes/tn3156-create-rich-previews-for-messages)). WhatsApp
  crawls the address and wants `og:title`, `og:description` and `og:url` within the first 300 KB, and an image under
  600 KB ([WhatsApp link previews](https://developers.facebook.com/docs/whatsapp/link-previews/)). Signal's preview is
  fetched by the sender's app through a proxy
  ([Signal blog](https://signal.org/blog/i-link-therefore-i-am/)). None of them sees what is after the `#`.
- **The profile is the Nostr profile (kind 0).** The service fetches it from relays when a page is asked for, as njump
  does, and keeps it in memory for some minutes. It stores no profile.
  - Where to look: the project's relay, White Noise's relays, where the app copies every profile change
    (`core/src/messaging.rs:83-90`), and the relays a link or record names.
  - The photo: the service fetches it, shrinks it and serves it itself. Otherwise each visitor's browser would contact
    whatever server the photo's address names. The app avoids that for the same reason
    (`docs/core-interface.md`, Photos).
- **Open Graph:** `og:title` ("Emil Dahl on peoples.chat"), `og:description`, `og:image` and `og:url`
  ([ogp.me](https://ogp.me/)).
- **Not for search engines:** `X-Robots-Tag: noindex`, and no `robots.txt` rule that would hide the header from them
  ([Google, block indexing](https://developers.google.com/search/docs/crawling-indexing/block-indexing)).
- **Profile text is shown as plain text.** A name or an about text is written by its owner and may hold links made to
  mislead.

### The directions it shows

1. **Open in peoples.chat:** the button from [question 2](#2-opening-the-app).
2. **Get peoples.chat:** store links once there are any. Today the website has a sign-up form for the preview.
3. **Using White Noise or another Marmot app?**
   - A button to `marmot://profile/<npub>`, which White Noise for Android, iOS and Linux open
     ([question 6](#6-white-noise-and-other-marmot-apps)).
   - The key (`npub1…`) with Copy, and the handle as `emil@peoples.chat` when there is one: White Noise finds a person
     by either.
4. **On a computer:** the link as a QR code, to scan with a phone, as Threema's page does.

For a group, the page shows what the link's maker chose to show, and says that a member's app will add you
([question 7](#7-groups)).

### Website repository, or a small service?

- **The website can't do it as it is.** It is a static Astro site, uploaded to Bunny storage behind Bunny's CDN
  (`peoples-chat/www` `README.md:62-70`). Its one dynamic part, the sign-up form, is a separate Bunny edge script that
  stores nothing (`ops/edge/signup.ts:1-5`).
- **Astro can render on demand,** but only through an adapter for Cloudflare, Netlify, Node.js or Vercel
  ([Astro, on-demand rendering](https://docs.astro.build/en/guides/on-demand-rendering/)). That means moving the site,
  or running a second deployment beside it.
- **Bunny's edge scripts** run on Deno and can call other servers
  ([edge scripting](https://bunny.net/docs/edge-scripting-overview)). Whether they can hold the WebSocket connections
  that relays speak is **unverified**. Bunny's database is "currently in Public Preview"
  ([Bunny Database](https://bunny.net/docs/database)).
- **A small service of our own** fits what the project already runs: the relay is strfry behind Caddy on a Hetzner
  server, set up from files in `relay/` (`relay/README.md:7-17`). A links service is one more program behind the same
  Caddy, with one SQLite file.
- **So: a small service,** not the website repository. It borrows the website's colours and font so the page looks
  like peoples.chat. Where its code lives is in [Decisions for Brian](#decisions-for-brian).

**What is live today.** `https://peoples.chat/i`, `/.well-known/assetlinks.json` and
`/.well-known/apple-app-site-association` all answered 404 on 27 November 2026, and the website repository has no `/i`
page. So the links the app shares today open neither the app nor a page on a real phone. #72 and #99 name this as
waiting for the website.

## 4. Handles

### NIP-05 is the bridge

A handle `emil` is the Nostr address `emil@peoples.chat`
([NIP-05](https://raw.githubusercontent.com/nostr-protocol/nips/master/05.md)):

- A client asks `https://peoples.chat/.well-known/nostr.json?name=emil` and gets `{"names": {"emil": "<hex key>"}}`,
  optionally with a `relays` map from key to relay addresses.
- The local part may use only `a-z0-9-_.`.
- The file should be served with `Access-Control-Allow-Origin: *`.
- "The `/.well-known/nostr.json` endpoint MUST NOT return any HTTP redirects."
- "Clients must always follow public keys, not NIP-05 addresses." A handle is a pointer; the key stays the identity.
  The spec says the same: "Readable names such as name@domain are optional pointers to a key" (`docs/spec.md:116`).

What this buys with White Noise, from its code ([question 6](#6-white-noise-and-other-marmot-apps)):

- Its Android and iOS apps resolve a typed `emil@peoples.chat` in New chat.
- Both show the address on the profile, with a verified mark once the file answers with the same key.
- So a handle is the one identifier a peoples.chat user can say aloud to a White Noise user.

**The address is on the main domain.** `emil@peoples.chat` is looked up on `peoples.chat`, which the CDN serves. The
CDN has to pass that one path to the service without a redirect. A Bunny middleware script can answer a request
itself ("Return `Promise<Response>` to short-circuit and respond immediately",
[middleware](https://bunny.net/docs/scripting/middleware/overview.md)); that it suits this path needs a test. The
alternative, `emil@on.peoples.chat`, needs no routing and reads worse.

### The app can resolve names of any domain

Today `addContact` refuses a name: "Finding people by name is not possible yet" (`core/src/invite.rs:56-57`). Reading
`name@domain` through NIP-05 needs no service of ours, and works for White Noise users' addresses too. It is the
cheapest step in this document.

### Rules

| Question | Recommended | Why, and what others do |
| --- | --- | --- |
| Characters | `a-z`, `0-9` and `_`; starts with a letter; 4 to 24 characters; stored lowercase | Signal and Telegram allow the same three. One separator instead of three (`.`, `-`, `_`) leaves fewer lookalikes. ASCII only avoids letters from other alphabets that look the same ([UTS #39](https://www.unicode.org/reports/tr39/)) |
| Uniqueness | First come, first served. One handle per account, one account per handle | Telegram: first come, first served ([FAQ](https://telegram.org/faq)). Signal avoids the question by requiring digits |
| Registration | The app signs the request with the account key. No email, no password | [NIP-98](https://raw.githubusercontent.com/nostr-protocol/nips/master/98.md): a kind 27235 event in the `Authorization` header, bound to the address and method, valid for about 60 seconds |
| Reserved names | Names of 1 to 3 characters; the project's names (`peoples`, `peopleschat`, `support`, `admin`…); White Noise's and Marmot's; common service names | [RFC 2142](https://www.rfc-editor.org/rfc/rfc2142.txt), [The Big Username Blocklist](https://raw.githubusercontent.com/marteinn/The-Big-Username-Blocklist/main/README.md), [reserved-usernames](https://raw.githubusercontent.com/shouldbee/reserved-usernames/master/README.md) |
| Squatting | A limit per network address, kept in memory. The key must have a profile and a key package on the relays. No sale or transfer of names. The terms allow taking back a name used to pass as someone else | Keys are free, so a limit per key stops nobody. Telegram reserves "the right to recall usernames" ([FAQ](https://telegram.org/faq)) |
| Changing | Allowed. The old name stays out of use for 90 days, and its owner can take it back in that time | Signal frees the old name "soon"; Bluesky reserves the last one without end ([Bluesky](https://bsky.social/about/blog/4-28-2023-domain-handle-tutorial)). Old links must not reach a stranger the next day |
| Deleting | A signed request removes it at once, with the same 90 days. Deleting the account in the app releases the handle | GDPR Article 17 ([text](https://gdpr-info.eu/art-17-gdpr/)) |
| Inactivity | No automatic release for now | The service can't tell an inactive account from a quiet one |
| Listing | None. The file answers one name at a time | NIP-05 allows a static file with every name; that would publish the whole list |

The app writes the address into the profile's `nip05` field. MDK's profile update carries that field
(`docs/mdk.md:298-299`).

This reopens a decision of 22 November 2026: "Usernames are left out for now" (`docs/core-interface.md`, Profile).

## 5. Shortkeys

A shortkey is for people who haven't picked a handle: the short link works from the first day.

- **Alphabet:** Crockford's base32, in lowercase: digits and letters without i, l, o and u. Decoding ignores case and
  reads `i` and `l` as `1` and `o` as `0` ([Crockford](https://www.crockford.com/base32.html)). A link read aloud or
  typed from paper survives.
- **Length: 8 characters,** which is 40 bits, about 1.1 × 10¹² values.
  - With a million shortkeys in use, one random guess hits one with probability 10⁻⁶ (shortkeys in use, divided by
    all values).
  - The service makes them, so it checks each new one against those in use; collisions never reach a user.
  - A guessed shortkey reveals a public key and its profile, which the relays publish anyway. It reveals no secret:
    invitation tokens are after the `#` and never reach the service.
  - Threema's IDs are 8 characters too ([Threema ID](https://threema.com/en/faq/threema-id)).
- **No expiry.** A profile link is printed, pinned and kept in address books.
- **Reset instead.** The owner's app asks for a new shortkey, and the old one answers "This link was reset" for good.
  It is never given out again. Signal does the same with its link and QR code.
- **Made when it is first needed:** when the user first shares their link. Whether that happens without asking is in
  [Decisions for Brian](#decisions-for-brian).
- **What the service stores per shortkey:** the shortkey, the public key, up to three relay hints, whether it is a
  person or a group, the day it was made, and whether it was reset. No name, no photo, no network address.

### What goes in the QR code

The QR code and the shared link have different jobs, so they carry different forms of the same address.

| | Carries | Why |
| --- | --- | --- |
| **Shared link** | The handle if there is one, else the shortkey | Short, and it has a preview |
| **QR code** | `https://on.peoples.chat/npub1…` | The scanning app reads the key from the code itself, without asking the service. Scanning in person keeps working when the internet is gone, which is the app's whole point. A phone's camera opens the page |
| **Second QR code, "For White Noise"** | `marmot://profile/<npub>` | White Noise's scanners read nothing on our domain ([question 6](#6-white-noise-and-other-marmot-apps)) |

Size, from the capacity table at
[thonky.com](https://www.thonky.com/qr-code-tutorial/character-capacities) (a secondary source; the standard's own
table is published as images) and the size rule at
[qrcode.com](https://www.qrcode.com/en/about/version.html), 21 modules across at version 1 and 4 more per version:

| Content | Characters | QR version at error level M | Modules across |
| --- | --- | --- | --- |
| Today: `nostr:nprofile1…` with three relays | | 9 (`docs/mdk.md:197`) | 53 |
| `https://on.peoples.chat/npub1…` | 87 | 6 | 41 |
| `marmot://profile/npub1…` | 80 | 5 | 37 |
| `https://on.peoples.chat/k7m2p9qx` | 32 | 3 | 29 |

So the new code is also easier to scan than today's. The relay hints go, and little is lost: the app looks people up
on its own relays and White Noise's (`core/src/messaging.rs:134-139`), and White Noise's scanners drop the hints
anyway (`docs/mdk.md:203-204`).

An all-capitals address would make the code one version smaller, since QR codes pack capitals and digits more tightly
([alphanumeric mode](https://www.thonky.com/qr-code-tutorial/alphanumeric-mode-encoding)), and bech32 allows it:
"inside QR codes uppercase SHOULD be used"
([BIP-173](https://raw.githubusercontent.com/bitcoin/bips/master/bip-0173.mediawiki)). Whether every phone still
opens the app for `HTTPS://ON.PEOPLES.CHAT/…` is untested, so it is left out.

## 6. White Noise and other Marmot apps

Files marked ✔ were read for this document. The rest come from the first pass ([Method](#method-and-sources)).

### What White Noise takes for adding a contact

| Input | Android | iOS | Linux |
| --- | --- | --- | --- |
| `npub1…` | Yes | Yes | Yes |
| `nprofile1…` | Scanner only; pasting is refused | Yes | Yes |
| `nostr:` before either | Yes, same limits | Yes | Yes |
| `marmot://profile/<npub>` | Yes | Yes, also with an nprofile or hex key | Yes |
| `whitenoise://profile/<npub>` | Yes | Yes | No |
| `https://whitenoise.chat/profile/<npub>`, also on `marmot.app` | Yes | No | No |
| An https link on any other host | **No** | **No** | Only `njump.me/` |
| `name@domain` (NIP-05) | Yes | Yes | In the group member fields only |

- **Android** ✔ `app/src/main/java/dev/ipf/whitenoise/android/core/ProfileLink.kt`:
  - `parse` (38-50) takes a bare npub, `nostr:` plus an npub, and links.
  - Link schemes are `marmot`, `whitenoise` and the build's variants (105-112), as `scheme://profile/<npub>` or
    `scheme://<npub>` (74-80).
  - Web links count on four hosts only: `PROFILE_HTTPS_HOSTS = setOf("whitenoise.chat", "www.whitenoise.chat",
    "marmot.app", "www.marmot.app")` (114), and `if (host !in PROFILE_HTTPS_HOSTS) return null` (85).
  - Nothing in the app makes such web links: "the app's own whitenoise:// scheme stays registered for inbound/legacy
    links but is no longer generated" (13-16). `https://whitenoise.chat/profile/<npub>` answered 404 on 27 November
    2026.
- **Android, pasting** ✔ `…/core/RecipientPastePolicy.kt`: a paste is searched for an npub anywhere in it (146-185).
  One valid npub, with no letter, digit, `_` or `-` on either side, replaces the pasted text (98-110). A paste that
  holds an `nprofile1` anywhere is refused (48-55, 174-180).
  - So pasting `https://on.peoples.chat/npub1…` into White Noise for Android gives the npub. This follows from the
    code; no test in the repository covers it.
  - Pasting our link with an nprofile is refused. That is one reason the QR code and the page use the npub.
- **Android, NIP-05** ✔ `…/core/Nip05Resolver.kt:89`:
  `URL("https://$domain/.well-known/nostr.json?name=$encodedName")`.
- **iOS** ✔ `Shared/NostrProfileReference.swift`: `referenceForResolution(from:)` (34-54) takes an npub, an nprofile
  or a hex key, the same after `nostr:`, and a link whose scheme is one of `"marmot", "marmot-staging", "whitenoise",
  "whitenoise-staging"` (✔ `Shared/DeepLink.swift:36-39`). There is no branch for web links, and nothing searches
  inside a longer text. An all-capitals key is accepted (283).
- **iOS, NIP-05** ✔ `whitenoise-ios/Recipients/Nip05Resolver.swift:46-50` builds the same lookup address.
- **Linux** ✔ `app/deeplink.odin:25-38` takes `marmot://profile/<ref>` only. ✔ `app/nostr_ref.odin:36-44` allows the
  prefixes `@`, `nostr:`, `https://primal.net/e/` and `https://njump.me/` before a key. NIP-05 lookups are in
  `app/nip05.odin:57-87`, used from the group member fields (`app/handlers.odin:2035`, `2350-2353`).
- **wn-tui** has no parsing of its own: it passes what is typed to the `wn` command (`src/main.rs:939`,
  `src/app.rs:1310-1319`).

### QR codes

| App | Its own code | Its scanner reads |
| --- | --- | --- |
| Android | `marmot://profile/<npub>?from=qr` (✔ `ProfileLink.kt:16-20`) | What `ProfileLink.parse` takes, or text that starts with `npub1` or `nprofile1`, with or without `nostr:` (✔ `…/ui/qr/QrScanResult.kt:77-107`) |
| iOS | `marmot://profile/<npub>` (✔ `whitenoise-ios/Settings/ShareAndConnectView.swift:35-37`, ✔ `Shared/DeepLink.swift:53-72`) | The same forms as pasting (✔ `Shared/DeepLink.swift:112-123`) |
| Linux | `marmot://profile/<npub>?from=qr` (`app/pane_widgets.odin:55-65`) | It has no scanner |
| wn-tui | None | None |

A web link in a QR code is refused by both scanners. Android's test says so:
`assertEquals(QrScanOutcome.Invalid, viewProfile("https://example.com"))`
(✔ `app/src/test/java/dev/ipf/whitenoise/android/ui/qr/QrScanResultTest.kt:93`). iOS answers "That QR code isn't a
White Noise profile." (`whitenoise-ios/Chats/NewChatFlowView.swift:140-145`).

### Links that open White Noise

- **Android** ✔ `app/src/main/AndroidManifest.xml:188-199`: one filter for the build's scheme and `marmot`. No web
  host, no `autoVerify`, no `nostr:`.
- **iOS** ✔ `Config/Production.xcconfig:6-7`: `marmot` and `whitenoise`. No universal links: the entitlements have no
  associated domains (✔ `whitenoise-ios/whitenoise-ios.entitlements`).
- **Linux:** `x-scheme-handler/marmot` (`assets/whitenoise-linux.desktop:6,10`).
- **Routes:** `profile/<npub>` opens the profile. `chat/<group id>` opens a chat the user is already in
  (✔ `Shared/DeepLink.swift:14-15`, `92-97`; Android `core/ChatLink.kt:16-53`). Neither joins anything.
- `https://whitenoise.chat/.well-known/assetlinks.json` and `apple-app-site-association` answered 404 on 27 November
  2026, which agrees with the code.

### Sharing, and the profile

- **Share** gives `marmot://profile/<npub>` on both phones (Android `ui/settings/ShareConnectScreen.kt:184-191`;
  iOS ✔ `ShareAndConnectView.swift:75-80`). Copy gives the bare npub.
- **NIP-05 on a profile** is shown by all four apps. Android and iOS check it and show a verified mark when the
  resolved key matches (Android `ui/chats/newchat/RecipientResolution.kt:310-318`; iOS
  `whitenoise-ios/Profile/ProfileAddressVerificationModel.swift:22-56`). iOS takes a local part of `[a-z0-9._+-]`
  (`Shared/ContentSanitizer.swift:183-189`); the handle rules above fit inside that.
- **Finding the account** is a separate matter from links: White Noise looks a new contact up on its own two relays
  (✔ `core/MarmotClient.kt:59-63`; #76). The app already copies profiles there and looks people up there (#113, #120).
  Whether relay lists and key packages are copied too is #76, still open. A link that reaches White Noise is only
  useful once that is settled.

### What a White Noise user gets from our link

| They… | Today's link (`peoples.chat/i#nprofile1…`) | Recommended |
| --- | --- | --- |
| Tap it in a chat or mail | A 404 page | The profile page, with "Open in White Noise" (`marmot://profile/<npub>`), the key to copy, and the handle |
| Scan our QR code with the phone's camera | Nothing: no app takes `nostr:` | The same page |
| Scan it inside White Noise | Works: it reads `nostr:nprofile1…` | "Not a White Noise profile" for the main code. The second code works |
| Paste the link into White Noise | Refused on both phones | Android: the key form (`/npub1…`) works, others become a name search. iOS: a name search that finds nothing |
| Type `emil@peoples.chat` | Nothing to type | Found on both phones, and verified |

The one thing that gets worse is scanning our main QR code inside White Noise. Hence the second code, and a question
for White Noise: Android already accepts profile links on a list of hosts. A rule that reads `https://<host>/<npub>`
on more hosts, or on any, would make every Marmot app's links work in every other. That is a conversation with them
([Decisions for Brian](#decisions-for-brian)).

### Other Marmot apps

MDK itself reads hex keys, `npub`, `nprofile` and `marmot://profile/` links (`crates/marmot-uniffi/API-REFERENCE.md`,
`normalize_member_ref`, from the first pass). Any app built on it can take the key from our page. `marmot-ts` was not
examined.

## 7. Groups

### How a group is joined today

- **Only by being added.** An inviter fetches the person's key packages, commits an Add, and sends a Welcome:
  "Later, an inviter fetches them, adds a leaf for each KeyPackage it selects ... and sends each selected device a
  Welcome" (`foundation/key-packages.md:9-11`). The Welcome travels as a gift wrap to the person's inbox relays
  (`transports/nostr.md:226-231`).
- **Only an admin can add someone else.** "The following non-component operations also require an active admin to
  commit: invite a new member; remove another member" (✔ `app-components/admin-policy-v1.md:100-103`). The one
  exception is a member adding their own other devices (✔ 107-112).
- **The joiner checks it.** A Welcome is rejected "unless the author is authorized to add in the resulting group
  state: either the author is an active admin there" or it is the joiner's own account
  (✔ `protocol-core/joining.md:69-73`).
- **Nobody can add themselves.** "All other Commits from non-admins are invalid. In particular, adopted v1 defines no
  non-admin `new_member_commit` External Commit path" (✔ `protocol-core/group-messaging.md:55-57`).
- **But a request is allowed, as an app's own feature.** "A request for an admin-gated group-state change is an
  application payload or feature-owned request flow, not a standalone MLS proposal" (✔ 60-61). The spec defines no
  such flow, and no group link. Its `ideas/` folder lists three ideas, none of them about joining
  (`ideas/README.md:19-21`).
- **MDK has the adding and nothing else:** `invite_members` (`crates/marmot-app/src/runtime/commands.rs:1320`). No
  join request and no group link were found in its API reference.
- **White Noise has no group links either.** Members are added by a member who supplies their key
  (Android `state/AppStateProfileGroups.kt:179-200`; iOS `whitenoise-ios/Group/AddMembersSheet.swift:247-256`).

### What a link can't carry

- **Not the group's id.** "That id is private group security state and MUST NOT be published through a transport
  envelope" (✔ `protocol-core/group-setup.md:21-22`).
- **Better not its relay address either.** "An outsider can learn the Nostr routing handle and send envelopes that
  reach validation and trial decryption" (`security-considerations.md:203`).
- **The group's name and picture are not public.** They live in the group's encrypted state
  (`app-components/group-profile-v1.md:9`). A page can only show what the link's maker hands the service.

### What a group link can realistically do

A group link names **who to ask**, not the group:

1. An admin's app makes it: a shortkey or handle whose record holds the admin's key, a name and picture for the page
   if the admin wants them shown, and a random token after the `#`.
2. Someone opens it. The page, or their app, says "Book club. Emil will add you."
3. Their app sends the admin's app a join request with the token. The app has this mechanism already: a personal
   invitation's token travels to the inviter as an app event of the app's own kind, 9910 (#95,
   `docs/core-interface.md`, Personal invitations).
4. The admin's app checks the token and adds the person with MDK's ordinary `invite_members`, by itself or after the
   admin says yes.
5. The person gets an ordinary Welcome. Their app checks that it comes from the admin the link named, which is the
   spec's own trust rule: "The first-contact trust root is therefore the Welcome author"
   (✔ `protocol-core/joining.md:117-119`).

The limits, to say plainly in the interface:

- **Joining isn't instant.** It happens when the admin's phone is next online.
- **The link dies with its maker.** If that admin leaves or loses admin rights, the link stops working. A record
  could name several admins later.
- **White Noise users can't join by it,** and White Noise admins can't serve one. For them the page says who to ask,
  with that person's profile button and key.
- **Anyone with the link can ask.** Whether a valid token is enough, or the admin confirms each person, is in
  [Decisions for Brian](#decisions-for-brian). Signal offers both.
- **It needs group chats,** which the app doesn't have yet (`docs/core-interface.md`, Not built yet).

A join request that every Marmot app understands would be a proposal to the Marmot spec. The sentence quoted above
leaves the room for it.

## 8. How it fits peoples.chat as built

| Built today | Keep, replace or combine |
| --- | --- |
| `inviteLink`: `nostr:nprofile1…`, in the QR code (#19) | **Keep reading it.** The QR code moves to `https://on.peoples.chat/npub1…`, with the second code for White Noise |
| `shareLink`: `https://peoples.chat/i#nprofile1…` (#72) | **Replace** with the handle, shortkey or key link on `on.peoples.chat`. Keep reading the old form: `core/src/invite.rs:28-35` |
| `addContact` reading White Noise's links and bare keys (`core/src/invite.rs:83-117`) | **Keep, and add** the new host's forms and `name@domain` |
| `createInvite`: `…/i#nprofile1…/<token>`, valid once and for 7 days, kept by the inviter's app (#95) | **Combine.** The same token, after the `#` of the user's short link: `https://on.peoples.chat/k7m2p9qx#<token>`. The service never sees the token, and the rules stay the inviter's app's |
| `previewInvite`: the inviter's name and photo without an account (#95) | **Keep.** It also feeds the profile sheet a link opens |
| Deep links: one entry for `peoples.chat/i` (#72) | **Replace** with `on.peoples.chat`, any path, and add the scheme `peopleschat` for the page's button |
| The share sheet (#66) | **Keep.** It shares whatever `shareLink` is |
| "Usernames are left out for now", "No server" (both 22 November 2026) | **Reopened** by this brief |

What stays true:

- **The app works without the service.** The QR code carries the key. A link with a key in it is read by the app
  alone. When the service can't be reached, the app shares the key link instead of the short one.
- **The service is not in any message's path.** Chats, invitations and radio never touch it.
- **Personal invitations keep their privacy property** where it matters: the token is never on a server.

What changes: the website no longer never sees a key. `on.peoples.chat/npub1…` in a browser tells the service which
profile was looked at, as a relay learns when the app fetches a profile. When the app is installed, the phone opens it
without asking the service at all.

## 9. The server

### The smallest design

One program, one SQLite file, behind Caddy.

| Request | Answer |
| --- | --- |
| `GET on.peoples.chat/<identifier>` | The page; or, for the app, the record as JSON: key, relay hints, person or group |
| `GET peoples.chat/.well-known/nostr.json?name=<handle>` | NIP-05 |
| `GET on.peoples.chat/.well-known/assetlinks.json`, `apple-app-site-association` | The two files that let phones open the app |
| Signed requests under `/api/` (NIP-98) | Make or reset a shortkey; claim, change or release a handle; make or remove a group link |

- **The first version stores nothing:** key links and their pages need no database. Shortkeys, handles and groups
  each add a table.
- **Caddy** gets certificates by itself ("Automatic HTTPS provisions TLS certificates for all your sites and keeps
  them renewed", [Caddy](https://caddyserver.com/docs/automatic-https)) and passes requests on
  ([`reverse_proxy`](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy)). The relay's Caddy already
  runs this way and "Writes no access log" (`relay/README.md:15`).

### What it stores, and what it can see

| | Stored | Seen in passing |
| --- | --- | --- |
| Per shortkey | Shortkey, public key, up to three relays, kind, day made, reset or not | |
| Per handle | Handle, public key, day claimed, day released | |
| Per group link | The above, plus the name and picture the admin chose to show | |
| Per request | Nothing | The network address, and which identifier was asked for |
| Never | Messages, contacts, who talks to whom, invitation tokens, account keys | |

It can link a visitor's network address to the profile they looked at, for as long as the request lasts. Nothing
writes that down: no access log, limits kept in memory, the system journal capped as on the relay (#90, section 2.3).

Unlike the relay, it does hold identities: handle to key. The spec says of the project's services "None of these
holds identities or readable content" (`docs/spec.md:148`). That sentence would have to change, which makes it
Brian's decision.

### Privacy and GDPR

Facts, not legal advice. The relay's open question, to "have someone who knows Danish and EU rules for hosting
services review" the terms (#90, section 8), covers this service too.

- **A key and a handle are personal data.** Data under a pseudonym "should be considered to be information on an
  identifiable natural person" ([Recital 26](https://gdpr-info.eu/recitals/no-26/)). Network addresses are online
  identifiers ([Recital 30](https://gdpr-info.eu/recitals/no-30/)). The usual citation for addresses, the EU court's
  Breyer judgment (C-582/14), could not be opened: **unverified**.
- **Keep little:** "adequate, relevant and limited to what is necessary"
  ([Article 5(1)(c)](https://gdpr-info.eu/art-5-gdpr/)). The table above is that list.
- **A lawful basis** is needed ([Article 6](https://gdpr-info.eu/art-6-gdpr/)). The user asks for the handle or link
  themselves, which points at 6(1)(b). For the reviewer to confirm.
- **Erasure** "without undue delay" ([Article 17](https://gdpr-info.eu/art-17-gdpr/)): the signed delete request.
- **A privacy note** says all this. The relay needs one too (`relay/README.md:91`).
- **The Digital Services Act.** The service stores what users give it and shows it to the public. A hosting service
  must offer a way to report illegal content (Article 16). Small and micro enterprises are exempt from most of the
  platform section (Article 19). Both were read in a reproduction of the text, because the official site would not
  load ([Article 16](https://www.springlex.eu/en/packages/dsa/dsa-regulation/article-16/),
  [Article 19](https://www.springlex.eu/en/packages/dsa/dsa-regulation/article-19/)): treat as **unverified** until
  the reviewer confirms.

### Abuse

| Risk | Answer |
| --- | --- |
| A handle that passes as someone else | Reserved names, the terms, a contact address, and a block list by handle |
| An offensive name, text or photo on a page | A block list by key. It hides the page; the account and its chats are untouched, since the service controls nothing else |
| Misleading links in a profile | Profile text is shown as plain text |
| Registering names in bulk | Limits per network address; the key needs a profile and a key package on the relays; one handle per key |
| Listing every shortkey | Limits per network address. A hit reveals only what relays publish |
| Flooding | Limits in Caddy and in the service; pages are cached for minutes |
| The service is taken over | It could point a handle or shortkey at another key. The app shows the key's own profile before a chat starts, and marks contacts verified in person (`docs/spec.md:116`). Key links and QR codes don't depend on the service |
| The service is down | Short links and handles stop resolving. Chats, key links and QR codes work |

### Hosting, cost and who runs it

| Option | Cost a month | Notes |
| --- | --- | --- |
| **On the relay's server** | Nothing more | Hetzner, Helsinki, 1 vCPU and 1 GB (`relay/README.md:7-8`). Same Caddy, same no-log setup. One operator sees both the relay's and the links' traffic, which is true of the project either way |
| A server of its own | About €5.49, plus backups | Hetzner's smallest plan on offer, 1 vCPU and 1 GB in Helsinki ([Hetzner](https://www.hetzner.com/cloud/regular-performance/)). Keeps a fault in one from taking the other down |
| Cloudflare Workers with D1 | Free at this size | 100,000 requests and 5 million rows read a day ([pricing](https://developers.cloudflare.com/workers/platform/pricing/)). The database can be pinned to the EU; the code still runs worldwide ([data location](https://developers.cloudflare.com/d1/configuration/data-location/)). A US company in the path of every lookup |
| Bunny edge scripts and database | About $0.22 at least | $0.20 per million requests ([pricing](https://bunny.net/docs/scripting/pricing)). The database is a preview; relay connections from scripts are unverified |

Who runs it is open decision #11, which today covers relays, the media server and the push server. This service
joins that list. The recommendation is the same operator as the relay, on the relay's server to begin with.

## Recommendation

**One sentence:** put every link on `https://on.peoples.chat/`, served by one small service next to the relay, built
in four steps so that the first step stores nothing.

| Topic | Recommendation |
| --- | --- |
| **Domain** | `on.peoples.chat` |
| **URL scheme** | `/@handle`, `/<shortkey>`, `/npub1…`, `/nprofile1…`; an invitation's token after `#`; no prefix for groups |
| **Identifiers** | All three. The key works from the first step and needs nothing stored. The shortkey is what people share before they pick a handle. The handle is also `handle@peoples.chat` |
| **QR code** | `https://on.peoples.chat/npub1…`, readable without the service, with a second code `marmot://profile/<npub>` for White Noise |
| **Opening the app** | Universal links and App Links on `on.peoples.chat`; the page's button uses the scheme `peopleschat://` and, on Android, an `intent://` link. A link opens a profile sheet with Start chat. iPhones wait for the developer account (#61, #99) |
| **Web page** | Rendered by the service from the Nostr profile, with Open Graph tags, not indexed. Three ways on: open peoples.chat, get peoples.chat, open in White Noise or copy the key |
| **Handles** | NIP-05 on `peoples.chat`. `a-z`, `0-9`, `_`, 4 to 24 characters, first come, first served, claimed with a signature, reserved list, 90 days before a released name returns. The app also resolves `name@domain` of any domain |
| **Shortkeys** | 8 characters of Crockford base32, no expiry, resettable, made at first share |
| **White Noise** | The page's `marmot://` button, the key, the handle as a NIP-05 address, the second QR code; and ask White Noise to read profile links on more hosts |
| **Groups** | A link to an admin's app, which adds the person: the personal invitation's mechanism, for a group. After group chats exist. Propose a shared join request to Marmot later |
| **Server** | One program with SQLite behind Caddy, on the relay's server, with no logs. Never in a message's path |
| **What exists** | Keep reading every old form. Personal invitations keep their token and rules, on the short link |

**The steps:**

1. **Key links and the page.** `on.peoples.chat/npub1…` opens the app or shows the page. Nothing is stored. This
   alone fixes what is broken today: shared links that reach a 404.
2. **Shortkeys.** The first table. Links get short.
3. **Handles.** The second table, and NIP-05 on the main domain.
4. **Group links,** once the app has group chats.

Resolving `name@domain` in the app is independent of all four and can go first.

## Follow-up tasks

Each is S or M. "Decision" refers to the numbered questions below.

| # | Task | Size | Depends on |
| --- | --- | --- | --- |
| 1 | **Core: resolve `name@domain` in `addContact` and `previewInvite`** through NIP-05, for any domain. No redirects, a size limit and a timeout. Replaces "Finding people by name is not possible yet" | S | None |
| 2 | **Infra: `on.peoples.chat`.** A DNS record to the server, a Caddy site without an access log, a folder for the service beside `relay/` with its setup script | S | Decisions 1, 2, 9 |
| 3 | **Links service, first version.** Pages for `/npub1…` and `/nprofile1…`: the profile from relays, the photo served by the service, Open Graph tags, `noindex`, the three directions, plain-text profile fields. `assetlinks.json` with the Android certificate. Stores nothing | M | 2; decision 6 |
| 4 | **Core: links on the new host.** `shareLink` becomes `https://on.peoples.chat/npub1…`. The parser reads the new forms, with a token after `#`, and keeps every old form. Update `docs/core-interface.md` | S | Decision 1 |
| 5 | **Shell: open the new links.** The deep-link entry for `on.peoples.chat`, the scheme `peopleschat` on phones and desktop, checked on the Android emulator with the app closed and running | S | 4 |
| 6 | **Interface: a link opens a profile sheet,** with name, photo and Start chat, in place of New chat with the link filled in | M | 4, 5 |
| 7 | **QR codes.** Profile's code carries `https://on.peoples.chat/npub1…`; a second code "For White Noise" carries `marmot://profile/<npub>`. The scanner reads both, and the old form | S | 4; decision 3 |
| 8 | **Personal invitations on the new links:** `<link>#<token>`, with the token and rules unchanged | S | 4 |
| 9 | **Links service: shortkeys.** The table, signed requests (NIP-98) to make and reset, JSON for the app, limits per address in memory, "This link was reset" | M | 3 |
| 10 | **Core and interface: short links.** Make one at first share, reset it in Settings, resolve shortkeys in `addContact` and `previewInvite`, share the key link when the service can't be reached | M | 9; decision 4 |
| 11 | **Links service: handles.** The table, the rules and reserved list, claim, change and release, `/@handle` pages, NIP-05 with the CORS header | M | 9; decisions 2, 5 |
| 12 | **Website: pass `/.well-known/nostr.json` on `peoples.chat` to the service** without a redirect, and check it with White Noise's two phone apps | S | 11 |
| 13 | **Core and interface: handles.** Claim, change and release on Profile; write `nip05` into the profile; read `@name` and `/@name` links | M | 11, 1 |
| 14 | **iPhones: `apple-app-site-association` on `on.peoples.chat`,** the team in the build, and a check on an iPhone that a link opens the app and that the part after `#` arrives. Extends #99 | S | 3, 5; #61 |
| 15 | **Spike: links that open the desktop app on macOS:** the scheme `peopleschat`, and whether universal links work in a Tauri build | S | 5 |
| 16 | **Privacy and terms page for the service,** with the relay's (#90), and a contact address for reports | S | Decision 10 |
| 17 | **Docs: the spec's "Services the project operates"** gains the links service, and loses "None of these holds identities" | S | Decision 2 |
| 18 | **Write to White Noise:** propose that Marmot apps read `https://<host>/<npub>` profile links, with the code references from this document | S | Decision 8 |
| 19 | **Spike: group links.** The join request's event, the admin's side, several admins behind one link, the page. Ends in a proposal for Marmot | M | Group chats in the core; 9; decision 7 |
| 20 | **Android: keep the link through an install** with the Play Install Referrer | S | The app on Google Play; 5 |

Tasks 1 to 8 need no stored data and no decision about handles. Task 1 needs nothing at all.

## Decisions for Brian

1. **Domain.** Is `on.peoples.chat` the address for links, or do you want another subdomain (`to.`, `go.`, `join.`,
   `p.`), links on `peoples.chat` itself, or a separate short domain that you would buy?
2. **A service that holds identities.** May the project run a service that stores which handle and which short link
   belong to which public key, although the spec says none of its services holds identities? Who runs it (#11): the
   relay's operator, on the relay's server, or on a server of its own for about €5.50 a month?
3. **The QR code.** Should Profile's QR code become the web link `https://on.peoples.chat/npub1…`, which any phone's
   camera opens but White Noise's scanner refuses, with a second code for White Noise? Or should it stay a code that
   White Noise scans and a camera can't use?
4. **Short links by default.** Should the app make a short link for every user the first time they share, without
   asking, or only when the user asks for one? The first tells the service about every account that shares a link.
5. **Handles.** Do you reopen the decision of 22 November 2026 to leave usernames out? If so:
   - plain unique names, first come, first served (`emil`), or names that must end in digits, as on Signal
     (`emil.42`), which takes the value out of squatting?
   - is 4 the shortest length, and 90 days the wait before a released name can be claimed again?
   - may the project take back a name used to pass as someone else?
6. **What the page shows.** Name, about text and photo, or the name only? And may search engines index the pages?
   The recommendation is all three, not indexed.
7. **Group links.** When someone opens a group link, does the admin's app add them by itself, or does the admin
   confirm each person? And may the page show the group's name and picture, which are otherwise not public?
8. **White Noise.** May we ask White Noise to read profile links on other hosts, such as `https://on.peoples.chat/<npub>`?
   And is #76, making our accounts findable from White Noise, to be settled first? Without it, a White Noise user who
   opens our link finds the profile but may not be able to start the chat.
9. **Where the service's code lives.** In this repository, in a folder beside `relay/`, or in a repository of its
   own?
10. **Legal review.** Who reviews the privacy note and terms for the links service and the relay (#90)?
