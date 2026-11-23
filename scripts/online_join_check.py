#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""online_join_check.py -- real Roblox join + RakNet crypto handshake probe.

What this script does
=====================

1. HTTP join (``join`` section)
   - authenticates with a ``.ROBLOSECURITY`` cookie (``--cookie`` or the
     ``ROBLOSECURITY`` environment variable; never hardcoded),
   - fetches the one-time authentication ticket, then asks
     ``gamejoin.roblox.com`` for a running server, with the CSRF dance and
     the same endpoint fallbacks as ``src/roblox/join.rs``,
   - extracts the crypto material from the join script:
       * ``ClientPublicKeyData``  -> applications.RakNetEarlyPublicKey
                                     versions[id=5].value (32-byte b64)
       * ``EphemeralEarlyPubKey``  -> 32-byte b64, URL-encoded
       * ``ClientTicket`` part2    -> 33-byte paBlob, part4 -> authVer
       * ``TokenValue``            -> 16-byte RUPP routing tag,
       * MachineAddress / ServerPort / UdmuxEndpoints,
   - prints ``joinTicket.EncryptedServerFields`` for debugging (opaque, not
     required by this flow).

2. UDP wire (``wire`` section), byte-exact against live captures
   - OCR1 : 31-byte RUPP prefix + ``7b 00 ff ff 00`` MTU probe, 1160 bytes.
   - ORP1 : 32-byte reply, last two bytes = MTU (1200).
   - OCR2 : prefix + 90-byte envelope header + 93-byte ChaCha20-Poly1305 box
           (nonce ``UniqueNumber``, AAD = the 90-byte header) padded to 520.
           Key = SESSDER#1(eph_priv, eph_pub, seed1).out2.
   - ORP2 : 158 bytes; the per-join *second* server value sits 21 bytes after
            the ``7d 00 ff ff 00`` magic (right after ``01 2c 00 3a``).
   - CREQ : prefix + ``ct || counter_hint(u16 LE) || tag(16)`` -- an
            AES-256-GCM box (nonce ``LE64(0x754e657571696e55+ctr) || "mbeR"``,
            empty AAD), 34-byte plaintext reused from a live capture with the
            per-session 8-byte id patched in.  Key = SESSDER#2(...).out2.
   - Every steady reply is opened with the derived rx key; a successful
            GCM tag check is the proof that the key schedule is right.

3. ``--selftest`` replays the published capture vectors through the crypto
   primitives (SESSDER, ChaCha, AES-GCM steady boxes, ORP2 parsing, OCR2/OCR1
   byte-exact construction).

Crypto, as confirmed from build 2.748.1527 (see docs/SESSION-CRYPTO-RE.md):

    Z   = X25519(priv = clamp(eph_priv), pub = server_pub)
    H   = BLAKE2b-512(Z || eph_pub || server_pub)
    out1, out2 = H[0:32], H[32:64]          # out2 is the key actually used

    envelope layer (OCR2): RFC 8439 ChaCha20-Poly1305, nonce "UniqueNumber",
                           AAD = the whole 90-byte header.
    steady   layer (CREQ+): AES-256-GCM, nonce LE64(BASE+ctr)+"mbeR", AAD
                           empty, container ct||hint||tag.  NOTE: this is
                           *standard* GCM (tag over the ciphertext), not the
                           tag-over-plaintext variant described in an earlier
                           note -- the vectors below prove which one is real.

No credentials or keys are hardcoded.  The baked-in bytes are wire *structure*
templates plus published capture vectors used only by ``--selftest``.

Usage
=====

    /storage/rblx_debug/venv/bin/python scripts/online_join_check.py --selftest
    ROBLOSECURITY=... /storage/rblx_debug/venv/bin/python \\
        scripts/online_join_check.py --place-id 606849621 --debug
    ... --server 128.116.44.33:49861 --seed1 <hex-or-b64>   # skip the web join
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import http.client
import ipaddress
import json
import os
import re
import socket
import ssl
import struct
import sys
import time
import urllib.parse

# ---------------------------------------------------------------------------
# third-party: `cryptography` (present in /storage/rblx_debug/venv)
# ---------------------------------------------------------------------------
try:
    from cryptography.exceptions import InvalidTag
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.x25519 import (
        X25519PrivateKey,
        X25519PublicKey,
    )
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM, ChaCha20Poly1305
except ImportError as exc:  # pragma: no cover - environment problem
    sys.exit(
        "online_join_check.py needs the 'cryptography' package:\n"
        "  /storage/rblx_debug/venv/bin/python %s --selftest\n"
        "  (original error: %s)" % (os.path.basename(__file__), exc)
    )


# ===========================================================================
# 0. constants and logging
# ===========================================================================

CLIENT_UA = "Roblox/WinInet"
GAMEJOIN_BASE = "https://gamejoin.roblox.com"
AUTH_TICKET_URL = "https://auth.roblox.com/v1/authentication-ticket"

#: RakNet steady-layer nonce base: ASCII "UniqueNu" read as a LE64.
STEADY_BASE = 0x754E657571696E55

#: Nonce suffixes.  Empirically (live pcaps raknet27/raknet29 + hook_v29) the
#: *steady* layer uses capital-R "mbeR" in BOTH directions; the envelope
#: ChaCha layer uses lower-case "mber" ("UniqueNumber").
STEADY_SUFFIX = b"mbeR"
ENVELOPE_NONCE = b"UniqueNumber"

#: RakNet password tail of CONNECTION_REQUEST (matches RBX_PASSWORD in
#: src/net/raknet/handshake/mod.rs).  Part of the wire format, not a secret.
RBX_PASSWORD = bytes.fromhex("374f5e116c45")

DEBUG = False


def log(msg: str) -> None:
    print(msg, flush=True)


def dbg(msg: str) -> None:
    if DEBUG:
        print(msg, flush=True)


def hexs(data: bytes, limit: int | None = None) -> str:
    s = data.hex()
    if limit is not None and len(s) > limit * 2:
        return s[: limit * 2] + "...(%d bytes)" % len(data)
    return s


def dump(label: str, data: bytes, width: int = 64) -> None:
    """Hex-dump a buffer, debug only."""
    if not DEBUG:
        dbg("%s (%d bytes) %s" % (label, len(data), hexs(data, 32)))
        return
    log("%s (%d bytes):" % (label, len(data)))
    h = data.hex()
    for i in range(0, len(h), width * 2):
        log("  " + h[i : i + width * 2])


def b64d_loose(text: str) -> bytes:
    """base64 decode tolerant of URL-safe chars / missing padding."""
    t = text.strip().replace("-", "+").replace("_", "/")
    t += "=" * (-len(t) % 4)
    try:
        return base64.b64decode(t)
    except (binascii.Error, ValueError):
        return b""


def b64e(data: bytes) -> str:
    return base64.b64encode(data).decode()


# ---------------------------------------------------------------------------
# 1. crypto
# ---------------------------------------------------------------------------


def x25519_pub(priv: bytes) -> bytes:
    """Raw 32-byte X25519 public key for a 32-byte private scalar."""
    key = X25519PrivateKey.from_private_bytes(priv)
    return key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )


def x25519(priv: bytes, peer_pub: bytes) -> bytes:
    """Raw X25519 shared secret (cryptography clamps the scalar per RFC 7748)."""
    return X25519PrivateKey.from_private_bytes(priv).exchange(
        X25519PublicKey.from_public_bytes(peer_pub)
    )


def sessder(eph_priv: bytes, eph_pub: bytes, server_pub: bytes) -> tuple[bytes, bytes]:
    """SESSDER: BLAKE2b-512(Z || eph_pub || server_pub) split into two keys.

    ``server_pub`` is the X25519 peer *and* the third hash input, exactly as
    the client does it:
      * envelope layer  -> the server's long-lived early pub (join script),
      * steady layer    -> the second per-join value carried by ORP2.

    out1 is the opposite direction, out2 is what the client seals with.
    """
    if len(eph_priv) != 32 or len(eph_pub) != 32 or len(server_pub) != 32:
        raise ValueError("SESSDER needs three 32-byte values")
    z = x25519(eph_priv, server_pub)
    h = hashlib.blake2b(z + eph_pub + server_pub, digest_size=64).digest()
    return h[:32], h[32:]


def chacha_seal(key: bytes, nonce: bytes, pt: bytes, aad: bytes = b"") -> tuple[bytes, bytes]:
    """RFC 8439 ChaCha20-Poly1305; returns (ciphertext, tag)."""
    out = ChaCha20Poly1305(key).encrypt(nonce, pt, aad)
    return out[:-16], out[-16:]


def chacha_open(key: bytes, nonce: bytes, ct: bytes, tag: bytes, aad: bytes = b"") -> bytes | None:
    try:
        return ChaCha20Poly1305(key).decrypt(nonce, ct + tag, aad)
    except InvalidTag:
        return None


