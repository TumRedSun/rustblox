# SessionCrypto RE — Roblox Android client build 2.748.1527

Reverse-engineered live (Nov 2026) from `libroblox.so` of the current Android client
(APK 2.748.1527, `so` sha256 `ca6822c7…`, file at `/storage/rblx_debug/apk_new/libroblox.so`).
Method: `objdump -d` xrefs on the `movabs $0x754e657571696e55` immediate (= ASCII
"UniqueNu" LE — the counter/nonce base; the string "UniqueNumber" itself is NOT in
.rodata), then Frida hooks on the found functions during a real game join on the
Android emulator. All addresses below are file-offset == vaddr (first LOAD maps 1:1).

Everything here was confirmed on a live join unless marked "static".

## Function map (new build)

| Address | Role |
|---|---|
| `0x42d6xxx` | OpenReply2 handler: reads flag `*(byte*)(rbx+0x8a8)`, runs the KDF, adopts the secret into SessionCrypto, builds the nonce context |
| `0x43100d2` | KDF wrapper `(out_vec=rdi, flag=esi)`: `0x430f5dc` (provider getter, lazy global `0x71fbbf0`/init-gate `0x71fbc00`) → `0x430f130(dst, provider, ctx)`; writes a fixed 32-byte `.rodata` block `d371cb6e…463d` (`0x97ee00`) in its output paths — same constant as the old build's fake-setKeys, NOT the session key |
| `0x430f130` | **COMBINE / real KDF**: `(dst_vec, src=provider, ctx=labels)`. In a live join ctx holds two label strings: `"RakNetEarlyPublicKey"` and `"RakNetEarlySecretKey"` — the provider's slots for the client's early X25519 pair. Slot lookup by hash (`0x1ffd52a`), HKDF-like expansion inside |
| `0x430fca2` | **adopt**: `(crypto_obj, secret, len)`. Sets `+0x28=0`, runs derive-init, on `len==0x20` copies secret → `sub+0x40/+0x50`, then `0x4308140(dst1=sub+0xe0, dst2=sub+0xc0, sub, seed=sub+0x20, secret)` → two 32-byte session keys. Success `+0x29=1`, failure `+0x2b=1` |
| `0x430760a` | derive-init: zero flags `+0x29/+0x2b`, alloc 0x160 sub-object (4×32B slots, `0x43073cc`), store at `crypto_obj+0x20`, provider init `0x3f5dc46`, vtcall#1 → random 32B seed into `sub+0x20`, vtcall#2 installs |
| `0x4307906` site | steady-state **seal**: key = `crypto_obj+0x40 + *(byte*)(obj+0x28)*0x20`, counter = atomic `obj+0x08`, nonce written into the packet buffer = `LE64(0x754e657571696e55 + counter) + "mber"` (`0x7265626d`, lowercase), then AEAD call `0x430b163` |
| `0x4307bea` | steady-state **open**: `(crypto_obj, buf, len)`; requires `len ≥ 0x12`, `+0x2b==0`, `+0x2a!=0`; key at `+0x40/+0x60` by flag `+0x28`; recv counter = atomic `obj+0x10`; nonce = `LE64(counter)+"mbeR"` (`0x5265626d`, CAPITAL R — different from the seal side!) written at `buf+len-0x12`; tag at `buf+len-0x10` |
| `0x43099ce` | AEAD wrapper, AES-GCM path (chosen when `crypto_obj+0x2d & 2` — the `rakIsAesGcm` flag; live session had `0x2d=0x02` → AES) |
| `0x430b163` | AEAD wrapper, ChaCha path (`+0x2d & 2 == 0`) |
| `0x4307694` | counter clamp/sync from low16 packet hints (writes base "UniqueNu"+"mbeR" nonce form) |
| `0x43075d8` | nonce-context constructor (vtable `0x6cb9550`, "UniqueNu" at `+0x10/+0x18`) |

AEAD wrapper ABI (both paths, **corrected** from live captures v16–v19; stack args are
arg7..arg10 at `[rsp+8]/[rsp+0x10]/[rsp+0x18]/[rsp+0x20]`):

```
f(rdi=ct_ptr, rsi=tag_ptr, rdx=out_ptr/ctx, rcx, r8=ct_len,
  r9=aad_ptr, [rsp+8]=arg7(aad_len), [rsp+0x10]=arg8,
  [rsp+0x18]=nonce_ptr(arg9), [rsp+0x20]=key_ptr(arg10))
```
Inside the wrapper the callee reads key from `rbp+0x20` (arg9) and nonce from
`rbp+0x28` (arg10). Note: the CHA wrapper MACs the AAD from `(r9, arg7)`;
the steady open passes `r9=0, arg7=0` → AAD empty there (but see the AES tag
quirk below — the AES steady layer really is AAD-empty).

## SessionCrypto object layout (confirmed by live dumps)

