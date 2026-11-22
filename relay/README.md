# The peoples.chat relay

The relay the project runs, as one of the app's three default relays (#90). Its spec is the draft of 22 November 2026
in #90: it takes only what Marmot messaging needs, keeps no network addresses, and runs on one small Hetzner server.
This folder holds what the spec's deployment steps write by hand, as files, and a script that installs them.

It runs at `wss://relay.peoples.chat` since 22 November 2026, on a Hetzner server in Helsinki: 1 vCPU, 1 GB of memory,
Ubuntu 26.04. It isn't one of the app's defaults yet ([Making it a default](#making-it-a-default)).

| File | Installed as | What it does |
| --- | --- | --- |
| `strfry.conf` | `/etc/strfry.conf` | strfry: 2 MiB events, NIP-42, the write policy, the operator in NIP-11 |
| `write-policy.py` | `/etc/strfry/write-policy.py` | Takes Marmot's event kinds only, checks their tags, limits each address |
| `strfry.service` | `/etc/systemd/system/strfry.service` | Runs strfry as its own user, with a read-only system |
| `Caddyfile` | `/etc/caddy/Caddyfile` | TLS, and keeps `/metrics` private. Writes no access log |
| `strfry-retention` | `/etc/cron.daily/strfry-retention` | Deletes group messages and gift wraps after 90 days |
| `setup.sh` | | Installs all of it on a fresh Ubuntu server: 24.04 and 26.04 are tested |

## Setting it up

1. **Create the server** (the spec's step 1): Hetzner Cloud in Helsinki, with backups and a firewall that lets in
   TCP 22, 80 and 443. 1 GB of memory is enough to run the relay.
2. **Point the host name at it:** `A` and `AAAA` records. Wait until `dig +short <host>` answers, because Caddy needs
   them for the certificate.
3. **Install:**

   ```sh
   scp -r relay root@<host>:/root/relay
   ssh root@<host> 'sh /root/relay/setup.sh <host>'
   ```

   It builds strfry 1.1.3 from source, which needs about 2.5 GB of memory. A server with less gets a 3 GB swap
   file, which stays. On the project's server the build took about 30 minutes. Running the script again updates the
   configuration and restarts strfry.
4. **Check it** from this repository, against the relay alone:

   ```sh
   cargo run -p peoples-chat-core --example relay_check -- wss://<host>
   ```

5. **Add an uptime check** that runs somewhere else and alerts by email or phone (the spec's step 10).

## Checking a relay

`core/examples/relay_check.rs` runs the spec's acceptance tests that need no other app:

1. two throwaway accounts that know only this relay set up, become contacts and exchange messages, then are deleted;
2. a public note (kind 1) is refused;
3. a group message of 1.5 MB is taken, and one of 2.2 MB refused;
4. a gift wrap reaches its recipient, and it says who else can read it;
5. an event is gone once its expiry time has passed.

It works on any relay. Check 2 passes only on one that restricts what it takes.

## What testing changed

The files were tested on 22 November 2026, first in an Ubuntu 24.04 container with systemd, on Arm, then on the
project's server. There, all five checks passed against `wss://relay.peoples.chat`. Caddy got a Let's Encrypt
certificate by itself and answers 404 for `/metrics`. These parts differ from the spec:

- **Kind 3 is accepted.** MDK publishes a follow list (kind 3) with the profile when it sets an account up. Without
  it the app can't publish the account ("Could not publish the account").
- **Gift wraps are served to anyone,** which the spec gives as the fallback in its first open question. The content
  stays encrypted, but anyone can see that an account received some.
  - strfry 1.1.3 has no `restrictedReadKinds` or `restrictReadToInvolvedPubkey`, and ignores them without a word.
    Only strfry's master branch has them (`b6f9284`, after 1.1.3).
  - With master and those settings, invitations didn't arrive within a minute, although strfry logged that MDK
    authenticated. strfry asks for NIP-42 only as it closes a request with `auth-required`, and nothing was served
    after that, not even to a client of our own that authenticated as the recipient. Why is not settled.
- **The build runs one job per CPU,** with a swap file on a small server. With `-j4` in 3.7 GB of memory the compiler
  was killed.
- **Caddy from Ubuntu's archive is 2.6.2.** It takes the Caddyfile and gets certificates by itself; newer releases
  come from Caddy's own package repository.
- **`info` has no terms or privacy links yet,** and `contact` and `pubkey` are empty until the open questions below
  are settled.

## Making it a default

Once the relay passes its checks, in `core/src/messaging.rs`:

- put its address in `DEFAULT_RELAYS` instead of one of the three there, so that no two defaults share an operator;
- add it to `PROJECT_RELAYS`, so that Settings → Privacy → Relays and servers shows it as run by peoples.chat (#88).

Accounts created before then keep the relays they were set up with.

## Still open

From the spec, before the relay becomes a default:

- **Retention:** is 90 days right, given disk use and how long phones stay offline?
- **Terms and privacy pages, and a contact address:** the relay is a hosting service in the EU.
- **The operator's Nostr key** for `info.pubkey`.
- **White Noise:** a chat between White Noise and the app through this relay alone.