# --- standard AES-256-GCM (the steady layer) -------------------------------


def steady_nonce(counter: int, suffix: bytes = STEADY_SUFFIX) -> bytes:
    """12-byte GCM nonce: LE64(absolute counter) || "mbeR"."""
    return struct.pack("<Q", counter & 0xFFFFFFFFFFFFFFFF) + suffix


def steady_seal(key: bytes, counter: int, pt: bytes, aad: bytes = b"",
                suffix: bytes = STEADY_SUFFIX) -> bytes:
    """Seal one steady datagram body: ``ct || counter_hint(u16 LE) || tag``."""
    blob = AESGCM(key).encrypt(steady_nonce(counter, suffix), pt, aad)
    ct, tag = blob[:-16], blob[-16:]
    return ct + struct.pack("<H", counter & 0xFFFF) + tag


def steady_open(key: bytes, body: bytes, suffix: bytes = STEADY_SUFFIX,
                window: int = 16) -> tuple[bytes, int] | None:
    """Open a steady body; the wire only carries the low 16 bits of the
    counter, so scan a window of high bits around STEADY_BASE."""
    if len(body) < 18:
        return None
    ct, hint_raw, tag = body[:-18], body[-18:-16], body[-16:]
    hint = struct.unpack("<H", hint_raw)[0]
    delta = (hint - (STEADY_BASE & 0xFFFF)) & 0xFFFF
    for k in range(max(1, window)):
        counter = STEADY_BASE + delta + k * 0x10000
        try:
            return AESGCM(key).decrypt(steady_nonce(counter, suffix), ct + tag, b""), counter
        except InvalidTag:
            continue
    return None


# --- manual GCM (cross-check only; the fast path uses AESGCM) ---------------


def _aes_ecb_encrypt(key: bytes, block: bytes) -> bytes:
    enc = Cipher(algorithms.AES(key), modes.ECB()).encryptor()
    return enc.update(block) + enc.finalize()


def _gf_mul(x: int, y: int) -> int:
    """GF(2^128) multiply, GCM bit order."""
    r = 0xE1 << 120
    z = 0
    for i in range(128):
        if (y >> (127 - i)) & 1:
            z ^= x
        x = (x >> 1) ^ r if x & 1 else x >> 1
    return z


def _ghash(h: bytes, aad: bytes, ct: bytes) -> bytes:
    """GHASH(H, aad, ciphertext); standard BE64 length block."""
    def blocks(b: bytes) -> list[bytes]:
        b = b + b"\x00" * (-len(b) % 16)
        return [b[i : i + 16] for i in range(0, len(b), 16)]

    y = 0
    hi = int.from_bytes(h, "big")
    for blk in blocks(aad) + blocks(ct):
        y = _gf_mul(y ^ int.from_bytes(blk, "big"), hi)
    lens = (len(aad) * 8).to_bytes(8, "big") + (len(ct) * 8).to_bytes(8, "big")
    y = _gf_mul(y ^ int.from_bytes(lens, "big"), hi)
    return y.to_bytes(16, "big")


def aes_gcm_seal_manual(key: bytes, nonce: bytes, pt: bytes, aad: bytes = b"") -> bytes:
    """Textbook AES-GCM seal (CTR from nonce||2, tag over ct).  Slow but
    useful as an independent check of the library primitive."""
    ks = b""
    ctr = 2
    while len(ks) < len(pt):
        ks += _aes_ecb_encrypt(key, nonce + ctr.to_bytes(4, "big"))
        ctr += 1
    ct = bytes(a ^ b for a, b in zip(pt, ks))
    h = _aes_ecb_encrypt(key, b"\x00" * 16)
    j0 = _aes_ecb_encrypt(key, nonce + b"\x00\x00\x00\x01")
    tag = bytes(a ^ b for a, b in zip(j0, _ghash(h, aad, ct)))
    return ct + tag


def aes_gcm_open_manual(key: bytes, nonce: bytes, ct: bytes, tag: bytes,
                        aad: bytes = b"") -> bytes | None:
    h = _aes_ecb_encrypt(key, b"\x00" * 16)
    j0 = _aes_ecb_encrypt(key, nonce + b"\x00\x00\x00\x01")
    want = bytes(a ^ b for a, b in zip(j0, _ghash(h, aad, ct)))
    if want != tag:
        return None
    ks = b""
    ctr = 2
    while len(ks) < len(ct):
        ks += _aes_ecb_encrypt(key, nonce + ctr.to_bytes(4, "big"))
        ctr += 1
    return bytes(a ^ b for a, b in zip(ct, ks))


# ---------------------------------------------------------------------------
# 2. wire templates (structure only -- verified against live pcaps)
# ---------------------------------------------------------------------------

#: RUPP prefix (31 bytes):
#:   01 00 00 1f | 01 | 11 | <tokentype> | <16-byte udmux TokenValue> |
#:   02 06 | <RCC IPv4, 4 bytes> | <ServerPort BE16>
#: Confirmed twice: pcap raknet27 rec 250 uses 10.34.8.17:62781 while rec 251
#: uses 10.34.8.17:65481, and the Rust live log shows the same shape with
#: 10.9.2.82:49861.  The 16-byte token is the join script's udmux TokenValue.
RUPP_HDR = b"\x01\x00\x00\x1f\x01\x11"
RUPP_TOKEN_TYPE = 0x01
RUPP_RCC_TAG = b"\x02\x06"
PREFIX_LEN = 31

#: Client frame magic/constants used by OCR1 and the 90-byte OCR2 header.
OPEN_CONNECTION_REQUEST_1 = b"\x7b\x00\xff\xff\x00"
OPEN_CONNECTION_REQUEST_2 = b"\x78\x00\xff\xff\x00"
ENVELOPE_SEQ = b"\xfe\xfe\xfe\xfe"
ENVELOPE_SEQ2 = b"\xfd\xfd\xfd\xfd"
ENVELOPE_MARKER = b"\x12\x34\x56\x78"
OCR1_TAIL = b"\x05\x01\x00\x00\x00"      # protocol 5 + encryption epoch
OCR2_VER = b"\x03"
OCR2_AAD_LEN = b"\x5a\x00"               # 90
OCR2_BOX_LEN = b"\x5d\x00"               # 93 (plaintext length)
OCR2_SUPPORTED = b"\x05"
OCR2_PA_LEN = b"\x21"                    # 33

OCR1_WIRE_LEN = 1160
OCR2_WIRE_LEN = 1160
OCR2_HEADER_LEN = 90
OCR2_PT_LEN = 93

#: pt93 = const(8) || session id(8) || MTU BE16 || 0x02 || entropy(74).
PT93_CONST = bytes.fromhex("100ddbc750b30896")
CAPS_OVERRIDE = None  # set from --caps (the app's own 67-byte caps payload)

#: The steady-layer CONNECTION_REQUEST plaintext from a real capture (34 B):
#:   81 00 00 40 00 c0 00 00 00 00 09 | <session id 8> | 00*6 | <2> | 00 |
#:   37 4f 5e 11 6c 45                      <- the RakNet password tail
#: Only the 8-byte session id is patched at runtime.  The two unknown bytes
#: (here ``fd fc``) are kept verbatim -- see TODO(CREQ) below.
CREQ_PT_TEMPLATE = bytes.fromhex(
    "810000004000c0000000" "09" "bc1a859cb9a65a76"
    "000000000000" "fdfc" "00" "374f5e116c45"
)
CREQ_TEMPLATE_ID = bytes.fromhex("bc1a859cb9a65a76")


def build_prefix(token16: bytes, rcc_ip: str, server_port: int,
                 token_type: int = RUPP_TOKEN_TYPE) -> bytes:
    """Build the 31-byte client RUPP prefix."""
    if len(token16) != 16:
        token16 = (token16 + os.urandom(16))[:16]
    try:
        ip = socket.inet_aton(rcc_ip)
    except OSError:
        ip = b"\x00\x00\x00\x00"
    return (
        RUPP_HDR
        + bytes([token_type])
        + token16
        + RUPP_RCC_TAG
        + ip
        + struct.pack(">H", server_port & 0xFFFF)
    )[:PREFIX_LEN]


def build_ocr1(prefix: bytes) -> bytes:
    """OCR1: MTU probe, 1160 bytes on the wire (31-byte prefix included)."""
    body = (
        OPEN_CONNECTION_REQUEST_1
        + ENVELOPE_SEQ
        + ENVELOPE_SEQ2
        + ENVELOPE_MARKER
        + OCR1_TAIL
        + b"\x00" * 32
    )
    pkt = prefix + body
    if len(pkt) > OCR1_WIRE_LEN:
        raise ValueError("OCR1 template overflow")
    return pkt.ljust(OCR1_WIRE_LEN, b"\x00")


