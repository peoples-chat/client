# Test #49: one chat survives an outage on the bench

The proof of concept's headline scenario. It runs twice: with mock radios as an automated test, before the radios
arrive; and with two phones and two radios on a desk, on hardware day, before the field test.

**Mock version: passes three times in a row.** Run on 22 November 2026, branch `49-bench`. Each run took 19.4 to
19.5 seconds.

## The steps

| Step | On the bench | In the mock test |
| --- | --- | --- |
| 1 | Both phones online: a message each way over the relays | The same, through the test relay |
| 2 | Cut the internet on both. The chat flips to radio by itself | The relay is blocked. Each core hears the other's radio advertise, and the chat goes to radio |
| 3 | Ten messages each way over radio, with the receiving phone locked | Ten each way, each confirmed by the other radio. No phone to lock |
| 4 | Switch one radio off, write a message, and see it wait | Ada's radio disconnects. Her message is waiting, and can be cancelled |
| 5 | Restore the internet. The waiting message sends and the radio messages are republished | The relay is unblocked, with the same result |
| 6 | Compare the two histories: identical, no duplicates, each message marked with how it travelled | 23 messages on each phone, the same ids, times and transports, also after both restart |

## Running it

```sh
cargo test -p peoples-chat-core --test bench
```

The test is `core/tests/bench.rs`. It uses two cores with their own folders, mock radios on one mock mesh, and the test
relay. The test relay goes on answering NIP-11 requests while it is blocked. Otherwise, the phone that comes back can
get what went over the relays minutes late, which is #78. On the bench the internet goes away for real, so #78 can
show there.

## Results of the mock version

| Run | Result | Time |
| --- | --- | --- |
| 1 | Pass | 19.45 s |
| 2 | Pass | 19.40 s |
| 3 | Pass | 19.51 s |

## Hardware day

What the bench adds to the mock:

- real radios: pairing (#24), loading the contact (#35) and the MeshCore path (#36);
- real adverts deciding when the chat goes to radio (#44), and the hour that a radio counts as heard;
- the receiving phone locked (#25 to #28);
- the airtime of twenty messages, against the cap of #51;
- #78 on real relays.

**The record.** Film the two screens through the steps; the recording is also the fallback video for the demo. Note
the time each radio message takes to arrive and to be confirmed.
