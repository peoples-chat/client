<!--
DRAFT for review (#90). Not published, and not yet reviewed against Danish and EU rules.
Values in [BRACKETS] are placeholders. Each statement below follows the files in this folder as of
27 November 2026; change this note whenever they change.
-->

# Privacy note: peoples.chat relay

The peoples.chat relay at `wss://relay.peoples.chat` passes encrypted messages between people who use
peoples.chat or another Marmot app. It is run by [OPERATOR NAME AND ADDRESS]. Contact: [CONTACT EMAIL].

## What the relay can and cannot see

Messages, invitations and group changes reach the relay already encrypted on the sender's device. The relay
cannot read them, and holds no keys that could.

What it does see:

- **Public profiles and lists** that apps publish so others can find you: your name and photo link, your
  relay lists, and the keys others need to invite you to a chat. These are public by design, on this relay as
  on any other.
- **Who an invitation is for.** An invitation carries its recipient's public key, and anyone can ask this relay
  for the invitations to a given key. They stay encrypted, but it can be seen that an account received some.
- **Which group a message belongs to**, by a random group identifier, and when it was sent. Not who sent it, and
  not what it says.
- **Your network address,** while your app is connected.

The relay has no accounts, and keeps no names, phone numbers, email addresses or contact lists of its own.

## How long it keeps things

| What | Kept for |
| --- | --- |
| Group messages and invitations | 90 days, or until their own expiry time if that is sooner |
| Profiles, relay lists and invitation keys | Until the person replaces or deletes them |
| Network addresses | Not in an access log. The relay software's system log can contain them, and is kept for 7 days. The rate limit holds an address in memory for about a minute |
| Server backups | The hosting provider's automatic backups hold a copy of the server for up to 7 days |

## Where it runs

On one server at Hetzner Online GmbH in Helsinki, Finland, inside the EU. Nobody else receives data from the
relay. Other relays that your app uses are run by others, under their own terms.

## Your rights

You can delete what you published by deleting it in your app, which sends a deletion request; the relay acts on it.
For anything else, including access, correction or erasure under the GDPR, write to [CONTACT EMAIL]. Because the
relay has no accounts, it can find your data only by your public key. You can also complain to the Danish Data
Protection Agency (Datatilsynet).

Last changed: [DATE OF PUBLICATION].