def build_ocr2_header(eph_pub: bytes, auth_ver: int, pa_blob: bytes) -> bytes:
    """The 90-byte envelope header (also the AEAD AAD).

    Byte map (verified byte-exact against pcap raknet27 rec 253):

        0..5    78 00 ff ff 00          id 0x78 + frame magic
        5..9    fe fe fe fe             sequence (constant in every capture)
        9..13   fd fd fd fd             sequence #2 (constant)
        13..17  12 34 56 78             envelope marker
        17      03                      envelope version
        18..20  5a 00                   AAD length = 90
        20..22  5d 00                   plaintext/box length = 93
        22      05                      supported crypto version
        23..55  eph_pub (32)            NOTE: header[23:55], not 21..53 --
                                        measured on the capture; the pub
                                        starts right after the 23-byte
                                        pre-buffer above
        55      12                      authVer (ClientTicket part4)
        56      21                      paBlob length = 33
        57..90  paBlob (33)             the join call's own ClientTicket part2

    So the only bytes treated as *constants* are the 17-byte frame prefix,
    the six header bytes 17..23 and the 0x21 length byte; the pub, authVer
    and paBlob all come from this run's join.
    """
    if len(eph_pub) != 32:
        raise ValueError("eph_pub must be 32 bytes")
    if len(pa_blob) != 33:
        raise ValueError("paBlob must be 33 bytes (ClientTicket part2)")
    hdr = bytearray()
    hdr += OPEN_CONNECTION_REQUEST_2
    hdr += ENVELOPE_SEQ
    hdr += ENVELOPE_SEQ2
    hdr += ENVELOPE_MARKER
    hdr += OCR2_VER
    hdr += OCR2_AAD_LEN
    hdr += OCR2_BOX_LEN
    hdr += OCR2_SUPPORTED
    hdr += eph_pub
    hdr += bytes([auth_ver & 0xFF])
    hdr += OCR2_PA_LEN
    hdr += pa_blob
    assert len(hdr) == OCR2_HEADER_LEN, len(hdr)
    return bytes(hdr)


def build_pt93(session_id: bytes, mtu: int, relay_port: int = 0) -> bytes:
    """93-byte OCR2 plaintext, byte-mapped from two independent live captures:

    [0:8]   100ddbc750b30896  capabilities constant
    [8:16]  session id        the same 8 bytes the CREQ carries
    [16:18] MTU (BE16, 0x04b0 = 1200)
    [18:20] 02 04             constant
    [20:22] 7f 8b             constant
    [22:24] noise
    [24:26] relay port (BE16) -- the port the client dials; v27: ffc9=65481,
            v35: e021=57377, both matching their captures' server port
    [26:93] capabilities payload + noise

    The relay port is the only session-bound field the server drains the
    packet on (verified live); a random value there gets the OCR2 dropped.
    """
    if len(session_id) != 8:
        raise ValueError("session id must be 8 bytes")
    pt = (
        PT93_CONST
        + session_id
        + struct.pack(">H", mtu & 0xFFFF)
        + b"\x02\x04"
        + b"\x7f\x8b"
        + os.urandom(2)
        + struct.pack(">H", relay_port & 0xFFFF)
        + (CAPS_OVERRIDE if CAPS_OVERRIDE else b"\x42\x06")
        + os.urandom(OCR2_PT_LEN - 28 - (len(CAPS_OVERRIDE) - 2 if CAPS_OVERRIDE else 0))
    )
    assert len(pt) == OCR2_PT_LEN, len(pt)
    return pt


def build_ocr2(prefix: bytes, header: bytes, ct: bytes, nonce: bytes,
               tag: bytes) -> bytes:
    pkt = prefix + header + ct + nonce + tag
    if len(pkt) > OCR2_WIRE_LEN:
        raise ValueError("OCR2 overflow: %d" % len(pkt))
    return pkt.ljust(OCR2_WIRE_LEN, b"\x00")


def parse_orp1(pkt: bytes) -> int | None:
    """ORP1 is 32 bytes: ... 12 34 56 78 | guid(8) | 00 | mtu BE16.

    (The byte between the guid and the MTU is always 0x00 -- read the MTU
    from the tail rather than assuming the exact guid width.)
    """
    if pkt.find(ENVELOPE_MARKER) < 0 or len(pkt) < 16:
        return None
    return struct.unpack(">H", pkt[-2:])[0]


def parse_orp2(pkt: bytes) -> bytes | None:
    """Return the 32-byte per-join server value from OpenReply2.

    Layout: <server prefix 23> | 7d 00 ff ff 00 | seq(8) | 12 34 56 78 |
            01 2c 00 3a | server value prefix(23, clear) | 9-byte tail INSIDE
            the sealed box | box ct | nonce(12) | tag(16)
    Only the first 23 bytes of the value are on the wire; the last 9 bytes
    sit at the start of the box plaintext (see orp2_seed2()).  The value is
    located relative to the magic, so a prefix change cannot shift it.
    """
    i = pkt.find(b"\x7d\x00\xff\xff\x00")
    if i < 0 or len(pkt) < i + 21 + 23:
        return None
    return pkt[i + 21 : i + 21 + 23]


def orp2_box(pkt: bytes) -> tuple[bytes, bytes, bytes, bytes] | None:
    """Extract (aad, ct, nonce, tag) of the ORP2 sealed box.

    aad = payload[23 : 23+44] (the envelope header incl. the 23-byte clear
    server value), ct/nonce/tag follow.  Key = envelope SESSDER#1 out1
    (the server->client direction).
    """
    i = pkt.find(b"\x7d\x00\xff\xff\x00")
    if i < 0:
        return None
    aad = pkt[23 : 23 + 44]
    rest = pkt[23 + 44 :]
    if len(rest) < 28:
        return None
    return aad, rest[:-28], rest[-28:-16], rest[-16:]


def orp2_seed2(pkt: bytes, env_out1: bytes) -> bytes | None:
    """Recover the complete 32-byte steady peer key: 23 clear bytes from the
    ORP2 header + the 9-byte tail that lives in the box plaintext[0:9]."""
    clear = parse_orp2(pkt)
    box = orp2_box(pkt)
    if clear is None or box is None:
        return None
    aad, ct, nonce, tag = box
    try:
        pt = chacha_open(env_out1, nonce, ct, tag, aad)
    except Exception:
        return None
    return clear + pt[:9]


# ===========================================================================
# 3. join (HTTP)
# ===========================================================================


class HttpSession:
    """Minimal HTTPS client: cookie + CSRF dance + client User-Agent."""

    def __init__(self, cookie: str, timeout: float, ua: str = CLIENT_UA):
        self.cookie = cookie
        self.timeout = timeout
        self.ua = ua
        self.csrf: dict[str, str] = {}

    def post(self, url: str, payload: dict, referer: str | None = None,
             extra: dict[str, str] | None = None) -> tuple[int, dict[str, str], bytes]:
        body = json.dumps(payload).encode()
        parsed = urllib.parse.urlsplit(url)
        host = parsed.netloc
        path = parsed.path + ("?" + parsed.query if parsed.query else "")
        last: tuple[int, dict[str, str], bytes] | None = None
        for attempt in range(2):
            headers = {
                "Accept": "application/json",
                "Content-Type": "application/json",
                "User-Agent": self.ua,
                "Cookie": ".ROBLOSECURITY=" + self.cookie,
            }
            if referer:
                headers["Referer"] = referer
                headers["Origin"] = "https://www.roblox.com"
            if host in self.csrf:
                headers["X-CSRF-Token"] = self.csrf[host]
            if extra:
                headers.update(extra)
            conn = http.client.HTTPSConnection(
                host, timeout=self.timeout, context=ssl.create_default_context()
            )
            try:
                conn.request("POST", path, body=body, headers=headers)
                resp = conn.getresponse()
                data = resp.read()
                hdrs = {k.lower(): v for k, v in resp.getheaders()}
            finally:
                conn.close()
            last = (resp.status, hdrs, data)
            token = hdrs.get("x-csrf-token")
            if resp.status in (401, 403) and token and attempt == 0:
                dbg("CSRF challenge from %s -- retrying with X-CSRF-Token" % host)
                self.csrf[host] = token
                continue
            break
        assert last is not None
        return last


def fetch_auth_ticket(sess: HttpSession, place_id: int) -> str | None:
    """One-time join ticket: empty JSON body, ticket in a response header."""
    referer = "https://www.roblox.com/games/%d/" % place_id
    status, hdrs, body = sess.post(AUTH_TICKET_URL, {}, referer=referer)
    text = body.decode("utf-8", "replace")
    log("auth ticket: HTTP %d (%d bytes body)" % (status, len(body)))
    if not 200 <= status < 300:
        log("auth ticket failed: %s" % text[:200])
        return None
    ticket = (hdrs.get("rbx-authentication-ticket") or "").strip()
    if not ticket:
        try:
            v = json.loads(text or "{}")
            for key in ("ticket", "authenticationTicket", "token"):
                if isinstance(v.get(key), str) and v[key].strip():
                    ticket = v[key].strip()
                    break
        except ValueError:
            if text.strip() and not text.lstrip().startswith("<"):
                ticket = text.strip()
    if ticket:
        log("auth ticket: received (%d chars)" % len(ticket))
    else:
        log("auth ticket: none in the response (joining cookie-only)")
    return ticket or None


