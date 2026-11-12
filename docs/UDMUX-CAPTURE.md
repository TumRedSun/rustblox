# UDMUX/RUPP wire capture — Nov 2026

Source: `sober_ro.pcap` (32.4 MB, 40 478 UDP datagrams, session 11:04:19–11:07:08 UTC,
flow `192.168.1.117:40002 <-> 128.116.21.33:61446`, 33 783 datagrams) plus an
8-datagram handshake dump of a second session (11:40 UTC, same place 537413528,
RCC `10.206.18.67:49251`, udmux advertised `128.116.5.33:49251`) and its
`join_full.json`. Captured from a working modern client (Sober).

## The client RUPP envelope (byte-exact)

```
byte[0]   01                RUPP flag (not a RakNet message id)
byte[1:3] 00 00             header length, high bytes
byte[3]   04 | 17 | 1f      TOTAL header length (4 = bare prefix, no fields)
byte[4:7] 01 11 <cls>       cls: 0x01 = client datagrams while the offline
                            handshake (magic) is on the wire; 0x02 = everything
                            after, and every server datagram
byte[7:23] tag              16-byte session tag
byte[23:31] mux             only when byte[3] = 1f:
                            02 06 <rcc ip4> <rcc port BE16>
body                        RakNet datagram
```

Measured header bytes, handshake dump pkt 0 (C->S OCR1, rcc `10.206.18.67:49251`):

```
01 00 00 1f 01 11 01 84 03 fd 4a ec 75 0b ce 1c 44 12 c3 e6 f4 cc 55
02 06 0a ce 12 43 c0 63 | 7b | 00 ff ff 00 fe fe fe fe fd fd fd fd 12 34 56 78 | 05 | zero pad
```

* `02 06 0a ce 12 43 c0 63` = `02 06` + `10.206.18.67` + `49251` — the private RCC
  address from `joinScript.ServerConnections`, verbatim. The proxy routes on this,
  not on the tag. Second session confirms it (`02 06 0a 11 06 a5 f0 06` =
  `10.17.6.165:42480`).
* The pre-connect tag (`84 03 fd 4a ec 75 0b ce 1c 44 12 c3 e6 f4 cc 55`) matches
  no join-script field: not `TokenValue` (`5456f19d984e04384d64464e9e5805ec`),
  not `NetStackTokenValue` (`6944354f510627468fedad17b0d3204c`), not a hash/md5
  of either, not a slice of `RandomSeed1/2`. It is client-chosen.
* The connected-phase capture shows BOTH sides sending the SAME tag
  (`4a69c22758f1bdadbc29c3a3a17470b4`) — the client adopts the server's tag from
  the reply headers.
* Server REPLY_1 arrives with a bare `01 00 00 04` prefix; REPLY_2 and later
  server datagrams use `01 00 00 17 01 11 02 <tag>` (no mux field).

## Handshake sequence observed (times relative to first datagram)

| t (ms) | dir | hlen | body id | notes |
|---|---|---|---|---|
| 0 | C->S | 1f | `7b` | OCR1: magic + proto `05` + zero pad to 1160 |
| 65.0 | S->C | 04 | `7e` | REPLY_1: magic + guid(8) + `00` + MTU `04 b0` (1200) |
| 65.2 | C->S | 1f | `78` | OCR2: magic + `03 5a 00 5d 00 05` + opaque blob + `"UniqueNumber"` + 16 B |
| 130.7 | S->C | 17 | `7d` | REPLY_2: magic + `01 2c 00 3a 03` + blob + `"UniqueNumber"` + 16 B |
| 130.9 | C->S | 1f | — | encrypted (early key), cls still 01, client tag |
| 195.6 | S->C | 17 | — | encrypted pair (`3f…`, 31 B) |
| 195.7 | S->C | 17 | — | encrypted (`05…`, 159 B) |
| 196.0 | C->S | 1f | — | encrypted, cls 02, SERVER tag adopted |

## What this changed in the client

* `udmux::client_envelope` — the measured shape; pinned without probing when the
  join script carries both the token and the private RCC address.
