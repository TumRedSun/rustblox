# Game join / application layer — live RE (24 Nov 2026)

Scope: what the real Android client (2.748.1527) actually sends between the
RakNet connected handshake and the first world replication, plus the `0x9b`
challenge/reply. Everything marked **CONFIRMED** below was reproduced in this
session by reading raw bytes, live process memory/hook output, or instructions
in `/storage/rblx_debug/apk_new/libroblox.so` (sha256
`ca6822c705888047e066bb9cfbb883cf316cc75a324680aff0cbd2b554771ab4`,
addresses == file offsets in the first LOAD).

Artifacts used (outside the repo):

```
/storage/rblx_debug/session-crypto/logs/hook_open2.log   live app session, Frida v2 hook
/storage/rblx_debug/session-crypto/logs/hook_open3.log   second live app session, v3 hook
/tmp/opencode/relre/hook_open2.js / hook_open3.js        the hooks that produced them
/tmp/opencode/relre/parse_log.py                         reassembles both directions
/storage/rblx_debug/session-crypto/pcap/raknet27.pcap    v27 session (wire)
/storage/rblx_debug/session-crypto/pcap/raknet29.pcap    v29 session (wire)
```

## 1. CONFIRMED — the client's join upload sequence

One real session (`hook_open2.log`, emulator attempt 1 at 17:36) was captured in
**both directions** by hooking the transport seal entry (`0x4307bea`, plaintext
in `rsi`) and by hooking the transport open (`0x4307cc8`, plaintext in `rsi`
after return). Parsing the datagrams (header `0x80 | flags<<4 | count`, u24 LE
sequence; frames `u8 flags, u16 BE bit length, reliability header, split
metadata`) gives this ordered application stream (order channel 0):

| order | app message | length | evidence (hook_open2.log) |
|---|---|---|---|
| 0 | `0x13` NEW_INCOMING (system) | 102 | `AESWRAP n=2` |
| 1 | `0xa7` (split id 4, 8 fragments) | 7443 | `AESWRAP n=21..28` |
| 2 | `0x90` (split id 5, 13 fragments) | 13153 | `AESWRAP n=29..41` |
| 3 | `0x92` | 10 | `AESWRAP n=41` |
| 4 | `0x8a` (split id 6, 2 fragments) | 1793 | `AESWRAP n=42..43` |
| 5 | `0x8f` | 2 | `AESWRAP n=43` |
| 6 | `0x9b` reply | 9 | `AESWRAP n=46` |
| 7+ | `0x83` replication | many | `AESWRAP n>=77` |

The same order (without `0xa7`) is visible in the earlier v66 capture:
`90(order1) → 92(2) → 8a(3) → 8f(4) → 9b(5)`. `a7` is optional/earlier.

The `0x9b` reply's exact wire datagram (real client, same session) is:

```
81 25 00 00 | 60 00 48 | 23 00 00 | 06 00 00 00 | 9b 23e81568 fa582465
```

i.e. one RELIABLE_ORDERED frame, msg #35, order index 6, channel 0 — matching
the static builder `0x6107184` (`$0x2` priority, `$0x3` reliability, channel 0).

## 2. CONFIRMED — `a7`/`90`/`8a` are plain counted arrays of 16-byte items

They are **not encrypted and not compressed**. The whole body is a counted
array of opaque 16-byte values:

```
0xa7 | u16 LE count | count * 16 bytes
0x90 |               count * 16 bytes   (count = (len - 1) / 16)
0x8a |               count * 16 bytes   (count = (len - 1) / 16)
```

Byte-checked by me on every captured instance:

| file | bytes | head | count rule | remainder |
|---|---|---|---|---|
| `analysis/join_plain/a7_321.bin` | 7363 | `a7` | count 460 == (7363-3)/16 | 0 |
| same-session `a7` (`same_sid4.bin`) | 7443 | `a7` | count 465 == (7443-3)/16 | 0 |
| `analysis/join_plain/90_359.bin` | 13025 | `90` | 814 items | 0 |
| same-session `90` (`same_sid5.bin`) | 13153 | `90` | 822 items | 0 |
| `analysis/join_plain/8a_363.bin` | 1745 | `8a` | 109 items | 0 |
| same-session `8a` (`same_sid6.bin`) | 1793 | `8a` | 112 items | 0 |
| third live session (`hook_open3.log`) | 7251 | `a7` | 453 | 0 |
| third live session (`hook_open3.log`) | 13137 | `90` | 821 | 0 |
| third live session (`hook_open3.log`) | 1793 | `8a` | 112 | 0 |

Entropy of the bodies is 7.89–7.99 bit/byte — i.e. the natural entropy of
random 16-byte hashes/GUIDs, not the signature of a block cipher. There is no
`28 b5 2f fd` (zstd), `04 22 4d 18` (lz4), gzip/zlib/brotli/lzma frame anywhere
in the bodies, and the engine's own `CompressBuffer`/`DecompressBuffer` path
(0x5865c3a / 0x5865df2) always requires the zstd magic — these messages never
touch it.

Content behaviour across sessions (same job, same account):

* `0xa7`: **mostly persistent**. In the compared pair, 431 of 460 items are
  identical; the first 429 items match, divergence starts at item 429
  (9 of the new items are absent from the old list). This is a cached list.
* `0x90` / `0x8a`: **per session**. Only 38 of 13024 / 9 of 1744 bytes coincide
  between two sessions (chance level), and the first items are entirely
  different. This is a per-connection list (instance hashes/GUIDs).

What is **not** proven: what a 16-byte item *is* (GUID vs content hash vs
`RBX::Guid`) and how the server consumes the lists. Replaying another session's
lists is not accepted by the server (see §5), which is consistent with the
per-session items being tied to server-issued state.

## 3. CONFIRMED — the `0x9b` exchange has no application cipher

Static path (instructions read in this session):

| address | role |
|---|---|
| `0x6104ae6` | packet-id dispatcher, jump table at `0xb27c08` (ids 0x92..0xa4) |
| `0x61054a3` | inbound `0x9b` case |
| `0x6106fb2` | field extractor: skip id, read `u32 A`, `u32 B`, then `readString` (`u32 n`, capped) |
| `0x6107184` | outbound builder + send |
| `0x61072cb` / `0x453243c` | lookup keyed by the packet's string `S`, default `"challenge"` (`0x4feb41`) |
| `0x61072fa` | `cvttsd2si 0x10(%rbx),%r12` — the looked-up double → int64 |
| `0x61073d2..0x61073fc` | append `u32 B` then `u32 X` (`r12d`) |
| `0x61074de..0x61074f1` | send priority 2, reliability 3, channel 0 |

Inbound layout:

```
9b | u32 A | u32 B | u32 n | S[n] | ...
```

Outbound layout:

```
9b | u32 B | u32 X          (9 bytes total)
```

`A` is only fed to a telemetry event (`0x6107222`), never into the reply.
There is **no AES/BLAKE2/HMAC/ChaCha call** on this path. The earlier docs'
"10-byte `9b` plaintexts" were actually `[AEAD AES OUT]` **ciphertexts** of
10-byte RakNet ACKs (`IN` partner `c0 00 01 00 ...`); the real reply is always
9 bytes.

`X` is produced by a per-thread lookup of a `double` keyed by `S` (default
string `"challenge"`), then truncated to int64 and written as its low 32 bits.
The semantic meaning of that map value is **UNCONFIRMED** (it is not a hash and
not a clock; see §6). A live dump of the degenerate builder call (A=B=0) showed
the looked-up `double = 998163170`; that is the only direct value observed, and
it does not establish the real challenge's map value.

## 4. CONFIRMED — four independent challenge → reply pairs

All four pairs come from one session each; `B` is copied verbatim from
`nonce[4:8]` in all four (not a hash).