def _join_endpoint(join_script: dict) -> tuple[str, int, str]:
    """(address, port, source) with the same priority order as join.rs."""
    for key in ("ServerConnections", "UdmuxEndpoints"):
        arr = join_script.get(key)
        if isinstance(arr, list):
            for item in arr:
                if not isinstance(item, dict):
                    continue
                ip = item.get("Address") or ""
                port = int(item.get("Port") or 0)
                if ip and 0 < port <= 65535:
                    return str(ip), port, key
    ip = join_script.get("MachineAddress") or ""
    for pk in ("ServerPort", "MachinePort", "ClientPort"):
        port = int(join_script.get(pk) or 0)
        if ip and 0 < port <= 65535:
            return str(ip), port, pk
    return "", 0, ""


def _udmux_endpoint(join_script: dict) -> tuple[str, int] | None:
    arr = join_script.get("UdmuxEndpoints")
    if isinstance(arr, list):
        for item in arr:
            if not isinstance(item, dict):
                continue
            ip = item.get("Address") or ""
            port = int(item.get("Port") or 0)
            if ip and 0 < port <= 65535:
                return str(ip), port
    return None


def _join_script_obj(resp: dict) -> dict:
    js = resp.get("joinScript")
    if isinstance(js, dict):
        return js
    if isinstance(js, str) and js.strip().startswith("{"):
        try:
            v = json.loads(js)
            if isinstance(v, dict):
                return v
        except ValueError:
            pass
    elif isinstance(js, str):
        # Classic XML-ish dialect: recover the handful of fields we need.
        out: dict = {}
        for tag in (
            "MachineAddress", "ServerPort", "MachinePort", "ClientPort",
            "ClientPublicKeyData", "EphemeralEarlyPubKey", "TokenValue",
            "UdmuxToken", "ClientTicket", "JobId", "SessionId",
        ):
            m = re.search(r"<%s>([^<]*)</%s>" % (tag, tag), js)
            if m:
                out[tag] = m.group(1).strip()
        if out:
            return out
    return {}


def _parse_keyring_value(raw) -> tuple[bytes | None, int | None, list]:
    """ClientPublicKeyData -> (32-byte key, version id, all versions)."""
    try:
        v = json.loads(raw) if isinstance(raw, str) else raw
    except ValueError:
        return None, None, []
    app = ((v or {}).get("applications") or {}).get("RakNetEarlyPublicKey") or {}
    versions = app.get("versions") or []
    chosen = None
    for item in versions:
        if isinstance(item, dict) and item.get("id") == 5:
            chosen = item
            break
    if chosen is None and versions:
        chosen = versions[0] if isinstance(versions[0], dict) else None
    if not chosen:
        return None, None, versions
    key = b64d_loose(str(chosen.get("value") or ""))
    return (key if len(key) == 32 else None), chosen.get("id"), versions


def _parse_client_ticket(join_script: dict) -> tuple[bytes | None, int | None]:
    """ClientTicket part2 (paBlob) and part4 (authVer) for OCR2."""
    ct = join_script.get("ClientTicket")
    if not isinstance(ct, str) or not ct.strip():
        return None, None
    parts = ct.split(";")
    pa = None
    ver = None
    if len(parts) > 2:
        blob = b64d_loose(parts[2].strip())
        if len(blob) == 33:
            pa = blob
        elif blob:
            log("join: ClientTicket part2 is %d bytes (expected 33) -- ignoring" % len(blob))
    if len(parts) > 4:
        try:
            ver = int(parts[4].strip())
        except ValueError:
            ver = None
    return pa, ver


def _print_encrypted_server_fields(resp: dict) -> None:
    """Best-effort dump of joinTicket.EncryptedServerFields (debug aid)."""
    jt = resp.get("joinTicket")
    obj = jt
    if isinstance(jt, str):
        try:
            obj = json.loads(jt)
        except ValueError:
            obj = None
    esf = (obj or {}).get("EncryptedServerFields") if isinstance(obj, dict) else None
    if not isinstance(esf, dict):
        return
    log("joinTicket.EncryptedServerFields (opaque, early-box path):")
    for key in ("ephPubKey", "nonce", "ciphertext", "tag"):
        val = esf.get(key)
        if isinstance(val, str):
            dec = b64d_loose(val)
            log("  %-12s %4d b64 chars -> %3d bytes %s"
                % (key, len(val), len(dec), hexs(dec, 16)))


def join_place(cookie: str, place_id: int, timeout: float) -> dict | None:
    """Run the real join flow; return a dict with everything the wire needs."""
    sess = HttpSession(cookie, timeout)
    ticket = fetch_auth_ticket(sess, place_id)
    referer = "https://www.roblox.com/games/%d/" % place_id
    base = {
        "placeId": place_id,
        "isTeleport": False,
        "isImmersiveAdsTeleport": False,
        "gameJoinAttemptId": str(__import__("uuid").uuid4()),
    }
    attempts = []
    with_ticket = dict(base)
    if ticket:
        with_ticket["authenticationTicket"] = ticket
    attempts.append(("/v1/join-game", with_ticket))
    if ticket:
        no_ticket = dict(base)
        no_ticket["authenticationTicket"] = None
        attempts.append(("/v1/join-game", no_ticket))
    attempts.append(("/v1/join-game-instance", with_ticket))

    resp = None
    for path, payload in attempts:
        url = GAMEJOIN_BASE + path
        status, _hdrs, body = sess.post(url, payload, referer=referer)
        text = body.decode("utf-8", "replace")
        log("join %s: HTTP %d" % (path, status))
        dbg("  body: %s" % text[:400])
        if not 200 <= status < 300:
            continue
        try:
            v = json.loads(text)
        except ValueError:
            log("  join response is not JSON")
            continue
        if not v.get("jobId"):
            log("  refused: %s" % _refusal(v))
            continue
        resp = v
        break
    if resp is None:
        return None

    js = _join_script_obj(resp)
    log("join: jobId=%s" % resp.get("jobId"))
    log("join script keys: %s" % ", ".join(sorted(js.keys())[:40]))

    rcc_ip, rcc_port, src = _join_endpoint(js)
    udmux = _udmux_endpoint(js)
    log("join: RCC endpoint %s:%d (from %s)" % (rcc_ip or "?", rcc_port, src or "?"))
    if udmux:
        log("join: udmux endpoint %s:%d" % udmux)
    if not rcc_ip:
        log("join: no server address in the response")
        return None

    dbg("join: top-level fields: %s" % ", ".join(sorted(resp.keys())))
    _print_encrypted_server_fields(resp)

    token = js.get("TokenValue") or js.get("UdmuxToken") or ""
    token16 = b64d_loose(token) if isinstance(token, str) else b""
    log("join: udmux token %s" % ("%d bytes" % len(token16) if token16 else "absent"))

    cpk_raw = js.get("ClientPublicKeyData")
    cpk_key, cpk_ver, cpk_versions = _parse_keyring_value(cpk_raw)
    if cpk_key:
        log("join: ClientPublicKeyData -> version %s, key %s (32 bytes)"
            % (cpk_ver, cpk_key.hex()))
    else:
        log("join: ClientPublicKeyData missing/unparsable")

    eph_raw = js.get("EphemeralEarlyPubKey") or ""
    eph_key = b64d_loose(urllib.parse.unquote(eph_raw)) if eph_raw else b""
    if len(eph_key) == 32:
        log("join: EphemeralEarlyPubKey -> %s (32 bytes)" % eph_key.hex())
    else:
        log("join: EphemeralEarlyPubKey absent or not 32 bytes")
        eph_key = b""

    pa_blob, auth_ver = _parse_client_ticket(js)
    if pa_blob:
        log("join: ClientTicket part2 -> %d-byte paBlob" % len(pa_blob))
    if auth_ver is not None:
        log("join: ClientTicket part4 -> authVer %d (0x%02x)" % (auth_ver, auth_ver))

    return {
        "job_id": resp.get("jobId"),
        "rcc_ip": rcc_ip,
        "rcc_port": rcc_port,
        "udmux": udmux,
        "token16": token16,
        "seed1_candidates": [k for k in (eph_key, cpk_key) if k],
        "seed1_names": [
            n for n, k in (("EphemeralEarlyPubKey", eph_key), ("ClientPublicKeyData", cpk_key)) if k
        ],
        "ephemeral_pub": eph_key,
        "pa_blob": pa_blob,
        "auth_ver": auth_ver if auth_ver is not None else 0x12,
    }