* `rupp_parse` accepts header length `0x04` (bare REPLY_1 prefix).
* `Envelope.cls` field; class byte 0x01 pre-connect, 0x02 from Connected on.
* `RakClient` learns the server tag from every framed reply and adopts it at
  Connected (matching the capture's single-tag connected phase).
* The legacy token-derived probe matrix stays as fallback for direct dials.

## Still unknown (needs the next capture or a live run)

* The encrypted post-REPLY_2 bodies (`91…/3f…/05…/2b…`) — which layer they
  belong to (early-pubkey auth vs session AEAD) and their plaintext framing.
* Whether the advertised `UdmuxEndpoints[0]` port is really the dial target
  (the 11:04 session used proxy `128.116.21.33:61446`, not a `:49251` endpoint).

## Nov 12 follow-up — handshake_full.txt + join_full.json (same session)

The user uploaded the full 8-datagram handshake dump and the full join script
of the SAME session (`jobId 96a696fd…`, place 537413528, RCC
`10.206.18.67:49251`, proxy `128.116.5.33:49251`). Re-measured byte-exact:

* **Our OCR1 was rejected for two provable reasons, not the tag.**
  1. Size: the real client probes OCR1 at **1160 bytes on the wire**
     (MTU 1200 minus a 40-byte allowance); REPLY_1 answers `mtu = 04 b0`
     (1200). We sent 1400 (`mtu: 1400` − 31 header) — above the proxy's
     1200 path MTU, silently dropped in transit.
  2. Tail: OCR1 is `7b | magic | 05 | 01 | zero pad` — the byte after the
     protocol version is `01`, not pad. Meaning unknown (epoch/auth flag);
     our `05 00` never drew a reply. Fixed as `RBX_OCR1_EPOCH`.
  The pre-connect tag stays client-chosen (capture tag `84 03 fd 4a…`
  matches no join-script field, again) — the pinned envelope was never the
  blocker. OCR2 is zero-padded to the same 1160-byte datagram
  (content ends at byte 243).
* **Post-REPLY_2 framing identified** (matches `ref/raknet-decryption`):
  body = `ciphertext || counter_hint(u16 LE) || tag(16)`, ChaCha20-Poly1305
  or AES-256-GCM, key 32 B, nonce = `u64 LE counter || "mbeR"`. Hints in the
  dump are consecutive per direction (`…6e55`, `…6e56`).
* **OCR2/OR2 tail**: both end with the ASCII string `UniqueNumber` (12 B)
  followed by exactly 16 B (in OR2 it sits at `n-28`, i.e. the last 16 B is
  plausibly the AEAD tag; in OCR2 one more byte precedes it at content end).
  The opaque middle is 189 B (OCR2) / 113 B (OR2) — the early-encryption
  exchange. Undecoded without the client binary.
* **Join-script crypto material, decoded** (URL-encoding gotcha:
  `EphemeralEarlyPubKey` rides percent-encoded — `%2B/%2F/%3D` — which made
  our b64 reader bail with 0 bytes; now decoded before base64):
  - `EphemeralEarlyPubKey` = **32 B X25519 public key** (per-session).
  - `ClientPublicKeyData` = JSON, not binary: a persistent client key
    `RakNetEarlyPublicKey`, version id 5, 32 B, `"send":5,"revert":5` —
    created 2022, rotated by version.
  - `NetStackConfig` = percent-encoded JSON: `pubKey` (32 B), `port`,
    `caps "120ddbe750bb1896"`, `certHashes` (SHA-256-looking, 32 B).
  - `joinTicket.EncryptedServerFields` = AES-GCM envelope for the client:
    `ephPubKey` 64 B + `nonce` 12 B + `ciphertext` 2607 B + `tag` 16 B —
    the server→client secret drop encrypted under the client's early key.
  - `TokenValue` / `NetStackTokenValue` = 16 B each; neither appears in the
    captured datagrams.
* Fixed in the client: OCR1 probe size 1160 + `05 01` tail, OCR2 padded to
  1160, percent-decode of `EphemeralEarlyPubKey`/`NetStackConfig`,
  `TokenGenAlgorithm` comment unlinked from the (now settled) tag question.
* Next wall: the OCR2 blob — needs the client binary (libroblox) around the
  strings `UniqueNumber`, `RakNetEarlyPublicKey`, `EphemeralEarlyPubKey`.

## Nov 12 (evening) — the crypto handshake decoded to its framing

New material: the full RE corpus from the working session (`asdasd.zip`):
`libroblox.so` disassembly fragments (`verify_func`, `material40ca584`,
`preauth_window`, `keyring_parse`, `keyringclient`, `derive40c9d5a`,
`mac68b20c0`), `str_addrs.json` (string→address map), probe scripts
(`probe.py`..`probe4.py`), `good_tag.bin`, `reply1.bin`, `rustblox.dmp`, and
`ref/raknet-decryption` (a public PoC that decrypts the Roblox RakNet stream
when given session keys). Everything below is either byte-measured or
cross-checked between the dump, the pcap and that reference.

### Framing: two distinct crypto phases

1. **Offline crypto boxes** (the OCR2 request box and the OpenReply2 reply
   box): `nonce(12, random, on the wire) || ct || tag(16)`.
   * OCR2: AAD = the 90-byte RakNet prefix (`78 | magic | 03 5a 00 5d 00 05 |
     pub32 | 11 | pa_len | pa33`); box = 93 = 12 + 65 + 16.
   * OpenReply2: box = 53 on the wire (its own header declares `01 2c 00 3a
     03` = ver 1, aad 44, box 58 — the declared 58 does not match the 53
     measured; parsed tolerantly).
2. **Everything after OpenReply2** (boxed CONNECTION_REQUEST, ACCEPTED,
   NEW_INCOMING, the whole connected session — one scheme):
   `wire = ct || counter_hint(u16 LE) || tag(16)`, nonce =
   `u64 LE counter || "mbeR"`, AAD EMPTY. Proven: the pcap's 56/59-byte
   keepalives carry strictly sequential hints (39970, 39971, 39972, …
   shared across sizes), and the dump's pkt4..7 hints are `6e55, 6e55,
   6e56, 6e56` — both directions count from the SAME negotiated base.
   This is exactly `ref/raknet-decryption`'s scheme (`get_nonce_bytes`).
   The `"mbeR"` tail comes from the literal `UniqueNumbeR` string.

### The counter base is NOT zero

The dump's first post-OR2 counter is 0x6e55 on both sides — the base rides
inside session material we cannot decrypt offline (probably the OpenReply2
box or `ID_SET_GLOBALS`). The client takes `RUSTBLOX_NONCE_BASE` (default 0)
and the opener scans a window of high bits around the base.

### Key schedule (probe4.py + ref, not yet server-validated)

`H = BLAKE2b-512(x25519(client_sk, server_pk) || client_pk || server_pk)`;
client sends with `H[32:]`, receives with `H[:32]` (libsodium crypto_kx).
Two agreements: the join script's `EphemeralEarlyPubKey` (OCR2 box) and the
OpenReply2's server key (everything after). ChaCha20-Poly1305
(ref also tries AES-256-GCM).

### What is implemented now (src/roblox/early.rs)

* `from_join` — percent-decode + b64 the join key (the "0 bytes" bug).
* `crypto_kx` — the schedule above (RFC 7748 + BLAKE2b vector tested).
* `build_ocr2` — the byte-exact crypto OCR2 (AAD 90, box 93,
  `UniqueNumber` + 16).
* `parse_reply2` — the crypto OpenReply2 (server key, box, UniqueNumber).
* `seal_counter`/`open_counter` — the post-OR2 counter framing.
* `parse_keyring` — `ClientPublicKeyData` JSON (versions + send id).
* RakClient: crypto OCR2 first, sala shapes as fallback; boxed CR; counter
  open on every recv path; tag adoption + cls byte switch unchanged.

### Open items (needs a live probe run — tools/udmux_probe.py)

* The 65-byte OCR2 box plaintext (default zeros, `RUSTBLOX_OCR2_PAYLOAD`).
* The 33-byte preauth blob: 240 HMAC/BLAKE2/keying combinations over the
  public join fields miss the captured sample; carried verbatim
  (`RUSTBLOX_PREAUTH`) until the libroblox derivation is recovered.
  The libroblox preauth-window code accepts exactly 32, 50 or 33 bytes.
* The fork's CONNECTION_REQUEST plaintext: the dump's box holds **34**
  bytes; the vanilla CR is 24 (`ID+guid+time+useSec+password`). The
  extra 10 bytes are unresolved — the probe tries both sizes.
* The counter base (see above) and the OpenReply2 box content.
* The 16 bytes after `UniqueNumber` in both directions (client-chosen?
  server-chosen? a MAC over the clear section?).