| session | challenge size | nonce (8 B) | reply (9 B) | X | source |
|---|---|---|---|---|---|
| live #1 (same job) | 2025 | `8cee4721 23e81568` | `9b 23e81568 fa582465` | `fa582465` | `hook_open2.log`, S->C n=27 / C->S n=46 |
| live #2 | 2025 | `2cd6ff6f 458765f0` | `9b 458765f0 c67a78be` | `c67a78be` | `hook_open3.log`, S->C n=28 / C->S n=45 |
| v29 pcap | 2032 | `d96c6d69 ce2dc995` | `9b ce2dc995 23d55c38` | `23d55c38` | `raknet29.pcap` rec 368+369 → rec 377 |
| v27 pcap | 2037 | `6e33bd68 ed184a53` | `9b ed184a53 1f47e82a` | `1f47e82a` | `raknet27.pcap` rec 371+372 → rec 383 |

I re-checked the pcap bytes/offsets and the tuples myself: v29 challenge at
file offsets `0x117cc` + `0x11c7a`, reply at `0x1391a`, both on the same
5-tuple `128.116.44.33:51323 ↔ 10.0.2.16:46692`, reply 12.8 ms later; v27 at
`0x11d70` + `0x1221e` → `0x14ccd`, tuple `128.116.31.33:65481 ↔
10.0.2.16:52058`, reply 18 ms later. The echo relation holds 4/4.

Saved-pair hashes (all four files are in `/tmp/opencode/relre/`):