def _refusal(v: dict) -> str:
    parts = []
    if "status" in v:
        parts.append("status %s" % v.get("status"))
    err = v.get("joinError") or {}
    for key in ("joinStatusCode", "errorCode"):
        if err.get(key) is not None:
            parts.append("%s=%s" % (key, err.get(key)))
    desc = err.get("description") or err.get("message") or v.get("message") or ""
    if desc:
        parts.append('"%s"' % desc)
    return ", ".join(parts) or "no reason"


# ===========================================================================
# 4. wire (UDP)
# ===========================================================================


class RakNetProbe:
    """One UDP session against a Roblox RakNet-fork join endpoint."""

    def __init__(self, host: str, port: int, prefix: bytes, timeout: float):
        self.host = host
        self.port = port
        self.prefix = prefix
        self.timeout = timeout
        try:
            self.addr = (socket.gethostbyname(host), port)
        except socket.gaierror as exc:
            raise ValueError("cannot resolve %s: %s" % (host, exc)) from exc
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("0.0.0.0", 0))
        self.local_port = self.sock.getsockname()[1]

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass

    def send(self, data: bytes) -> None:
        dbg("send %d bytes -> %s:%d (local %d)" % (len(data), self.addr[0], self.addr[1], self.local_port))
        dump("  out", data, 32)
        self.sock.sendto(data, self.addr)

    def recv(self, deadline: float) -> bytes | None:
        while True:
            left = deadline - time.time()
            if left <= 0:
                return None
            self.sock.settimeout(left)
            try:
                data, peer = self.sock.recvfrom(4096)
            except socket.timeout:
                return None
            except OSError as exc:
                dbg("recv error: %s" % exc)
                return None
            if peer[0] != self.addr[0] and not peer[0].startswith("127."):
                dbg("ignoring datagram from unexpected peer %s:%d" % peer)
                continue
            dbg("recv %d bytes <- %s:%d" % (len(data), peer[0], peer[1]))
            dump("  in", data, 32)
            return data

    def exchange(self, packet: bytes, want: bytes, attempts: int,
                 retry: float = 0.5) -> bytes | None:
        """Send ``packet`` with retransmits until a reply containing ``want``."""
        deadline = time.time() + self.timeout
        for n in range(max(1, attempts)):
            self.send(packet)
            until = min(time.time() + retry, deadline)
            while True:
                data = self.recv(until)
                if data is None:
                    break
                if want in data:
                    return data
                dbg("  reply without expected magic %s (ignored)" % want.hex())
            if time.time() >= deadline:
                break
        return None


def run_wire(args, join: dict | None) -> int:
    """OCR1/ORP1 -> OCR2/ORP2 -> CREQ/replies."""
    if join is None and not args.server:
        log("wire: no join data -- nothing to probe")
        return 2

    if args.server:
        host, _, port_s = args.server.partition(":")
        try:
            port = int(port_s or 0)
        except ValueError:
            port = 0
        if not host or not (0 < port <= 65535):
            log("wire: --server must be host:port")
            return 2
        rcc_ip = args.rcc_ip or host
        rcc_port = port
        token16 = bytes.fromhex(args.token) if args.token else os.urandom(16)
        seed1_candidates = []
        if args.seed1:
            raw = bytes.fromhex(args.seed1) if _looks_hex(args.seed1) else b64d_loose(args.seed1)
            if len(raw) == 32:
                seed1_candidates = [raw]
        pa_blob = bytes.fromhex(args.pa_blob) if args.pa_blob else None
        auth_ver = args.auth_ver
        seed2_override = _maybe_key(args.seed2)
    else:
        host, port = (join["udmux"] if args.endpoint == "udmux" and join["udmux"] else (join["rcc_ip"], join["rcc_port"]))
        if args.endpoint == "auto" and join["udmux"] and _is_private(join["rcc_ip"]):
            log("wire: RCC address is private -- dialing the udmux proxy")
            host, port = join["udmux"]
        rcc_ip, rcc_port = join["rcc_ip"], join["rcc_port"]
        token16 = join["token16"] or os.urandom(16)
        seed1_candidates = join["seed1_candidates"]
        pa_blob = join["pa_blob"]
        auth_ver = join["auth_ver"]
        seed2_override = _maybe_key(args.seed2)

    if pa_blob is None:
        log("wire: WARNING: no ClientTicket paBlob -- using the captured 33-byte")
        log("      placeholder; the server will most likely reject OCR2.")
        pa_blob = _CAPTURE_PA_BLOB

    if not seed1_candidates:
        log("wire: WARNING: no seed1 (ClientPublicKeyData/EphemeralEarlyPubKey)")
        log("      -- OCR2 will use a throwaway key and the server will ignore it")
        seed1_candidates = [os.urandom(32)]
        if join is not None:
            join["seed1_names"] = ["random/absent"]

    if args.random_token:
        token16 = os.urandom(16)
        log("wire: --random-token: using 16 random RUPP bytes instead of the udmux token")

    log("wire: remote %s:%d, RUPP token %s" % (host, port, token16.hex()))
    prefix = build_prefix(token16, rcc_ip, rcc_port, args.token_type)

    # --- session key material (fresh per run) -----------------------------
    eph_priv = os.urandom(32)
    eph_pub = x25519_pub(eph_priv)
    session_id = os.urandom(8)
    log("wire: client eph_pub %s" % eph_pub.hex())
    log("wire: client eph_priv %s" % eph_priv.hex())
    log("wire: client session id %s" % session_id.hex())

    probe = None
    try:
        probe = RakNetProbe(host, port, prefix, args.timeout)
    except (ValueError, OSError) as exc:
        log("wire: cannot open the UDP probe: %s" % exc)
        return 2
    log("wire: local UDP port %d" % probe.local_port)
    try:
        # --- OCR1 / ORP1 --------------------------------------------------
        log("wire: OCR1 -> ORP1 (MTU probe)")
        orp1 = probe.exchange(build_ocr1(prefix), b"\x7e\x00\xff\xff\x00", args.attempts)
        if orp1 is None:
            log("wire: no ORP1 -- the endpoint or the RUPP prefix was rejected")
            log("wire: (try --token-type 2 or --server <udmux-ip:port>)")
            return 3
        mtu = parse_orp1(orp1) or 1200
        log("wire: ORP1 received (%d bytes), MTU %d" % (len(orp1), mtu))

        # --- OCR2 / ORP2 --------------------------------------------------
        # seed1 is one of two join-script keys; try both (the server answers
        # only for the one it owns).  A fresh eph pair per attempt is safest.
        orp2 = None
        seed1_used = None
        seed1_name = ""
        env_out1 = None
        for idx, seed1 in enumerate(seed1_candidates):
            name = join["seed1_names"][idx] if join and idx < len(join["seed1_names"]) else "seed1"
            eph_priv = os.urandom(32)
            eph_pub = x25519_pub(eph_priv)
            out1, out2 = sessder(eph_priv, eph_pub, seed1)
            header = build_ocr2_header(eph_pub, auth_ver, pa_blob)
            _pt93 = build_pt93(session_id, mtu, port)
            dbg("  pt93 %s" % _pt93.hex())
            ct, tag = chacha_seal(out2, ENVELOPE_NONCE, _pt93, header)
            packet = build_ocr2(prefix, header, ct, ENVELOPE_NONCE, tag)
            log("wire: OCR2 with seed1=%s (%s) -- %d bytes"
                % (seed1.hex()[:16] + "...", name, len(packet)))
            dbg("  envelope key (out2) %s  aad %s" % (out2.hex(), header.hex()))
            orp2 = probe.exchange(packet, b"\x7d\x00\xff\xff\x00", args.attempts)
            if orp2 is not None:
                seed1_used = seed1
                seed1_name = name
                env_out1 = out1
                break
            log("wire: no ORP2 with that seed1")
        if orp2 is None:
            log("wire: OCR2 was not answered -- the join key or paBlob is wrong")
            log("wire: (cross-check: ClientPublicKeyData vs EphemeralEarlyPubKey)")
            return 4

        log("wire: ORP2 received (%d bytes)" % len(orp2))
        log("wire: envelope out1 %s" % (env_out1.hex() if env_out1 else "?"))
        server_value = parse_orp2(orp2)
        if server_value is None:
            log("wire: ORP2 layout not recognised")
            dump("ORP2", orp2)
            return 4
        log("wire: ORP2 server value %s" % server_value.hex())
        log("wire: seed1 used = %s (%s)" % (seed1_used.hex(), seed1_name))
        log("wire: NOTE the ORP2 value is only accepted as the SESSDER#2 peer;")
        log("      a live experiment below also tries EphemeralEarlyPubKey.")

        # --- steady key candidates ---------------------------------------
        candidates: list[tuple[str, bytes]] = []
        if seed2_override:
            candidates.append(("--seed2", seed2_override))
        if env_out1 is not None:
            complete = orp2_seed2(orp2, env_out1)
            if complete is not None:
                log("wire: ORP2 box opened -- complete seed2 %s" % complete.hex())
                candidates.append(("ORP2 header+box tail", complete))
        candidates.append(("ORP2 value (23B only)", server_value))
        if join and join.get("ephemeral_pub"):
            if join["ephemeral_pub"] != server_value:
                candidates.append(("EphemeralEarlyPubKey", join["ephemeral_pub"]))
        seen = set()
        uniq = []
        for name, seed in candidates:
            if seed not in seen:
                seen.add(seed)
                uniq.append((name, seed))
        steady = []
        for name, seed in uniq:
            if len(seed) != 32:
                log("wire: skipping steady candidate %s (%d bytes, SESSDER needs 32)"
                    % (name, len(seed)))
                continue
            out1, out2 = sessder(eph_priv, eph_pub, seed)
            steady.append({"name": name, "seed": seed, "tx": out2, "rx": out1})
            log("wire: steady candidate %-22s seed %s" % (name, seed.hex()[:16] + "..."))
            log("      tx(out2)=%s" % out2.hex())
            log("      rx(out1)=%s" % out1.hex())

        # --- CREQ (steady box) --------------------------------------------
        creq_pt = build_connection_request(session_id)
        log("wire: CREQ plaintext (%d bytes) %s" % (len(creq_pt), creq_pt.hex()))
        if creq_pt[11:19] != session_id:
            log("wire: WARNING: the session id was not patched into the CREQ")

        counter = STEADY_BASE
        variants = [v.strip() for v in args.creq_variants.split(",") if v.strip()]
        cands = steady[:1] if args.creq_first_only else steady
        for cand in cands:
            for variant in variants:
                body = build_connection_request_box(variant, cand, counter, creq_pt, prefix)
                pkt = prefix + body
                log("wire: CREQ[%s/%s] %d bytes, counter hint %s"
                    % (cand["name"], variant, len(pkt), hexs(body[-18:-16])))
                probe.send(pkt)
                counter += 1
                opened = _collect_replies(probe, steady, args.wait)
                if opened:
                    log("wire: *** steady key CONFIRMED: seed2 source = %s ***"
                        % cand["name"])
                    return 0
                log("wire: no decryptable reply for CREQ[%s/%s]"
                    % (cand["name"], variant))
        log("wire: no reply opened with any candidate -- handshake not accepted")
        log("wire: (all received datagrams were logged above with --debug)")
        return 5
    finally:
        probe.close()