```
+0x00  vtable
+0x08  send/recv counter (u64, lock xadd; the seal side uses +0x08,
       the open side +0x10 — two independent per-direction counters)
+0x10  second counter (starts as "UniqueNu" = 0x754e657571696e55)
+0x18  "UniqueNu" (u64)
+0x20  ptr → sub-object (0x160) with key derivation scratch:
        +0x20 seed(32B random), +0x40/+0x50 adopted secret,
        +0xc0/+0xe0 derived key pair (via 0x4308140)
+0x28  key-slot flag (0 → slot +0x40, 1 → +0x60)
+0x29  ready, +0x2a ready2, +0x2b error, +0x2d bit1 = rakIsAesGcm
+0x40  key slot 1 (32B), +0x60 key slot 2 (32B)
```

## Crypto library bundle (static map, one region of .so)

- AES/GCM code: `0x2950000–0x2980000` (642 `aesenc`/`pclmulqdq` instructions)
- `0x29577d0` AES key expand `(key=rdi, bits=esi ∈ {0x80,0xc0,0x100}, sched=rdx)`
- `0x2953a40` / `0x2953a90` AES enc/dec block `(in=rdi, out=rsi, ctx=rdx)`;
  `ctx[0..0x20]` = raw key, rounds at `ctx+0xf0` (13 loops = AES-256, 9 = AES-128)
- BLAKE2b cluster around `0x2a43390`; IETF ChaCha20 `0x2a4b120` (key=rcx,
  ctr+nonce=r8) + AVX2 variant `0x2a4c7c0`; original-DJB chacha `0x3f5dcd2`
  (8-byte nonce, stack-protector prologue); chacha state setter `0x63b9ec8`
  `(ctx=rdi, key=rsi[32], nonce=rdx[12])` — fired only with random nonces in
  live joins (not the handshake boxes)

## Live-captured values (one join, v5 hook cycle)

- KDF-OUT (adopted session secret): `9bcf4240a2b618aa64ba57609160117828d4aeb3816ae23cf77dc250442ddc55`
- derive seed (random, sub+0x20): `0f59b2624ee7cd0bc3b4a0d1975516d0e3daae6f6176d395b3073e33fc2c1ba8`
- SessionCrypto slots: `+0x40 = 06cb6fa94a7d8c8edac9a601786d5e41e4b19ff50fbf0a6e710a11e421cbb152`,
  `+0x60 = b10c4a2d8fc5dfe153ee7b68169b394e15ec6e4a4e4cc078545d0dbb861ac1bd`
- first seal call key (stack arg): `e69e081e591c6ddaef9f56b8d1d69d62b0955345c98db0de644a984a6b90055`
  — does not match the two slots; its single call had `len=93`, ct starting
  `100ddbc750b30896…` and a `[rsp+0x18]` pointer to a buffer with ASCII
  `"UniqueNumber"+zeros` (identification pending)
- wire (same join): OCR1 1160B / REPLY1 32B / OCR2 **1160B** (`cpub`, box 128B,
  ASCII `"UniqueNumber"`, uniq 16B, then 918B of zero padding) / ORP2 158B
  (`sp2`, box 54B, `"UniqueNumber"`, uniq 16B) — pcap LINKTYPE_LINUX_SLL2

## Negative results (do not retry)

- The adopted secret does NOT open the OCR2/OpenReply2 boxes with nonce
  `"UniqueNumber"` (AES-GCM & ChaCha20-Poly1305, several AADs).
- Session slots are not plain hashes of the secret (sha256, blake2s, blake2b-256,
  blake2b-512 halves, sha512 halves) — they come out of `0x4308140(seed, secret)`.
- ChaCha entries `0x2a4b120` / `0x2a4c080` / `0x3f5dcd2` are silent through whole
  joins (TLS/other subsystems); the `0x63b9ec8` setter fires with random nonces.
- ~2500 AES keys captured from the HTTPS/join-script/telemetry layers open none
  of the handshake material.

## Open questions / next

(All four original questions are now CLOSED — see "CONFIRMED FORMULAS (day-3)"
below. Kept for history: ~~1. what encrypts the OCR2/OpenReply2 boxes~~ —
answered: the tests used the wrong AEAD variant and wrong AAD;
~~2/3. exact math of COMBINE/SESSDER~~ — SESSDER fully confirmed, COMBINE
still open; ~~4. port into rustblox~~ — pending, now unblocked.)

---

# CONFIRMED FORMULAS (23 Nov, day-3 — v16–v21 hook cycles)

Everything below was verified **offline** against engine-captured ground truth
(exact byte-for-byte matches), not just read from disassembly.

## 1. Steady AES layer = STANDARD AES-256-GCM (wire-validated)

The engine's AES path (`0x43099ce`, chosen when `crypto_obj+0x2d & 2`):

- **Standard AES-256-GCM**: tag over the CIPHERTEXT, AAD empty.
- Keystream = standard GCM CTR (J0 = nonce||0x00000001, data blocks from
  nonce||0x00000002).
- Wire container per datagram: `ct || hint(u16 LE) || tag(16)`.
- Nonce: `LE64(0x754e657571696e55 + ctr) || "mbeR"` in both directions;
  the hint (low 16 bits of BASE+ctr) rides in the container for sync.
