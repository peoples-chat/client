#!/usr/bin/env python3
"""Test vectors for the radio message format v0 (docs/radio-format-v0.md, #38).

A reference written apart from the core, with nothing but the standard library, so that the
core's implementation (#39) can be checked against something it didn't produce. Each primitive
first checks itself against a published vector: ChaCha20-Poly1305 against RFC 8439, HKDF against
RFC 5869, and the event id against MDK's own NIP-01 test.

Usage: python3 -I scripts/radio_vectors.py
"""

import base64
import hashlib
import hmac
import json
import struct

# --- ChaCha20-Poly1305, RFC 8439 ---------------------------------------------------------------


def _rotl(value, count):
    return ((value << count) & 0xFFFFFFFF) | (value >> (32 - count))


def _quarter_round(x, a, b, c, d):
    x[a] = (x[a] + x[b]) & 0xFFFFFFFF
    x[d] = _rotl(x[d] ^ x[a], 16)
    x[c] = (x[c] + x[d]) & 0xFFFFFFFF
    x[b] = _rotl(x[b] ^ x[c], 12)
    x[a] = (x[a] + x[b]) & 0xFFFFFFFF
    x[d] = _rotl(x[d] ^ x[a], 8)
    x[c] = (x[c] + x[d]) & 0xFFFFFFFF
    x[b] = _rotl(x[b] ^ x[c], 7)


def chacha20_block(key, counter, nonce):
    state = [0x61707865, 0x3320646E, 0x79622D32, 0x6B206574]
    state += list(struct.unpack("<8I", key)) + [counter] + list(struct.unpack("<3I", nonce))
    x = state[:]
    for _ in range(10):
        _quarter_round(x, 0, 4, 8, 12)
        _quarter_round(x, 1, 5, 9, 13)
        _quarter_round(x, 2, 6, 10, 14)
        _quarter_round(x, 3, 7, 11, 15)
        _quarter_round(x, 0, 5, 10, 15)
        _quarter_round(x, 1, 6, 11, 12)
        _quarter_round(x, 2, 7, 8, 13)
        _quarter_round(x, 3, 4, 9, 14)
    return struct.pack("<16I", *[(x[i] + state[i]) & 0xFFFFFFFF for i in range(16)])