def _collect_replies(probe: RakNetProbe, steady: list[dict], wait: float) -> bool:
    """Read datagrams until the wait expires; try to open each as a steady box."""
    deadline = time.time() + wait
    any_opened = False
    while True:
        data = probe.recv(deadline)
        if data is None:
            return any_opened
        log("wire: server datagram %d bytes %s" % (len(data), hexs(data, 48)))
        opened = False
        for off in (23, 0, 31, 24, 4):
            body = data[off:]
            if len(body) < 18:
                continue
            for cand in steady:
                for suffix in (STEADY_SUFFIX, b"mber"):
                    for label, key in (("rx", cand["rx"]), ("tx", cand["tx"])):
                        res = steady_open(key, body, suffix=suffix)
                        if not res:
                            continue
                        pt, ctr = res
                        log("wire:   OPENED with %s/%s/%s offset %d counter 0x%x -> %d bytes"
                            % (cand["name"], label, suffix.decode(), off, ctr, len(pt)))
                        log("wire:   plaintext %s" % pt.hex())
                        _describe_steady_plaintext(pt)
                        opened = True
                        any_opened = True
                        break
                    if opened:
                        break
                if opened:
                    break
            if opened:
                break
        if not opened:
            log("wire:   (not a steady box we can open -- raw bytes above)")
    return any_opened


def _describe_steady_plaintext(pt: bytes) -> None:
    """Small human-readable hint about an opened steady payload."""
    if not pt:
        return
    ident = pt[0]
    names = {
        0x05: "CONNECTION_REQUEST",
        0x06: "CONNECTION_REQUEST_ACCEPTED",
        0x81: "raknet-frame/0x81",
        0x82: "raknet-frame/0x82",
        0x83: "raknet-frame/0x83",
        0xC0: "ACK",
        0xA0: "NAK",
        0xD0: "reliable/ACK-container",
    }
    log("wire:   id 0x%02x (%s)" % (ident, names.get(ident, "unknown/encrypted-frame")))


def build_connection_request(session_id: bytes) -> bytes:
    """The 34-byte steady plaintext sent as the client's first post-ORP2 box.

    Confirmed by decrypting pcap raknet27 rec 257 (and raknet29 rec 256):

        81 00 00 40 00 c0 00 00 00 00 09 | <session id 8> | 00*6 |
        <2 unknown bytes: fd fc in the capture> | 00 | 37 4f 5e 11 6c 45

    Only the 8-byte session id (the same one announced in pt93) is ours.

    TODO(CREQ): the two bytes after the six zero bytes are per-session and
    their derivation is still unknown (observed ffa9 / ff4d / ff2e / fdfc).
    The template value is sent verbatim; `--creq-marker` overrides it and the
    `raw*` variants exist as fallbacks.  The vanilla 24-byte layout is
    provided by `build_vanilla_connection_request` for experiments.
    """
    if len(session_id) != 8:
        raise ValueError("session id must be 8 bytes")
    return CREQ_PT_TEMPLATE.replace(CREQ_TEMPLATE_ID, session_id)


def build_vanilla_connection_request(guid8: bytes, ts_ms: int) -> bytes:
    """RakNet CONNECTION_REQUEST (24 B): id, guid, time, useSecurity, password."""
    return b"\x05" + guid8 + struct.pack(">Q", ts_ms & 0xFFFFFFFFFFFFFFFF) + b"\x00" + RBX_PASSWORD


def build_connection_request_box(variant: str, cand: dict, counter: int,
                                 pt34: bytes, prefix: bytes = b"") -> bytes:
    """Seal a CREQ variant into the steady container (ct || hint || tag).

    Variants exist because the steady container was the last unresolved piece
    of the handshake; the confirmed layout is ``steady34``.  The others stay
    available for live experiments / older servers:

      steady34      captured 34-byte pt, tx key (SESSDER#2 out2), empty AAD
      steady34-o1   same pt but the *other* SESSDER key (out1)
      steady34-aad  same pt/key, the RUPP prefix as AAD
      steady24      vanilla 24-byte RakNet CONNECTION_REQUEST, tx key
      raw34/raw24   unsealed fallbacks (plaintext on the wire)
    """
    if variant in ("steady34", "steady34-o1", "steady34-aad", "steady24"):
        key = cand["rx"] if variant == "steady34-o1" else cand["tx"]
        aad = prefix if variant == "steady34-aad" else b""
        if variant == "steady24":
            guid = pt34[11:19]
            pt = build_vanilla_connection_request(guid, int(time.time() * 1000))
        else:
            pt = pt34
        return steady_seal(key, counter, pt, aad=aad)
    if variant == "raw34":
        return pt34
    if variant == "raw24":
        guid = pt34[11:19]
        return build_vanilla_connection_request(guid, int(time.time() * 1000))
    raise ValueError("unknown CREQ variant %r" % variant)


def _is_private(ip: str) -> bool:
    try:
        return ipaddress.ip_address(ip).is_private
    except ValueError:
        return False


def _looks_hex(s: str) -> bool:
    s = s.strip()
    return len(s) >= 2 and len(s) % 2 == 0 and all(c in "0123456789abcdefABCDEF" for c in s)


def _maybe_key(s: str | None) -> bytes | None:
    if not s:
        return None
    raw = bytes.fromhex(s) if _looks_hex(s) else b64d_loose(s)
    return raw if len(raw) == 32 else None


# ===========================================================================
# 5. selftest vectors (published captures -- no live credentials)
# ===========================================================================

#: pcap raknet27, session 16:22.  Client eph pair + both SESSDER seeds.
_V27_EPH_PRIV = bytes.fromhex("477ece574ba776b5ea29fde7a520b6c02e6b679316d6a98373668e6a3997bc93")
_V27_EPH_PUB = bytes.fromhex("c840edc139a090d05992e6307bb2af61fa2d9c082d17b84f2c7a484491366f55")
_V27_SEED1 = bytes.fromhex("b6d5994bec64e2934a2d2c13724d68ef5b383a0f4c3fcdd9b6d2ff1ae342401b")
_V27_SEED2 = bytes.fromhex("0259d3d55362bf48abfb3895f3b46431418408bd38cfb7f06e5def6a7009e146")