- The earlier "GHASH over the plaintext" reading was an artifact of the Frida
  hook: on the seal path the wrapper's `rdi` is the PLAINTEXT buffer (the
  engine seals in place), so the post-call buffer is the ct — the validation
  that "matched a GHASH over pt" was in fact the standard GHASH over ct.
  Live validation: real wire datagrams (OCR2 tail / CREQ / server replies)
  decrypt with `AESGCM(key).decrypt` directly.

## 2. ChaCha path = genuine RFC 8439, but AAD = the packet header

The ChaCha wrapper (`0x430b163`):

- polykey = `chacha_block(key, nonce, ctr=0)[0:32]` (verified == captured poly key)
- ct = `chacha(key, nonce, ctr=1..) XOR pt` (verified == engine ct)
- tag = `poly1305(polykey, pad16(AAD) || pad16(ct) || LE64(aad_len) || LE64(ct_len))`
  (verified == engine tag)
- **AAD = the whole RUPP envelope header preceding the ct** (90 bytes in the
  captured join: `78 00 ff ff 00 | fe fe fe fe | fd fd fd fd | 12 34 56 78 |
  03 | 5a 00 | 5d 00 | 05 | …`). The frame sub-header embeds its own lens:
  `5a 00` = 90 = aad len, `5d 00` = 93 = ct len. A second captured seal had
  envelope `7d 00 ff ff …` with aad_len 44 (`2c 00`) — the envelope type
  byte varies (`78`/`7d`).

This is why every earlier "AAD=empty" test on sealed material failed.

## 3. Nonce formulas (final)

- BASE = `0x754e657571696e55` ("UniqueNu" as LE64).
- seal (client→server): nonce = `LE64(BASE + ctr) || "mber"`, ctr = atomic
  counter at `crypto_obj+0x08`; first packet = ASCII "UniqueNumber".
- open (server→client): nonce = `LE64(BASE + ctr) || "mbeR"`, ctr = atomic
  counter at `crypto_obj+0x10`; first packet = ASCII "UniqueNumbeR".
- Per-direction counters, per crypto object. Wire hint for counter sync:
  low16 of BASE+ctr appears in packet headers (`0x4307694` clamps/syncs).
- Cipher choice: `crypto_obj+0x2d & 2` → AES variant, else ChaCha. **Two
  crypto objects exist per session**: the envelope/early layer (ChaCha in
  our joins) and the steady RakNet layer (AES in our joins) — the same
  session used ChaCha for one layer and AES for the other.

## 4. Session key derivation chain (per join, live-captured end to end)

```
COMBINE 0x430f130   (ctx label "(RakNetEarlyPublicKey…" — domain-separated
                     labeled KDF; exact math still open)
ADOPT 0x430fca2     installs early key material (ADOPT's rsi arg ==
                     seed of SESSDER call #1)
SESSDER 0x4308140   called TWICE per join:
  call#1 → (out1, out2): out2 == the ChaCha envelope-layer seal key
           (== [AEAD-CHA #1] key of the same join)
  call#2 → (out1, out2): out2 == the steady AES key
           (== [AEAD-AES #2..#4] keys of the same join)
```

**SESSDER formula (fully confirmed offline, v20+v21; semantics finalized v27):**

```
SESSDER(dst1=rdi, dst2=rsi, eph_pub=rdx, eph_priv=rcx, server_pub=r8):
    Z = X25519(priv = clamp(eph_priv), pub = server_pub)
    H = BLAKE2b-512(Z || eph_pub || server_pub)
    dst1 = H[0:32]     # opposite direction
    dst2 = H[32:64]    # the AEAD key actually used by seal/open
```

The "secret" argument is NOT a random secret: it is the **client's ephemeral
X25519 PUBLIC key**. Verified live (v27): `X25519-pub(sub) == the value in the
`secret` slot == the bytes the client puts into its own envelope header on the
wire (OCR2 header offset +21..53)`. The eph pair is generated locally per join
(the derive-init random seed); the server learns the pub from the wire. The
server mirrors the derivation with `X25519(server_priv, eph_pub)`.

Live ground truth (v20): secret `2600cb0b…` and sub `576bc63f…` are the same
in both calls; the seed differs: call#1 seed `8d736991…` (== ADOPT's rsi arg),
call#2 seed `36db11c7…` (origin TBD). Outputs: call#1 →
`e31804db… / fb99f2fc…` (fb99f2fc = CHA seal key), call#2 →
`64a7cbcf… / 139a3adb…` (139a3adb = steady AES key). Independently confirmed
on a second join (v21): Z and BLAKE2b traced internally
(`X25519(sub,seed) == Z` ✓, `BLAKE2b(Z||secret||seed) == H` ✓).

Internal notes: X25519 wrapper = `0x3f692f4` (out=rdi, sub-obj=rsi, seed=rdx;
rejects all-zero output, prov-method `*0x70307a8`); BLAKE2b init/update/final
= `0x2173ce5` / `0x2173d5e` / `0x2173ecc` with param block
`40 00 01 01 …` (digest 64, fanout/depth 1 — standard unkeyed blake2b-512).

## 5. Corrected interpretation of earlier negatives

- "Adopted secret does not open the boxes": the sealed material was fine —
  the tests used standard GCM (wrong tag construction, §1) and/or empty AAD
  (wrong for the envelope layer, §2).