def chacha20(key, counter, nonce, data):
    out = bytearray()
    for start in range(0, len(data), 64):
        block = chacha20_block(key, counter + start // 64, nonce)
        out += bytes(a ^ b for a, b in zip(data[start : start + 64], block))
    return bytes(out)


def poly1305(message, key):
    r = int.from_bytes(key[:16], "little") & 0x0FFFFFFC0FFFFFFC0FFFFFFC0FFFFFFF
    s = int.from_bytes(key[16:], "little")
    p = (1 << 130) - 5
    acc = 0
    for start in range(0, len(message), 16):
        acc = (acc + int.from_bytes(message[start : start + 16] + b"\x01", "little")) * r % p
    return ((acc + s) % (1 << 128)).to_bytes(16, "little")


def _pad16(data):
    return data + b"\x00" * (-len(data) % 16)


def seal(key, nonce, plaintext, aad):
    """Ciphertext and tag."""
    one_time_key = chacha20_block(key, 0, nonce)[:32]
    ciphertext = chacha20(key, 1, nonce, plaintext)
    mac_data = _pad16(aad) + _pad16(ciphertext) + struct.pack("<QQ", len(aad), len(ciphertext))
    return ciphertext, poly1305(mac_data, one_time_key)


def open_(key, nonce, ciphertext, tag, aad):
    """The plaintext, or None when the tag doesn't match."""
    one_time_key = chacha20_block(key, 0, nonce)[:32]
    mac_data = _pad16(aad) + _pad16(ciphertext) + struct.pack("<QQ", len(aad), len(ciphertext))
    if not hmac.compare_digest(poly1305(mac_data, one_time_key), tag):
        return None
    return chacha20(key, 1, nonce, ciphertext)


def _check_rfc8439():
    # Section 2.8.2.
    plaintext = (
        b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for "
        b"the future, sunscreen would be it."
    )
    aad = bytes.fromhex("50515253c0c1c2c3c4c5c6c7")
    key = bytes(range(0x80, 0xA0))
    nonce = bytes.fromhex("070000004041424344454647")
    ciphertext, tag = seal(key, nonce, plaintext, aad)
    assert ciphertext[:16].hex() == "d31a8d34648e60db7b86afbc53ef7ec2", ciphertext.hex()
    assert tag.hex() == "1ae10b594f09e26a7e902ecbd0600691", tag.hex()
    assert open_(key, nonce, ciphertext, tag, aad) == plaintext


# --- HKDF-SHA256, RFC 5869 ----------------------------------------------------------------------


def hkdf(ikm, info, length=32, salt=b""):
    prk = hmac.new(salt or b"\x00" * 32, ikm, hashlib.sha256).digest()
    out, block, counter = b"", b"", 1
    while len(out) < length:
        block = hmac.new(prk, block + info + bytes([counter]), hashlib.sha256).digest()
        out += block
        counter += 1
    return out[:length]


def _check_rfc5869():
    # Test case 3: no salt, no info.
    okm = hkdf(bytes.fromhex("0b" * 22), b"", 42)
    assert okm.hex() == (
        "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
    )


# --- The message id: a Nostr event id, NIP-01 ---------------------------------------------------

CHAT_KIND = 9


def event_id(pubkey_hex, created_at, text):
    """The id of the kind-9 event MDK signs for a text without mentions, so without tags."""
    preimage = json.dumps(
        [0, pubkey_hex, created_at, CHAT_KIND, [], text], separators=(",", ":"), ensure_ascii=False
    )
    return hashlib.sha256(preimage.encode()).hexdigest()


def _check_event_id():
    # MDK's canonical_id_matches_nip01 in crates/traits/src/app_event.rs.
    expected = "da824251ccc2eb3ce8a9e9994e9b0683d520a9729c3611992f8b143cc806ba19"
    assert event_id("00" * 32, 0, "") == expected


# --- The format ---------------------------------------------------------------------------------

LABEL = b"peoples.chat/radio/v0"
VERSION = 0
TEXT = 0
HEADER = struct.Struct(">BBHI")
TAG_BYTES = 16
MAX_CARRIER_CHARS = 160


def radio_key(chat_secret, sender_hex):
    """The vectors' fixed key for what `sender` sends. A real radio key is random (#128); these
    are made as spike #37 once made them, so that the vectors stay as they were."""
    return hkdf(chat_secret, LABEL + bytes.fromhex(sender_hex))


def nonce(counter):
    return b"\x00" * 4 + struct.pack(">Q", counter)


def seal_text(key, epoch, counter, created_at, text):
    header = HEADER.pack(VERSION << 4 | TEXT, epoch & 0xFF, counter & 0xFFFF, created_at)
    ciphertext, tag = seal(key, nonce(counter), text.encode(), header)
    return header + ciphertext + tag


def open_text(key, packet, counter):
    """The text and its send time, or None. `counter` is the full counter the receiver rebuilt."""
    header, body = packet[: HEADER.size], packet[HEADER.size :]
    version_type, _, low, created_at = HEADER.unpack(header)
    if version_type != VERSION << 4 | TEXT or low != counter & 0xFFFF:
        return None
    text = open_(key, nonce(counter), body[:-TAG_BYTES], body[-TAG_BYTES:], header)
    return None if text is None else (text.decode(), created_at)


def full_counter(highest, low):
    """The full counter whose low 16 bits are `low`, closest to the one after `highest`."""
    expected = highest + 1
    candidate = (expected & ~0xFFFF) | low
    if candidate - expected > 0x8000:
        candidate -= 0x10000
    elif expected - candidate > 0x8000:
        candidate += 0x10000
    return candidate


def carrier(packet):
    """How the packet travels in a MeshCore text message: base64 without padding."""
    return base64.b64encode(packet).decode().rstrip("=")


def from_carrier(text):
    return base64.b64decode(text + "=" * (-len(text) % 4))


def max_text_bytes():
    packet = MAX_CARRIER_CHARS * 3 // 4
    return packet - HEADER.size - TAG_BYTES


# --- Vectors ------------------------------------------------------------------------------------

ADA = "aa" * 32
BEA = "bb" * 32
SECRET = bytes(range(32))


def main():
    _check_rfc8439()
    _check_rfc5869()
    _check_event_id()

    ada_sends = radio_key(SECRET, ADA)
    bea_sends = radio_key(SECRET, BEA)
    print("## Keys (epoch 7, the chat secret 00 01 … 1f)\n")
    print(f"- Ada sends with: `{ada_sends.hex()}`")
    print(f"- Bea sends with: `{bea_sends.hex()}`\n")

    cases = [
        ("Ada to Bea", ada_sends, ADA, 7, 0, 1795300000, "Hello Bea"),
        ("Bea to Ada", bea_sends, BEA, 7, 300, 1795300060, "Hej! Ses vi i morgen? ☔"),
        ("Ada to Bea, epoch 263, after 70,000 messages", ada_sends, ADA, 263, 70000, 1795300120, "Still here."),
    ]
    for title, key, sender, epoch, counter, created_at, text in cases:
        packet = seal_text(key, epoch, counter, created_at, text)
        assert open_text(key, packet, counter) == (text, created_at)
        assert from_carrier(carrier(packet)) == packet
        print(f"## {title}\n")
        print(f"- Text: {json.dumps(text, ensure_ascii=False)} ({len(text.encode())} bytes)")
        print(f"- Epoch {epoch}, counter {counter}, sent at {created_at}")
        print(f"- Nonce: `{nonce(counter).hex()}`")
        print(f"- Packet ({len(packet)} bytes): `{packet.hex()}`")
        print(f"- In the text message ({len(carrier(packet))} characters): `{carrier(packet)}`")
        print(f"- Message id: `{event_id(sender, created_at, text)}`\n")

    # The longest text that fits in one MeshCore text message.
    longest = "x" * max_text_bytes()
    packet = seal_text(ada_sends, 7, 1, 1795300180, longest)
    assert len(carrier(packet)) == MAX_CARRIER_CHARS
    assert len(carrier(seal_text(ada_sends, 7, 1, 1795300180, longest + "x"))) > MAX_CARRIER_CHARS
    print(f"## Budget\n\n- The longest text: {max_text_bytes()} bytes, {len(carrier(packet))} characters.\n")

    # Packets that must be refused.
    packet = bytearray(seal_text(ada_sends, 7, 0, 1795300000, "Hello Bea"))
    reflected = open_text(bea_sends, bytes(packet), 0)
    packet[-1] ^= 1
    forged = open_text(ada_sends, bytes(packet), 0)
    assert reflected is None and forged is None
    print("## Refused\n")
    print("- The first packet opened with Bea's sending key, as if reflected back to Ada: refused.")
    print("- The first packet with the tag's last bit flipped: refused.\n")

    print("## Rebuilding the counter\n")
    for highest, low in [(0, 1), (65530, 3), (70000, 4464), (70000, 4000)]:
        print(f"- Highest seen {highest}, low bits {low}: {full_counter(highest, low)}")


if __name__ == "__main__":
    main()