#: pcap raknet27 rec 253 (OCR2) -- decrypted in the captures.
_V27_OCR2_HEADER = bytes.fromhex(
    "7800ffff00fefefefefdfdfdfd12345678035a005d0005"
    "c840edc139a090d05992e6307bb2af61fa2d9c082d17b84f2c7a484491366f55"
    "1221c7dfce1fa8168af618b5d9c72e1bf3f506accc3c7a17a8037edfedcf4193e7e544"
)
_V27_PT93 = bytes.fromhex(
    "100ddbc750b30896bc1a859cb9a65a7604b002047f8be0deffc9420601c28cb7"
    "72918acc466844da66e88116b16fae63e3ff7012c92e8058625e83de404c2ce22"
    "b2a66c7fc5df062e0422bb354696e6e6150646d7c08c5784a9f2e3607"
)
_V27_OCR2_CT = bytes.fromhex(
    "f495e0f0426b4740bfcdb39edcd9faaf4d4fe2d48867ec4dcd527132b0f34645"
    "9fa881c3ad631b878c11a75c378b22580e3118a81e5b674eb4af5553ec0e5adc"
    "8f8afea661390c61276fbb82f6705b73024ba2c80a9c5fb745e5dd6f79"
)
_V27_OCR2_TAG = bytes.fromhex("9c510aaf82512676f55980dae780125a")
_V27_PA_BLOB = _V27_OCR2_HEADER[57:90]

#: pcap raknet27 rec 251 (OCR1, 1160 B).  Prefix token/ip/port:
_V27_OCR1 = bytes.fromhex(
    # 31-byte RUPP prefix + 7b 00 ff ff 00 + fe*4 + fd*4 + marker + 05 +
    # protocol(+epoch) -- the remaining 1107 bytes are zero padding.
    "0100001f011101996211444c14d9ffa5ff2544ccaff8dd02060a220811ffc9"
    "7b00ffff00fefefefefdfdfdfd123456780501000000"
) + b"\x00" * 1107
_V27_OCR1_TOKEN = bytes.fromhex("996211444c14d9ffa5ff2544ccaff8dd")
_V27_RCC_IP = "10.34.8.17"
_V27_RCC_PORT = 65481

#: pcap raknet27 rec 252 / 256.
_V27_ORP1 = bytes.fromhex("010000047e00ffff00fefefefefdfdfdfd12345678235a77a8065bd4da0004b0")
_V27_ORP2 = bytes.fromhex(
    "01000017011102232523af47043a916cd2ed1139c4ec897d00ffff00fefefefefdfdfdfd"
    "12345678012c003a0259d3d55362bf48abfb3895f3b46431418408bd38cfb7e8c6d283"
    "0f197daf63d03502bb5bb642c0270e2b2cececce5d182260e8f29ac95793144c81ce32"
    "6e41d187def5d3d1d601df63aa87d406d3edb4731e3fe2d1556e697175654e756d6265"
    "729f3052bd3cd972bb17854a3098f0c7da"
)

#: pcap raknet27 rec 257 (CREQ) and rec 258/259 (server steady replies).
_V27_CREQ_BODY = bytes.fromhex(
    "192e67a32717aa711ca14a87a69995c72f34a709ec879abc1fac04bfc1783b7ac2455"
    "56ead72164b54216daea8b42562cea7cd47"
)
_V27_CREQ_PT = bytes.fromhex("810000004000c000000009bc1a859cb9a65a76000000000000fdfc00374f5e116c45")
_V27_REP1_BODY = bytes.fromhex("59e040c2f891a1e8a54535e18c556ee1e3850f9b0bab932b8207c65f8b4545")
_V27_REP1_PT = bytes.fromhex("d0000101000000060000060000")
_V27_REP2_BODY = bytes.fromhex(
    "b24cc9216d031e6c2e6cbd28beef186ba36a6df8d583df45b00f31bf1a80c53174fb3c"
    "bd14597d8c3b4107e84cfa5d610a68f1923aab2a2ef81f6af1d796a70f43005466e659"
    "f66c87543df34c690342394c892521de0353533023c26c562336e3e75d2350ed4045cb"
    "63ad69313b4bf8e4f4e6bc1017566ee571bd3070572823d589a3fc332f3d52"
)
_V27_REP2_PT = bytes.fromhex(
    "81000000600340000000000000001004ffffffff0afd00020480fffefe000004ffffff"
    "ff000004ffffffff000004ffffffff000004ffffffff000004ffffffff000004ffffff"
    "ff000004ffffffff000004ffffffff000004ffffffff0000000000000000fdfc000000"
    "000256b63b000660f327668bd3"
)

#: The captured OCR2 paBlob is only a last-resort placeholder for --server mode.
_CAPTURE_PA_BLOB = _V27_PA_BLOB


class _T:
    def __init__(self) -> None:
        self.fail = 0
        self.total = 0

    def check(self, name: str, cond: bool, detail: str = "") -> None:
        self.total += 1
        if cond:
            log("  [PASS] %s" % name)
        else:
            self.fail += 1
            log("  [FAIL] %s %s" % (name, detail))


def selftest() -> int:
    t = _T()
    log("selftest: crypto primitives against published capture vectors")

    # --- 1. SESSDER -------------------------------------------------------
    secret = bytes.fromhex("2600cb0b8c531e33b7d1f521f008967ee4975c8a2b8ac8ae0220417297d9c634")
    priv = bytes.fromhex("576bc63fcad8efba701998ebbbae8206ea7924a503c0d5cb83ba04c6dd576cca")
    seed = bytes.fromhex("8d73699192944e540a960e37deeb965a2d84e64020e35c98c5b46210b84fd141")
    want1 = "e31804db6aa541f0ef1b26019862f0d979a2dd321be08942c398f05adcaf5c35"
    want2 = "fb99f2fc5adf47a9a20675917fb8c1ffb8d437954204e63016b7c72d0956629e"
    t.check("SESSDER pub(priv) == the 'secret' slot", x25519_pub(priv) == secret)
    o1, o2 = sessder(priv, x25519_pub(priv), seed)
    t.check("SESSDER out1", o1.hex() == want1, o1.hex())
    t.check("SESSDER out2", o2.hex() == want2, o2.hex())

    # --- 2. envelope ChaCha (pcap raknet27 rec 253, fully reproducible) ----
    env1, env2 = sessder(_V27_EPH_PRIV, _V27_EPH_PUB, _V27_SEED1)
    t.check("SESSDER#1 out2 == OCR2 seal key (a96e74b9...)",
            env2.hex() == "a96e74b96f2498b3536a542ebef6325b30a60a1956458af7e9b473ba8bf0e60e",
            env2.hex())
    ct, tag = chacha_seal(env2, ENVELOPE_NONCE, _V27_PT93, _V27_OCR2_HEADER)
    t.check("ChaCha seal ct == capture", ct == _V27_OCR2_CT)
    t.check("ChaCha seal tag == capture", tag == _V27_OCR2_TAG, tag.hex())
    pt = chacha_open(env2, ENVELOPE_NONCE, _V27_OCR2_CT, _V27_OCR2_TAG, _V27_OCR2_HEADER)
    t.check("ChaCha open roundtrip", pt == _V27_PT93)
    hdr = build_ocr2_header(_V27_EPH_PUB, 0x12, _V27_PA_BLOB)
    t.check("90-byte OCR2 header rebuilt byte-exact", hdr == _V27_OCR2_HEADER)

    # --- 3. steady AES-256-GCM -------------------------------------------
    st1, st2 = sessder(_V27_EPH_PRIV, _V27_EPH_PUB, _V27_SEED2)
    t.check("SESSDER#2 out2 == steady seal key (7f995325...)",
            st2.hex() == "7f995325cb65dfbd1ab3424e98a3bd7ecea81b15b53a1702edaf958cdf17f0e1",
            st2.hex())
    t.check("SESSDER#2 out1 == steady open key (09b81d09...)",
            st1.hex() == "09b81d09de6f8c5bb491bc66bf193eae8960a2fc404f64d0b90aefaade3dcbdc",
            st1.hex())

    body = steady_seal(st2, STEADY_BASE, _V27_CREQ_PT)
    t.check("CREQ box reseal == capture (ct||hint||tag)", body == _V27_CREQ_BODY, body.hex())
    res = steady_open(st2, _V27_CREQ_BODY)
    t.check("CREQ box open", res is not None and res[0] == _V27_CREQ_PT)

    for name, raw, want in (
        ("server reply #1", _V27_REP1_BODY, _V27_REP1_PT),
        ("server reply #2", _V27_REP2_BODY, _V27_REP2_PT),
    ):
        res = steady_open(st1, raw)
        t.check("%s opens with the rx key" % name, res is not None and res[0] == want)

    # --- 4. brief's AES-variant vector (library) --------------------------
    # The brief prints ct=8100..., pt=0ab9c124..., tag=c7f6....  Live captures
    # show 8100... is the *plaintext* and 0ab9c124... the ciphertext, i.e. a
    # standard GCM tag over the ciphertext.  Both hex strings are checked.
    brief_key = bytes.fromhex("548ae60749bc987524a72b3aa1a831b9e756b014da605f91f0cb79830e98fd03")
    brief_pt = bytes.fromhex("810000004000c0000000091c336b4aee8b62d8000000000000ff4d00374f5e116c45")
    brief_ct = bytes.fromhex("0ab9c1244265317a9617f40b385c1c619ca28785229a5a0f56b62860aa3d1cc2216f")
    brief_tag = bytes.fromhex("c7f6eca259a1e239d91304ca49d93352")
    nonce0 = steady_nonce(STEADY_BASE, b"mbeR")
    t.check("brief vector: AEAD-AES #2 is a *seal* of 8100...",
            AESGCM(brief_key).encrypt(nonce0, brief_pt, b"") == brief_ct + brief_tag)
    t.check("brief vector: standard-GCM open of 0ab9c124... == 8100...",
            AESGCM(brief_key).decrypt(nonce0, brief_ct + brief_tag, b"") == brief_pt)

    # --- 5. manual GCM agrees with the library ---------------------------
    t.check("manual GCM seal == AESGCM",
            aes_gcm_seal_manual(brief_key, nonce0, brief_pt) == brief_ct + brief_tag)
    t.check("manual GCM open",
            aes_gcm_open_manual(brief_key, nonce0, brief_ct, brief_tag) == brief_pt)

    # --- 6. wire parsing / construction ---------------------------------
    t.check("ORP1 MTU parsed", parse_orp1(_V27_ORP1) == 1200)
    t.check("OCR1 rebuilt byte-exact",
            build_ocr1(build_prefix(_V27_OCR1_TOKEN, _V27_RCC_IP, _V27_RCC_PORT)) == _V27_OCR1)
    seed2 = parse_orp2(_V27_ORP2)
    t.check("ORP2 server value prefix (23B) located", seed2 == _V27_ORP2[44:67], hexs(seed2 or b""))
    # the 9-byte tail lives in the ORP2 box plaintext (key = envelope SESSDER#1 out1)
    _v27_env_out1 = bytes.fromhex(
        "8795dcd2d44a95a8693cff9ac09f26cd8e2dbcde687640e38c01d521bc8c949d")
    complete2 = orp2_seed2(_V27_ORP2, _v27_env_out1)
    t.check("ORP2 box opens -> complete seed2",
            complete2 is not None
            and complete2.hex() == "0259d3d55362bf48abfb3895f3b46431418408bd38cfb7"
                                        "f06e5def6a7009e146",
            hexs(complete2 or b""))
    # Documented, important: the hook's SESSDER#2 seed shares only the first
    # 23 bytes with the raw ORP2 field.  The live probe therefore tries both
    # that field and EphemeralEarlyPubKey as the steady peer.
    t.check("ORP2 value shares the hook seed's first 23 bytes",
            seed2 is not None and seed2[:23] == _V27_SEED2[:23])
    log("  [note] ORP2 raw tail differs from the SESSDER#2 seed tail:")
    log("         wire %s" % (seed2[23:].hex() if seed2 else "?"))
    log("         hook %s" % _V27_SEED2[23:].hex())

    # --- 7. brief's ChaCha pair (documented as unreproducible) ----------
    brief_ch_key = bytes.fromhex("ffca3d9c0a5a848c350e7928d7879358db06b55a358141b9b63b89f70123fa08")
    _, brief_tag_actual = chacha_seal(brief_ch_key, ENVELOPE_NONCE, _V27_PT93, _V27_OCR2_HEADER)
    log("  [note] the brief's ChaCha key ffca3d9c + tag d6d27ecf come from")
    log("         hook_v16b, whose 90-byte AAD was never logged; with the")
    log("         recorded rec-253 AAD the same key yields %s" % brief_tag_actual.hex())
    log("         The reproducible rec-253 vector above is the hard check.")

    log("selftest: %d/%d checks passed" % (t.total - t.fail, t.total))
    return 0 if t.fail == 0 else 1