| file | sha256 |
|---|---|
| `pair_challenge.bin` (live #1) | `255e1de2c55f146be450ad9b6660bd5c289e750ecc1631c1eb3a90df424bd8f9` |
| `pair_answer.bin` (live #1) | `83555f34f3d1ce67a587f820a6b11dce3e07fc7e7252b16880d91d41e0519ad7` |
| `v3_SC_9b.bin` (live #2) | `33782ecc73ac6a3d84a3b85e84103499f164bc56d8396e898f7aa611561f1d02` |
| `v29_challenge.bin` | `5ad5eb4b9be8256f73089f78e41058ac070451d6e6a09477250ecc3e7b7775bd` |
| `v29_answer.bin` | `bffb1194f6cfcdd6f14fa2571042a3da8fae6a205e462925f134ab6eb59afb42` |
| `v27_challenge.bin` | `8dab6166dcc3edd2f02d3d9759ec30183b7fe10840ec6c89ea462de3084feeba` |
| `v27_answer.bin` | `ee8d6f32e7bf6690f281e4d347cf6b43d3e17e74a5b6d96b339de74a34d86b28` |

Provenance: the v27/v29 wire pairs were first recovered by a sub-analysis in
this session by AES-GCM-decrypting the pcaps with the SESSDER#2 keys logged in
`hook_v27.log`/`hook_v29.log`; I independently re-read the pcap records at the
offsets above, checked the 5-tuples and timestamps, hashed the files and
re-checked the `B` echo on all four pairs.

Raw challenge prefixes (full files are the four `*.bin` above):

```
live #1  9b 8cee4721 23e81568 dc070000 93dc9e973d2f804a21620b13cd23d3c7...
live #2  9b 2cd6ff6f 458765f0 dc070000 93dc9e973d2f804a21620b13cd23d3c7...
v29      9b d96c6d69 ce2dc995 e3070000 56cee505f05d4bd86450c08190d188ad...
v27      9b 6e33bd68 ed184a53 e8070000 a3232185cd080758110d8401bd045415...
```

(`n` = 0x7dc = 2012 for the live pairs, 0x7e3 = 2019 for v29, 0x7e8 = 2024 for
v27.)

Three live Rust-client sessions against the **same job**
(`jobId=637dda32-acd2-4642-801b-a9a8b827c1e3`, RCC `10.30.5.34:54190`) produced
different 8-byte nonces but the **byte-identical challenge body** after
`9b|nonce|u32 2012` — body SHA-256
`ad2f0c29888062796bf9f729ebe96a662171f6e2ea0375c273a3159bdc35c560`,
starting `93 dc 9e 97 3d 2f 80 4a ...`. The same full-body hash was also found
in the two independent app sessions (`hook_open2`/`hook_open3`) on that job.
So the body is per-job/static, only the 8-byte nonce changes. (Different jobs
have different bodies: e.g. `ec5f100f...`, `1a8360fc...`.) Note that two app
sessions with this identical body returned **different X** (`fa582465` vs
`c67a78be`), so X is not a function of the static body alone.

## 5. Replay experiments — what still fails, exactly

All runs are real joins from `examples/join_flow` with the account cookie and
the captured join blobs; the transport connects and the server sends `0x93`
then `0x9b` in every case.

| # | job / server | join data sent | reply | result |
|---|---|---|---|---|
| 1 | `637dda32…` / `10.30.5.34:54190` | `analysis/join_plain` `a7/90/8a` | `9b + nonce[0:8]` | `93` + `9b`, **disconnect** (wrong echo) |
| 2 | `637dda32…` same | `a7/90/8a` | `9b + nonce[4:8] + 00000000` | `93` + `9b`, **disconnect** |
| 3 | `91e03361-5dc9-46bd-a3c9-9ada5b8ed863` / `10.34.3.27:61745` | v66 set `90/92/8a/8f` | (never reached) | `93` only, **disconnect before 9b** |
| 4 | `637dda32…` same, targeted via `serverId` | `replay_same` `a7/90/92/8a/8f` | `9b + nonce[4:8] + 00000000` | `93` + `9b`, **disconnect** |
| 5 | `637dda32…` same, targeted | `replay_same` | `9b + nonce[4:8] + X_app(FA582465)` | `93` + `9b`, **disconnect** |
| 6 | `83b1f0d5-462e-46e1-baea-8ffab1c80ad9` / `128.116.5.33` | `a7/90/8a` | `9b + nonce[4:8] + 00000000` | `93` + `9b`, **disconnect** |
| 7 | `10.30.5.104:59702` | `a7/90/8a` | `9b + B + X_app(fa582465)` (wrong job) | `93` + `9b`, **disconnect** |

Step 3 is the key one: the same bytes that reach `9b` on their own job are
rejected **before** `9b` on another job, so the join lists/`92` are
server/job-bound. Steps 4–5 show that even on the **same job** with the
**complete same-session set** the reply does not pass, so there is a second
per-connection binding (X or the lists).

Therefore the echo rule and packet layout are reproduced exactly, but at least
one more per-connection binding exists. Candidates (both **UNCONFIRMED**):

* `X` is per-connection (a thread-local/telemetry double, not a job constant),
  or
* the join item lists / `0x92` are bound to the connection/server state, so
  replaying another connection's lists cannot complete the join.

The join API does accept `"serverId": "<jobId>"` (verified: it returns the
requested job), which is useful for reproducing a session but does not by
itself make the old join data valid.

## 6. Negative results — do not repeat

* The three 10-byte blobs `9b1f...`, `9b04...`, `9b51...` are **seal output
  ciphertext** of 10-byte ACKs, not replies. Neighbouring ACK ciphertexts start
  `c5 8f e9…`, `f4 1e 33…`; 1/256 of them start with `9b`. The real reply is
  9 bytes.
* `0x4307bea` is **not** the open path; it is a second seal (plaintext
  datagram in, ciphertext out, counter `+0x10`). The real open is `0x4307cc8`
  (checks `len>=0x12`, `+0x2b==0`, `+0x2a!=0`, reads the counter hint at
  `buf+len-0x12`, selects the AES/ChaCha open wrapper, writes plaintext back
  into `rsi`). Hooking `0x4307cc8` gives the complete inbound plaintext stream.
* No magicless-zstd frame (with or without the APK settings dictionary) at any
  offset 0..24 in the join bodies; no zlib/raw-deflate/lzma/bz2 frame at offsets
  0..63.
* `X` is not CRC32/CRC32C/Adler/FNV/DJB2/Murmur3/xxh32/xxh64, not
  MD5/SHA1/SHA256/BLAKE2b/BLAKE2s/SHA3 prefixes or suffixes, not HMAC/keyed
  BLAKE2/CMAC/AES-ECB/GCM/ChaCha with the object's `+0x40/+0x60` keys, and not
  a simple arithmetic/xor/byte-swap function of the nonce. All tested on the
  four real pairs.
* The map lookup is keyed by the packet's string `S`, default `"challenge"`;
  the string read is length-capped (32/46), so a 2012-byte body cannot be the
  key — the lookup falls back to the default. This is consistent with the
  observed behaviour but `X`'s source is still not proven.

## 7. Reproduce

```bash
# live app capture (both directions), then join a place in the emulator
frida -U -f com.roblox.client -l /tmp/opencode/relre/hook_open3.js -o /tmp/opencode/relre/hook_open3.log
adb shell am start -a android.intent.action.VIEW -d "https://www.roblox.com/games/606849621/"
adb shell input tap 690 1795

# reassemble both directions
python3 /tmp/opencode/relre/parse_log.py /tmp/opencode/relre/hook_open3.log

# Rust client replay (the example already supports RUSTBLOX_REPLAY_JOIN and
# RUSTBLOX_ANSWER_9B; RUSTBLOX_ANSWER_9B sends
# 9b | challenge[5..9] | 00000000):
RUSTBLOX_REPLAY_JOIN=/tmp/opencode/relre/replay_same RUSTBLOX_ANSWER_9B=1 \
  cargo run --example join_flow -- 606849621
```

For the same-job test, a **temporary diagnostic patch** (added and then
reverted) put `"serverId": "<jobId>"` into `base_body` in
`src/roblox/join/mod.rs`:

```rust
if let Ok(s) = std::env::var("RUSTBLOX_SERVER_ID") {
    body["serverId"] = serde_json::Value::String(s);
}
```

With that patch, `/v1/join-game` returned the requested job verbatim
(`jobId=637dda32-…`, `ip=10.30.5.34`, `port=54190`), which is how the same-job
replay tests were run. `RUSTBLOX_9B_TAIL` (our little-endian X injection) was
part of the same temporary patch and is not in the current example.

To capture the incoming `9b`, hook the **open** path, not the seal:

```js
Interceptor.attach(m.base.add(0x4307cc8), {
  onEnter(){ this.buf=this.context.rsi; this.len=this.context.rdx.toInt32(); },
  onLeave(){ /* plaintext = this.buf[0 .. this.len-0x12] */ }
});
```

## 8. Open questions

1. What is `X`? (Static: lookup of a `double` keyed by `S`, default
   `"challenge"`, then `(int64)`; live value observed only for the degenerate
   A=B=0 call.) A live dump of the real challenge's `double` at `0x61072fa`
   would settle it.
2. Does the server verify `X`? A real challenge with `X=0` and one with the
   app's `X` both disconnect; a controlled experiment on a **fresh** challenge
   with the client's own `X` is still needed.
3. What binds the join item lists to a connection? They are 16-byte opaque
   items, per-session for `90`/`8a`; generating them requires knowing the
   server-issued instance set, not a cipher.
4. Item semantics (`RBX::Guid`? content hash?) and the consumer at
   `0x6239902`.

## 9. The `0x92` / `0x8f` packets (observed, semantics UNCONFIRMED)

The real client sends these between the join lists and the `9b` reply. They are
small, per-session, and `8f` is constant:

| session | `0x92` (id + payload) | `0x8f` | order indices (channel 0) |
|---|---|---|---|
| v66 | `92 f4eea48fa4eaf99fd501` | `8f 00` | 90=1, 92=2, 8a=3, 8f=4, 9b=5 |
| hook_open2 | `92 cdbacb84f5b9f2d50c` | `8f 00` | a7=1, 90=2, 92=3, 8a=4, 8f=5, 9b=6 |
| hook_open3 | `92 8984dfd3b0d1b5a756` | `8f 00` | (same shape, a7 first) |

The `0x92` payload is 9–10 bytes and changes every session; `8f` is always
`8f 00`. Replaying v66's `92` together with v66's lists to a **different job**
makes the server disconnect before it even sends `9b`; sending the complete
`hook_open2` set to its **own job** reaches `9b` but still disconnects after the
reply. What `92` carries (a nonce? a key/registration? a cache token?) is
**UNCONFIRMED**; no cipher was found in it.

## 10. Hook ABI used for the live capture

Both functions live in the connected-phase `SessionCrypto` object
(`src/net/crypto` on the Rust side has the same shape):

```
seal 0x4307bea(rdi=obj, rsi=buf, rdx=len)
  - writes nonce at buf+len-0x12, counter at obj+0x10 (atomic xadd)
  - key slot = obj+0x40 + (obj[0x28] ? 0x20 : 0)   [flag0->+0x40, flag1->+0x60]
  - selects AES wrapper 0x43099ce or ChaCha 0x430b163 by obj[0x2d] & 2
  - INPUT (rsi) is plaintext; it is sealed in place

open 0x4307cc8(rdi=obj, rsi=buf, rdx=len)
  - requires len>=0x12, obj[0x2b]==0, obj[0x2a]!=0
  - reads the counter hint from buf+len-0x12 via 0x4307694
  - key slot = obj+0x40 + (obj[0x28] ? 0 : 0x20)   [flag0->+0x60, flag1->+0x40]
  - selects AES open 0x430b332 or ChaCha open 0x430ac43
  - AFTER RETURN rsi[0..len-0x12] is plaintext, buf+len-0x10 is the tag
```

The object dump taken at the first open in a live session gave
`obj+0x40 = 05e3fab0…748b9433`, `obj+0x60 = 61764442…0102a7db`, flags
`+0x28..0x2d = 00 00 01 00 00 02` (AES-GCM path). These are transport keys;
they do **not** open the `9b` body and are not used by the `9b` builder.

`0x4307bea` was previously mis-identified as the open path in
`SESSION-CRYPTO-RE.md`; the hook in `hook_v5.js` at that address therefore
dumped a second **seal**, not the incoming stream. The correct open is
`0x4307cc8`.

## 11. Evidence index / exact reproduction

Live app captures (Frida scripts write hex lines; `parse_log.py` reassembles):

```
hook_open2.log   emulator 2026-11-24 17:36, 13 866 datagrams (12 356 S->C,
                 1 510 C->S); challenge at S->C n=27, reply at C->S n=46
hook_open3.log   emulator 2026-11-24 17:45, challenge at S->C n=28,
                 reply at C->S n=45
```

Same-session join sets used in the replay tests:

```
/tmp/opencode/relre/same_sid4.bin   a7, 7443 B, 465 items
/tmp/opencode/relre/same_sid5.bin   90, 13153 B, 822 items
/tmp/opencode/relre/same_sid6.bin   8a, 1793 B, 112 items
/tmp/opencode/relre/same_single_92.bin  cd ba cb 84 f5 b9 f2 d5 0c (payload)
/tmp/opencode/relre/same_single_8f.bin  00
/tmp/opencode/relre/replay_same/        the same five files under the names
                                         examples/join_flow expects
```

Wire pcap pairs:

```
raknet29.pcap  rec 368 (0x117cc) + 369 (0x11c7a) -> challenge
               rec 377 (0x1391a)                  -> reply
raknet27.pcap  rec 371 (0x11d70) + 372 (0x1221e) -> challenge
               rec 383 (0x14ccd)                  -> reply
```

The 9b challenge layout confirmed against all four pairs:

```
9b | u32 A | u32 B | u32 n | S[n]
   A = nonce[0:4], B = nonce[4:8]
   reply = 9b | u32 B | u32 X
```

`sizes`: 2025 = 13 + 2012 (live), 2032 = 13 + 2019 (v29), 2037 = 13 + 2024
(v27). Different `n` values, same layout. The body is per-job: on job
`637dda32-…` it is 2012 B with SHA-256 `ad2f0c29…`; other jobs give other
bodies (`ec5f100f…`, `1a8360fc…`).

## 12a. Follow-up session (24 Nov 2026, later) — operational blockers found live

Live re-run on the same emulator/account, aimed at capturing a fresh `0x9b`
pair via the safe seal/open hooks (`0x4307bea`/`0x4307cc8`) while avoiding the
degenerate-call artefact in §3. Two **CONFIRMED** operational findings, both
reproduced twice:

* **Hooking `0x61072fa` (the `X` lookup) with a Frida `Interceptor.attach`
  reliably crashes the client.** Both attempts crashed identically:
  `SIGSEGV` (`SEGV_MAPERR`, fault addr `0x0`) in thread `RBX Worker C`, PC
  `libroblox.so+0x610745e` (14 bytes past the hooked address — consistent
  with Frida's inline trampoline corrupting a tight/hot call on a non-main
  thread). Confirmed as hook-induced, not a pre-existing client bug: the
  identical join (same place, same account, same emulator) completes
  normally with **no** crash when Frida is not attached at all. Do not hook
  `0x61072fa` with a plain `Interceptor.attach`; the safe path to recover `X`
  is still the seal entry (`0x4307bea`), which captures the outbound 9-byte
  reply (and therefore `X`) as plaintext without touching the lookup
  function itself.
* **The emulator/account used for this RE is now actively kicked by
  server-side anti-emulator detection**, independent of any join/crypto
  issue: `Disconnected — "We don't allow emulators for accounts without
  place edit access… (Error Code: 305)"`, observed twice, each a minute or
  so after a successful join (HUD and replication already flowing normally
  beforehand — `0x83` application traffic was seen going out just before the
  kick in one case). This is a plausible explanation for some of the
  session drops that were previously attributed only to the `9b`/list
  binding in §5 — at least some disconnects on this account may be this
  check, not the crypto. The error text's own wording ("accounts without
  place edit access") suggests a Studio/builder-access account would not
  trigger it; worth trying for future live captures instead of a plain
  player account.
* A same-job re-capture (fresh app process, fresh RakNet connect) reproduced
  the exact same challenge body hash (`93dc9e97…`) with a new nonce
  (`115b778e4b69db4f`), consistent with §4's finding that the body is
  per-job/static — a fifth independent confirmation, no new information.
  The corresponding `0x9b` reply was not captured this run (the session was
  kicked by the anti-emulator check before 500/20000 SEAL calls had
  elapsed, or the client silently resumed a cached session without a full
  re-handshake on one attempt — both are plausible and were not
  disambiguated).

Net effect: `X` is still unknown. The next attempt should (a) use an account
with place-edit access to avoid the 305 kick, and (b) hook only
`0x4307bea`/`0x4307cc8` (never `0x61072fa` directly) with a seal-call cap
high enough to survive past the 9b reply (order index 6, but hundreds of
transport ACK seals precede it).

## 12. Sub-analysis provenance (what came from where)

| report | contribution | how I treated it |
|---|---|---|
| `/tmp/opencode/relre/agent-9b.md` | static path of the `9b` handler/builder, addresses, layout, no-crypto proof | read the report; re-read the key claims against the live pairs and the 4-way echo rule; the addresses are listed above |
| `/tmp/opencode/relre/agent-joinformat.md` | 16-byte item-list hypothesis and framing of `a7/90/8a` | re-measured all lengths/counts on 5 session captures myself |
| `/tmp/opencode/relre/agent-artifacts.md` + `pair_report.py` | recovered the v27/v29 pcap pairs by GCM-decrypting the captures | re-read the pcap records at the given offsets, checked tuples/timestamps, hashed the files, re-checked the `B` echo |
| `/tmp/opencode/relre/agent-bitpack.md` + `bitpack_verify.py` | `8d01` outer header, inner tag layout, handler addresses | independently re-verified the `8d01` header on 2999/2999 messages and the inner tag/flag statistics on the 2999-body corpus; addresses spot-checked (jump table, `0x626f5ac`) |

Where a claim could not be re-checked by me (e.g. the exact consumer of the
`8d01` bulk blob), it is explicitly marked **UNCONFIRMED** in
`docs/REPLICATION-DECODE-RE.md`. No sub-analysis result was copied into the
repo as code without the data-level check above.