- The `e69e081e…` "first seal key" was the ChaCha envelope-layer key of that
  join (SESSDER call#1 out2) — not steady, not an unknown.

## Ground-truth regression vectors (offline test data)

Stored in `/storage/rblx_debug/session-crypto/logs/` (hook_v18b/v20/v21.log):
- v18b ChaCha seal: pt/ct/nonce/key/polykey/AAD90/tag — full byte-exact roundtrip.
- v18b AES open: ct/pt/key/nonce/tag — full byte-exact verification.
- v20/v21: two full SESSDER quadruples (inputs + outputs + internal Z/H).

## Remaining work (to finish rustblox connect)

1. **Origin of SESSDER inputs**: where `secret(rdx)`, `sub(rcx)` (our priv)
   and `seed(r8)` come from — presumably the join-ticket `ephPubKey` + the
   client's ephemeral pair via COMBINE/ADOPT. Hook the COMBINE label-lookup
   (`0x1ffd52a`) and dump provider slots at KDF time.
2. **COMBINE (0x430f130) exact math** — labeled HKDF-like expansion
   (`"RakNetEarlyPublicKey"` / `"RakNetEarlySecretKey"` slots).
3. Implement in `src/net/crypto`: the two AEADs (§1, §2), nonces (§3),
   SESSDER (§4), COMBINE once decoded; then the join flow: send client pubs
   in the join script, feed the join-ticket pub into the chain, live-verify
   the CONNECTION_REQUEST path.
4. Early-box layer (OCR2/OpenReply2 sealed blobs) — should now decrypt with
   the ChaCha formula + envelope AAD once the early key is derived (the
   envelope layer key IS the early key — SESSDER call#1 out2).

Live infrastructure notes (emulator `-gpu host`, adb root + root frida-server,
blind UI navigation recipe, tcpdump SLL2 parsing, Frida pitfalls) are recorded in
`todo.md` (session "ПРОРЫВ (23 ноя, день-2)").

---

# WIRE MAP + SEED ORIGINS (23 Nov, day-3 continuation — v22–v29)

## Where the two server pubs come from

- **seed1 (envelope layer)** = the server's long-lived early pub, delivered in
  the join response:
  `joinScript.ClientPublicKeyData` → JSON → `applications.RakNetEarlyPublicKey`
  → `versions[0].value` (base64, 32B; version `id: 5`, `creationTime` 2022 —
  the server process's stable key). This is the slot the `COMBINE` resolver
  (`0x430f130` — actually a **slot lookup by usageType/application/version**,
  not a KDF) returns into `ADOPT`; the ADOPT input == SESSDER call#1 seed.
  Stable across joins to the same server instance (observed: `9bcf4240…`,
  `5896050e…`, `f374ee53…`, `b6d5994b…`, `c6666954…` over multiple days —
  these are the same values the old notes called "KDF-OUT secrets").
- **seed2 (steady layer)** = a per-join server pub that arrives **RAW on the
  wire** inside the OpenReply2 packet header (right after the `12 34 56 78`
  marker and the `01 2c 00 3a` bytes; 32 bytes). Candidate duplicate:
  `joinScript.EphemeralEarlyPubKey` (32B, base64+URL-encoded) — cross-check
  live.
- The client's own ephemeral pair is generated locally per join; its pub is
  transmitted in-band in the envelope header (see below). Nothing needs to be
  sent in the join request for the out-of-band flow.
- The 64-byte `joinTicket.EncryptedServerFields.ephPubKey` is separate from
  both seeds (verified: neither half equals `EphemeralEarlyPubKey`); it belongs
  to the early-box path (see below).

## Wire templates (from live pcaps, LINKTYPE_LINUX_SLL2)

Client RUPP prefix (31B, before every client packet):
`01 00 00 1f | 01 | 11 | 01 | <16B per-session random> | 02 06 0a 22 08 11 ff c9`
(the last 8 bytes vary per session too). Server prefix: `01 00 00 17 | 01 | 11 | 02 | <16B marker>` (23B).

1. **OCR1** (client, ~1160B): prefix + `7b 00 ff ff 00 | fe fe fe fe | fd fd fd fd | 12 34 56 78 | 05 | 01 00 00 00 | <32B zeros>` + zero padding (MTU probe, not encrypted).
2. **ORP1** (server, 32B): `01 00 00 04 | 7e | 00 ff ff 00 fe fe fe fe fd fd fd fd 12 34 56 78 | <marker> | 00 04 b0`.
3. **OCR2** (client, **1160B**, NOT 520B — corrected 24 Nov after a live A/B):
   prefix + `78 00 ff ff 00 | <seq8> | 12 34 56 78 | 03 | 5a 00 (hdr len 90) | 5d 00 (ct len 93) | 05 | <32B = eph_pub> | <37B rest of header> | <93B ct> | "UniqueNumber" (12B) | <16B tag>` + zero padding. The ct is the ChaCha-sealed caps datagram (pt starts `100ddbc750b30896…`); key = SESSDER#1 out2, AAD = the 90-byte header. **Verified by decrypting a live capture.** The frame content is 242B; the datagram is padded with zeros to **1160B, exactly like OCR1** — the RCC silently drops a shorter OCR2.
4. **ORP2** (server, 158B): 23B prefix + `7d 00 ff ff 00 | <seq8> | 12 34 56 78 | 01 | 2c 00 | 3a | <32B seed2 RAW> | <57B ct> | "UniqueNumber" (12B) | <16B tag>`. The ct = the "early box" — NOT decryptable with SESSDER keys; it belongs to the old pair-KDF path (client persistent pair + the ticket's 64B ephPubKey). Not required for the out-of-band flow.
5. **CREQ** (client, 83B): prefix + 52B body (sealed, no envelope magic). Exact format still open (see "open" below).

## Status of the steady-layer wire container

The steady AES cts (e.g. `810000004000c000…`) are NOT found raw in the pcaps
— the steady datagrams are wrapped by another layer before hitting the wire
(or reassembled from fragments). Under investigation; the envelope-layer
seal/open is fully proven, the steady AEAD itself is proven (v18b), only the
container layout is pending.

## Open items

1. CREQ 52-byte body format (which seal/keys/nonce/AAD) — blocks the final
   online check step.
2. Steady wire container (outer wrapping) — needed to decrypt post-connect
   server traffic.
3. Live confirmation of `seed1 == ClientPublicKeyData value` and
   `seed2 == ORP2 header / EphemeralEarlyPubKey` (the online script tests
   this against the real server).

## seed2 recovery (complete, byte-exact vs v27/v29 live sessions)

The steady-layer server pub is split across two channels:

```
seed2 = ORP2_payload[21 : 44]                     # 23 bytes, CLEAR (envelope AAD)
      + ORP2_box_plaintext[0 : 9]                 # 9 bytes, sealed
ORP2 box: key = SESSDER#1(eph_priv, eph_pub, seed1).out1   # SERVER->CLIENT direction!
          nonce = in-band ("UniqueNumber"), AAD = payload[23:67] (44 B header),
          ct = payload[67:-28], tag = last 16
```

Verified: v27 ORP2 decrypts with out1 (`8795dcd2…`) to a 63-byte plaintext
starting `f06e5def6a7009e146` (= the missing seed2 tail) + the server's caps
blob `100ddbc750b30896…`. The `--selftest` of
`scripts/online_join_check.py` contains this vector.

Key direction rule (wire-correlated): SESSDER out1 = server→client (client
opens with it), out2 = client→server (client seals with it), for both the
envelope layer (SESSDER#1) and the steady layer (SESSDER#2).

## Remaining blocker: the OCR2 paBlob (33 B, header[57:90])

The RCC silently drops an OCR2 whose paBlob is wrong (verified: 8+ variants
tried, plain OCR2 also dropped). The paBlob builder lives in the RUPP layer
above `0x4307700`; RE in progress (analysis/paBlob.md). Everything else in the
handshake is proven: join → OCR1 → ORP1 → (OCR2 w/ valid paBlob) → ORP2 →
complete seed2 → SESSDER#2 → CREQ (`81…09|id8|00*6|2B|00|374f5e116c45`,
steady-sealed) → server steady replies.

## Last-mile status (23 Nov, late): everything proven, one gate left

Confirmed and integrated into `scripts/online_join_check.py` (selftest 23/23):

- **paBlob (33 B, OCR2 header[57:90]) = base64decode(ClientTicket.split(';')[2])**;
  `header[55] = int(ClientTicket.split(';')[-1])` (authVer = 18 = 0x12),
  `header[56] = 0x21`. Proven static (0x61c7bc7 getter → 0x60fb1c0 split →
  authobj) and live (hook_v33/v34/v35: byte-identical).
- **seed1 = joinScript.EphemeralEarlyPubKey** (per-server; live-proven: equals
  the ADOPT/SESSDER#1 seed of the same session, hook_v35).
- **seed2 = ORP2[21:44] (23 B clear) + ORP2 box plaintext[0:9]**; the box opens
  with SESSDER#1 **out1** (server→client), nonce in-band, AAD = ORP2[23:67].
  Verified byte-exact (v27, v35); `orp2_seed2()` + selftest vector.
- **pt93 byte map** (2 captures): `[0:8] 100ddbc750b30896 | [8:16] session id |
  [16:18] MTU | [18:20] 02 04 | [20:22] 7f 8b | [22:24] var | [24:26] relay
  port BE16 | [26] 42 | [27] 06 | [28:93] caps payload/noise`.
- **CREQ**: steady-sealed 34-B datagram `81 00 00 40 00 c0 00 00 00 00 09 |
  session id | 00*6 | 01 02 63 00 | 374f5e116c45`, AES-256-GCM out2, ctr 0,
  nonce "UniqueNumber".

Remaining gate: the RCC accepts the *real* client's OCR2 (structurally
identical packet, valid token/paBlob/seed1/keys/session id) but silently drops
ours. Next diagnostic step: run the emulator join with a pcap AND this script
against the same place at the same time, then byte-diff the two decrypted
OCR2s (full packet incl. RUPP prefix) to find the discriminating field.

## Live-join gate — final experiments (23 Nov, night)

Accumulated evidence (all on the SAME server, same second where applicable):

- The RCC answers the app's OCR2 and drops ours even with:
  * the app's own fresh join values (token, seed1=EphemeralEarlyPubKey,
    paBlob=ClientTicket part2, authVer) — captured live via the getter hook;
  * the correct udmux (discovered per private prefix: 10.30 → 128.116.13.34,
    repo table 17 → 128.116.21.33; the udmux port == the RCC port);
  * the app's own 67-byte pt93 caps payload (captured from a past seal);
  my OCR2 stays byte-identical in every constant field, valid keys, valid tag
  (verified by re-sealing real packets), yet is silently dropped.

- Additional facts learned:
  * The app's UDP is emitted by a SEPARATE process ("rkstack.process") — libc
    sendto hooks in the app process never see it; guest iptables DROP works and
    keeps the join session unused.
  * The app does not seal its OCR2 until it receives ORP1 — with its UDP blocked
    there is no pt93 to capture in the same cycle.
  * Udmux proxy table additions: 10.30.x → 128.116.13.34 (port = RCC port).
  * `EncryptedServerFields` is a NIST P-256 box addressed to the server (the
    client never opens it; 0 calls live) — irrelevant for the client flow.
  * The client has hardcoded static early pairs in .so:
    RakNetEarlySecretKey e3446c52… ↔ cbb18f91…; RakNetEphemeralEarlySecretKey
    3cdbcc14… ↔ d371cb6e… (the old "constant").
  * ClientPublicKeyData (272d2e5c…) is a GLOBAL Roblox key, not per-account.

Conclusion: every observable component of the OCR2/L handshake is reproduced
byte-exactly; the remaining acceptance gate is not in the packet bytes, values,
or keys — it must be tied to the identity of the joining HTTP session as seen by
the backend (TLS/HTTP fingerprint or device attestation), i.e. the RCC only
provisions a session for joins that look like the real app end-to-end.

## FINAL: the join gate is closed (24 Nov, live, reproducible)

The standalone client now completes the real RakNet-fork handshake against live
Roblox gateways with **no hardcoded keys** — `scripts/online_join_check.py` exits 0
with `*** steady key CONFIRMED` on 3/3 consecutive live runs (8 more runs before
that). Two concrete defects were the whole remaining gate:

1. **OCR2 datagram length must be 1160 bytes.** The frame itself is 242B
   (31 prefix + 90 header + 93 ct + 12 nonce + 16 tag); the real client pads it
   with 918 zero bytes to the same 1160B as OCR1. Every pcap (raknet27/29/35/37)
   shows `0x7b` and `0x78` both at 1160B. Sending the frame at 520B (an old
   misreading) makes the RCC drop it with no reply.
2. **seed1 must be `EphemeralEarlyPubKey`, and the first OCR2 of a session is
   the only one that counts.** `seed1_candidates` used to try
   `ClientPublicKeyData` (a global Roblox key) first; that wrong OCR2 consumes
   the session, so the subsequent correct attempt is dropped as well. With the
   correct key first the ORP2 arrives immediately.

Supporting facts established in the same session:

- The gateway is **not** stateless: `OCR1 -> ORP1` is answered only for a valid
  `TokenValue` **and** the correct private RCC address in the RUPP prefix.
  Random token, or the right token with a bogus `10.x` target both give silence.
- ORP1 comes from the gateway (4-byte reply prefix `01 00 00 04`); ORP2 carries
  the RCC's own 16-byte token in a 23-byte prefix `01 00 00 17 | 01 11 02 | t16`.
- ORP1 carries an 8-byte session marker that is **not** echoed anywhere in OCR2
  (verified by decrypting the pcap OCR2 with the hook key and comparing).
- `pt93[24:26]` is the RCC port (confirmed against a paired hook capture);
  `pt93[8:16]` is a client-chosen per-connection id, not a join-script field.
- Full steady phase works: after CREQ the server returns real RakNet frames
  (`81 …` connection-accepted, `d0 …` acks) sealed with the SESSDER#2 keys.

### udmux proxy table (private prefix -> public proxy, same port)

| 2nd octet | proxy | source |
|---|---|---|
| 8 | 128.116.95.33 | repo |
| 9 | 128.116.44.33 | repo + pcap |
| 17 | 128.116.21.33 | live join |
| 20 | 128.116.50.33 | repo |
| 30 | 128.116.13.34 | live join |
| 34 | 128.116.31.33 | repo + pcap |
| 206 | 128.116.5.33 | live join |

The authoritative source is always the join response's `UdmuxEndpoints`
(`join: udmux endpoint …`); the table is only a fallback.

## Rust port (same night) — `RakClient::connect` connects live

`src/net/crypto` + `src/net/raknet/handshake` were brought in line with the
verified flow; `cargo test` is green (82 lib + 17 integration tests) and
`cargo run --example join_flow -- 606849621` prints **CONNECTED** on 3/3 live
attempts (fresh join each time, real RCC, no hardcoded keys).

Changes that mattered:

- `crate::net::udmux`: token type byte **0x01** (was 0x02) in `RUPP_SUBFLAGS`
  and `client_envelope`; live envelope is
  `01 00 00 1f 01 11 01 <token16> 02 06 <rcc ip4> <port BE16>`.
- `crypto::PT93_CONST` corrected to `10 0d db c7 50 b3 08 96` (was
  `00 0d … 08 92`), and the pt93 layout is now the real one:
  `const(8) | session id(8) | mtu BE16 | 02 04 | 7f 8b | rand(2) |
   RCC port BE16 | 42 06 + rand(65)`.
- `EarlyCrypto::build_ocr2(mtu, rcc_port)`; the datagram is padded to
  1160 bytes (frame = 242 bytes).
- OpenReply2 is parsed with the live split: `73`… actually `7d | magic |
  01 2c 00 3a | clear23 | ct(63) | "UniqueNumber" | tag(16)`; the 32-byte
  SESSDER#2 peer = the 23 clear bytes + the 9 leading plaintext bytes of the
  box, which is opened with SESSDER#1 **out1** and AAD `body[0..44]`.
- The post-OpenReply2 layer is **AES-256-GCM**, not ChaCha20: nonce
  `LE64(0x754e657571696e55 + counter) || "mbeR"`, wire
  `ct || hint(u16 LE) || tag(16)`, AAD empty (`crypto::counter`).
- The first steady frame is the 34-byte fork CONNECTION_REQUEST
  `81 00 00 00 40 00 c0 00 00 00 09 | session id(8) | 00*6 | fd fc | 00 |
   37 4f 5e 11 6c 45` (session id = the one in pt93).
- The connect loop treats any opened steady datagram that is not a bare
  ACK (0xc0/0xa0/0xd0) as acceptance, because the fork wraps its messages
  in `0x80|count` datagrams rather than putting the ACCEPTED id at byte 0.

Known next gap (not the handshake): the connected-phase game-frame decoder
still reports `decoded=0 skipped=11` — its `0x80|count` + per-message
`flags/len` model does not yet parse the replication packets, so no
instances/events appear yet.

## How to reproduce (both clients, no hardcoded keys)

Python reference (the deliverable online check; 23/23 self-tests, live exit 0):

```bash
# offline invariants
./scripts/online_join_check.py --selftest

# real join + full UDP handshake (reads .ROBLOSECURITY from the env)
ROBLOSECURITY="<cookie>" ./scripts/online_join_check.py --place-id 606849621
# -> "wire: *** steady key CONFIRMED" and exit code 0

# replay a captured/known session instead of a fresh join
./scripts/online_join_check.py --server <udmux-ip:port> --rcc-ip <10.x.y.z> \
    --token <32 hex> --seed1 <64 hex> --pa-blob <66 hex> --auth-ver 18
```

Rust (the real client):

```bash
cargo test                       # 82 lib + 17 integration tests
RUSTBLOX_COOKIE="<cookie>" RUSTBLOX_DEBUG=net \
    cargo run --example join_flow -- 606849621
# -> "crypto OpenReply2: seed2 …" then "steady datagram 0x81 … session accepted"
#    then "CONNECTED (server guid …)"
```

Invariants that must not regress:

1. OCR1 and OCR2 are both 1160-byte datagrams (OCR2 = 242-byte frame + zeros).
2. seed1 = `EphemeralEarlyPubKey`, and the *first* OCR2 of a session is the
   only one the RCC considers — never send a probe OCR2 with another key.
3. The RUPP envelope token type is 0x01 and the mux field carries the private
   RCC `ip:port`; a wrong address or token draws total silence.
4. Post-OpenReply2 is AES-256-GCM with the `UniqueNumbeR`+counter nonce; the
   ChaCha box is only for OCR2 and ORP2.

Artifacts (not in the repo): hooks, pcaps, analysis and the capture logs live
under `/storage/rblx_debug/session-crypto/`.

## Connected-phase RakNet system messages — solved (the "silent server" bug)

After the steady `CONNECTION_REQUEST` the server does **not** start
replicating: it walks the client through the connected RakNet handshake, and
every message of it rides as an ordinary frame payload inside the steady
datagram stream (byte-mapped from `raknet29.pcap`; `analysis/gamejoin_frames.py`
prints the exact offsets):

```text
id  layout                                                         size
0x10 ACCEPTED      peer addr | sysIndex u16 | 10 addr slots |
                   request_ms u64 | server_ms u64 | server_us u64     104
0x13 NEW_INCOMING  peer addr | 10 addr slots | server_ms u64 |
                   client_ms u64 | client_us u64                      102
0x00 ping          client_ms u64                                 9 / 1160
0x03 pong          peer_ms u64 | own_ms u64 | own_us u64          25 / ping
0x51..0x53 sync    own_ms u64                                          9
0x54..0x56 ack     peer_ms u64 | own_ms u64 | own_us u64              25
0x15/0x16          disconnect / connection-lost (framed, not bare)     5
```

Addresses are the fork's XOR block (`family 4 | ip^0xFF | port BE`); the
trailing 8-byte value is a **microsecond** monotonic clock, while the echoed
fields are milliseconds (big-endian). Ids below `0x80` are system, everything
with bit 7 set is application data — a clean discriminator.

Four client-side bugs kept the session mute, all fixed:

1. **The framed ACCEPTED was discarded** (`connect` broke on the first
   steady datagram). It is now ingested, which queues
   `ID_NEW_INCOMING_CONNECTION` (message 1, order 0), an unpadded ping, an
   MTU-sized padded ping (split, as in the capture) and the three clock-sync
   announcements.
2. **ACKs were sent in the clear** while the steady layer is encrypted. The
   server ignored them and retransmitted ACCEPTED forever; every control /
   ACK / pong datagram now goes through the counter box.
3. **ACK ranges were matched against reliable *message* numbers** instead of
   *datagram* sequence numbers, so the resend queue never drained (it only
   "worked" while both counters happened to coincide at session start). The
   flush now records `datagram seq -> [message numbers]`.
4. The steady `CONNECTION_REQUEST` is datagram 0 / message 0; the next frame
   must continue at 1 or the server drops it as a duplicate.

With these, the server stops retransmitting and sends its first application
messages (the policy/flag table `93 00`, then the join challenge `9b`).

## Replication payload layer — zstd over bit-packed plaintext

The earlier note ("inflates to high-entropy bytes, a further encoding layer")
is **wrong**. `zstd -d` on the frames of the v29 capture gives plaintext:

| head | inflated size | content |
|---|---|---|
| `97 0d` | 330 985 | the **schema dictionary**: every class and property name (`AccessModifierType`, `AccessoryType`, `ActionType`…) |
| `83 0d` | 52 326 | world clusters: `rbxassetid://…`, part names (`SewerPart`, `ShippingContainer4`) |
| `83 16` | ~20 K | cluster chunks (bit-packed numbers/refs) |
| `8d 01` | 2 650 | streaming update frames (bit-packed) |

Header shape (`97 0d` is the clean byte-aligned example):
`id | subtype | u32 LE compressed-len | u32 LE inflated-len | zstd frame`.
The other subtypes insert 0–5 extra bytes before the lengths, so the parser
locates the `28 b5 2f fd` magic in the first 24 bytes instead of hard-coding
one layout (`src/roblox/replication.rs`, `ruzstd`). Strings inside the bodies
use two conventions — `u8 len + ASCII` (schema/cluster) and `u16 LE len +
ASCII` (the policy message) — both harvested into `DataModel::strings`.

Decoding the bit-packed cluster schema (class descriptors + property ids from
`97 0d`) into real parts is the next step; the container format is no longer
in the way.

## The remaining blocker: the client-side game join

Replaying the capture's client-side join payloads (`a7`, `90`, `92`, `8a`,
`8f`) from a live session makes the server answer with its `9b` challenge
(2053 bytes, header `9b | A4 | B4 | u32 len | data`), and the real client
answers `9b | B4 | 4 unknown bytes` — but our replay then draws the framed
`0x15` disconnect. Those payloads are **opaque**:

* entropy 7.97–7.99, no zstd / zlib / lz4 / brotli frame at any offset,
* not openable with the transport keys (SESSDER#1/#2, ChaCha `UniqueNumber`,
  AES-GCM `UniqueNumbeR` + counter) under any header/AAD offset tried
  (`analysis/applayer_try.py`),
* the 4-byte tail `23 d5 5c 38` of the capture's `9b` reply matches no hash
  (CRC32/Adler/FNV/DJB2/murmur, md5/sha1/sha256/blake2 prefixes) of the
  challenge, its data, the headers or the session id.

So the client→server application layer carries its own cipher/key that is not
derived from the transport handshake material we hold. Everything else on the
path is proven: the server's `93` policy table arrives live and is decoded
(57 names from the flag list), and the transport handshake is answered.

### What this costs in the app

The desktop client joins, shows `state: InGame`, receives the policy message
and then only keepalives — the server waits for the (encrypted) game-join
upload. The renderer therefore draws the fallback place (sky, baseplate grid,
spawn pad, local R6 avatar) and the local character walks/jumps against the
decoded world's colliders as soon as replication fills them.

## Visible proof (24 Nov) — the client renders a real, live session

`docs/rustblox-live-ingame.png` is a capture of the **running client** joined
to a real server (place «Побег из тюрьмы», 606849621: `state: InGame`, real
server/job, 75 fps, live ping). Reproduction:

```bash
cargo build
RUSTBLOX_AUTOJOIN=606849621 ./target/debug/rustblox
```

Two fixes made that path work: deferred auto-join (the restored session used
to replace the game screen with the menu) and the wgpu `adapter.features()`
request (the GL/llvmpipe fallback otherwise failed every frame with
"TextureView is invalid").

Screenshot tooling: `session-crypto/scripts/live_shot_real.sh` isolates the
client window in a Hyprland special workspace and refuses to capture unless
the active workspace and the front window are both the client. Note: on the
current Hyprland build the special-workspace toggle did not report
`special:rblx`, so the gate refused — verify by logs instead.

### Controls (desktop)

`WASD` walk (camera-relative, 16 studs/s), `Space` jump, mouse look with the
cursor captured on LMB, `Esc` releases the mouse and a second `Esc` leaves,
`F` toggles the debug free-fly, `R` respawn. The HUD hint line reflects this.