# ===========================================================================
# 6. main
# ===========================================================================


def _resolve_cookie(args) -> str:
    cookie = args.cookie or os.environ.get("ROBLOSECURITY") or ""
    if not cookie and args.cookie_file:
        try:
            with open(args.cookie_file, "r", encoding="utf-8", errors="replace") as fh:
                for line in fh:
                    if ".ROBLOSECURITY" in line:
                        cookie = line.strip().split("\t")[-1]
                        break
        except OSError as exc:
            log("cannot read --cookie-file: %s" % exc)
    cookie = cookie.strip().strip('"')
    if cookie.startswith(".ROBLOSECURITY="):
        cookie = cookie.split("=", 1)[1]
    return cookie


def main(argv: list[str] | None = None) -> int:
    global DEBUG, CAPS_OVERRIDE
    ap = argparse.ArgumentParser(
        description="Real Roblox join + RakNet crypto handshake check.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "examples:\n"
            "  %(prog)s --selftest\n"
            "  ROBLOSECURITY=... %(prog)s --place-id 606849621\n"
            "  %(prog)s --server 128.116.44.33:49861 --seed1 <hex> \\\n"
            "           --seed2 <hex> --token <32 hex> --debug\n"
        ),
    )
    ap.add_argument("--selftest", action="store_true", help="verify crypto against capture vectors")
    ap.add_argument("--cookie", help=".ROBLOSECURITY value (default: $ROBLOSECURITY)")
    ap.add_argument("--cookie-file", help="Netscape cookies.txt containing .ROBLOSECURITY")
    ap.add_argument("--place-id", type=int, default=606849621)
    ap.add_argument("--server", help="skip the web join: host:port of the game server")
    ap.add_argument("--endpoint", choices=("auto", "rcc", "udmux"), default="auto",
                    help="which join endpoint to dial (default auto: udmux for private RCCs)")
    ap.add_argument("--rcc-ip", help="RCC private IPv4 used in the RUPP prefix (--server mode)")
    ap.add_argument("--token", help="16-byte RUPP routing token as hex (--server mode)")
    ap.add_argument("--token-type", type=int, default=RUPP_TOKEN_TYPE,
                    help="RUPP prefix byte 6 (default 1; 2 is the other live variant)")
    ap.add_argument("--random-token", action="store_true",
                    help="ignore the join's udmux TokenValue and use 16 random RUPP bytes")
    ap.add_argument("--seed1", help="server early pub (hex or b64) when using --server")
    ap.add_argument("--seed2", help="override the steady-layer server value")
    ap.add_argument("--pa-blob", help="33-byte ClientTicket part2 as hex (--server mode)")
    ap.add_argument("--auth-ver", type=lambda s: int(s, 0), default=0x12)
    ap.add_argument("--caps", help="67-byte pt93 caps payload hex (the app's own)")
    ap.add_argument("--creq-variants", default="steady34",
                    help="comma list: steady34,steady34-o1,steady34-aad,steady24,raw34,raw24")
    ap.add_argument("--creq-first-only", action="store_true",
                    help="only try the first steady-key candidate")
    ap.add_argument("--creq-marker", help="override the unknown 2 bytes in the CREQ template (hex)")
    ap.add_argument("--timeout", type=float, default=8.0, help="per-phase receive timeout (s)")
    ap.add_argument("--attempts", type=int, default=6, help="UDP retransmits per phase")
    ap.add_argument("--wait", type=float, default=8.0, help="seconds to listen after CREQ")
    ap.add_argument("--debug", action="store_true", help="hex-dump every datagram")
    args = ap.parse_args(argv)

    DEBUG = args.debug
    if getattr(args, "caps", None):
        CAPS_OVERRIDE = bytes.fromhex(args.caps)
    if args.creq_marker:
        global CREQ_PT_TEMPLATE
        marker = bytes.fromhex(args.creq_marker)
        if len(marker) != 2:
            log("--creq-marker must be 2 bytes of hex")
            return 2
        CREQ_PT_TEMPLATE = CREQ_PT_TEMPLATE[:24] + marker + CREQ_PT_TEMPLATE[26:]

    allowed = {"steady34", "steady34-o1", "steady34-aad", "steady24", "raw34", "raw24"}
    bad = [v for v in (x.strip() for x in args.creq_variants.split(",")) if v and v not in allowed]
    if bad:
        log("unknown --creq-variants value(s): %s" % ", ".join(bad))
        log("allowed: %s" % ", ".join(sorted(allowed)))
        return 2

    if args.selftest:
        return selftest()

    cookie = _resolve_cookie(args)
    if not cookie and not args.server:
        log("no session cookie: pass --cookie, --cookie-file or set ROBLOSECURITY")
        return 2

    join = None
    if args.server:
        log("step 1/3: skipping the web join (--server given)")
    else:
        log("step 1/3: web join for place %d" % args.place_id)
        join = join_place(cookie, args.place_id, args.timeout)
        if join is None:
            log("join failed -- cannot continue (use --server + --seed1 to test the wire only)")
            return 2
        log("step 2/3: join data collected")

    log("step 3/3: RakNet crypto handshake")
    return run_wire(args, join)


if __name__ == "__main__":
    sys.exit(main())
