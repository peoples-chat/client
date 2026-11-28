<!--
DRAFT for review (#90). Not published, and not yet reviewed against Danish and EU rules for hosting
services. Values in [BRACKETS] are placeholders.
-->

# Terms of use: peoples.chat relay

The peoples.chat relay at `wss://relay.peoples.chat` is run by [OPERATOR NAME AND ADDRESS] ("we").
Contact: [CONTACT EMAIL]. By connecting to it you accept these terms.

## What it is for

The relay carries encrypted messaging events for peoples.chat and other apps that use the Marmot protocol. It takes
only the kinds of event that such messaging needs, and refuses all others. It is free to use.

## What it keeps

Group messages and invitations are deleted after 90 days, or at their own expiry time if that is sooner. Profiles,
relay lists and invitation keys stay until their author replaces or deletes them. The
[privacy note](privacy.md) says what the relay sees and keeps.

## Your part

Don't use the relay to break the law, to send spam, or to overload it. The relay limits how many events one network
address can send per minute.

## No guarantee

We aim to keep the relay running, but offer it as it is, without a promise that it is always available or that it
keeps any event. Apps are meant to use several relays at once, so that one relay being down doesn't stop a chat. As
far as the law allows, we are not liable for losses from using the relay or from it being unavailable.

## Reporting illegal content

The relay can't read messages, but anyone can report content they believe is illegal to [CONTACT EMAIL]. Give the
event's id, and say why it is illegal. We look at each report, may remove the event or block its author's key, and
tell you what we did.

## Changes

We may change these terms, or stop running the relay, and say so here first. The date below shows the last change.

Danish law applies. Last changed: [DATE OF PUBLICATION].
