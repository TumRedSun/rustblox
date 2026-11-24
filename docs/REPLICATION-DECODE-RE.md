# Replication decode — verified containers, and what is still not decoded

Status 24 Nov 2026. This document separates **what was byte-verified in this
session** from the parts that are still open. The existing
`src/roblox/replication/body.rs` heuristics ("len+kind") remain
**unverified**; they are not presented as truth here.

## 1. CONFIRMED — transport framing

For every live datagram, the plaintext is:

```
u8 0x80 | flags<<4 | count      (count = low nibble, 1..=8+)
u24 LE datagram sequence
per frame:
  u8  flags            (reliability = flags>>5, split = flags&0x10)
  u16 BE bit_length    (payload size = ceil(bit_length/8))
  u24 LE message       (reliability 2,3,4,6,7)
  u24 LE ordering      + u8 channel (reliability 1,3,4,7)
  if split: u32 BE fragment_count | u16 BE split_id | u32 BE fragment_index
  payload
```

Verified by parsing all 13 866 datagrams of the live `hook_open2.log`
(12 356 server→client, 1 510 client→server) and every 1 160/1 111-byte
padded datagram. The repo's `envelope.rs` implements this; the live counts
above are the reference.

One correction to the old notes: the `0x8d 0x01` prefix is a **LEB128 varint**
whose value is `0x8d` (bytes `8d 01`), not an `id, type` pair. Other heads
(`83 16`, `83 0d`, `97 0d`) are single-byte ids followed by a subtype/flag.

## 2. CONFIRMED — big bodies are ordinary zstd frames (with a length header)

The sticky point in earlier sessions was that `zstd -d` rejected some bodies.
Cause found: the application message contains a **length header followed by an
exact zstd frame and 1–2 trailing bytes**. With `ZSTD_findFrameCompressedSize`
the frame is complete; `ruzstd`'s `StreamingDecoder` stops at the frame end, so
the parser works (a regression test was added:
`zstd_frame_ignores_trailing_bytes`).

For **all 448 `83 16` and all 178 `83 0d` bodies of the live session**, the two
little-endian u32 values immediately before the `28 b5 2f fd` magic are exactly:

```
[compressed size of the zstd frame] [uncompressed size]
```

| head | header before the magic | example (hook_open2) |
|---|---|---|
| `83 16` | `id(2) 01 u32 compressed u32 uncompressed` | `83 16 01 48 66 00 00 3e 66 00 00` → 26 184 → 26 174 |
| `83 0d` | `id(2) 70 00 00 00 u32 compressed u32 uncompressed` | `83 0d 70 00 00 00 56 0b 00 00 3c 1f 00 00` → 2 902 → 7 996 |
| `8d 01` | `8d 01 b1 7e 01 00 u8 enc u32 cs u32 us` (magic at 15) | `8d 01 b1 7e 01 00 03 12 02 00 00 df 03 00 00` → cs 530, us 991 |

The `8d01` line was checked on every one of the 2 999 live `8d01` messages:
magic at offset 15, `cs == len - 15` (2 999/2 999), the zstd frame size equals
`cs` (2 999/2 999), and `us` equals the inflated length (2 999/2 999). The
meaning of the `enc` byte and of the `b1 7e 01 00` constant is **not** known.

`8316`: 448/448 matches; `830d`: 178/178 matches. The field at `[2..6]` for
`830d` is not the compressed size (it was `0x70` here); only the two u32s before
the magic are proven. For `8d 01` only the **last** u32 before the magic is
`us`; `cs` sits two u32s earlier (magic at 15, not at 11/14). The `97 0d`
header was not mapped; do not apply the `83` rule to other heads blindly.

Live inflations (checked with libzstd, and by writing the exact frame):

| sample | message | inflated | content |
|---|---|---|---|
| `corpus/8d01_0001.bin` | 545 B | 991 B | streaming update, bit-packed |
| `corpus/8316_0002.bin` | 26 196 B | 26 174 B | cluster chunk, high-entropy bit-packed |
| `corpus/830d_0001.bin` | 2 918 B | 7 996 B | world cluster; contains ASCII GUIDs (e.g. `65a1d8c7-5394-4f87-95e3-115da66e177b`) and tags |
| `97 0d` | 36 630 B | **330 985 B** | the schema dictionary (same size as in the old v29 capture) |

The full live corpus after join is inflated and stored at
`/tmp/opencode/relre/corpus/`:
`8d01` × 2 999, `8316` × 448, `830d` × 178 (26 MB total). This is the
verification corpus for the bit decoder.

## 3. CONFIRMED — the client join lists (new decoder in the repo)

The client→server join messages `0xa7/0x90/0x8a` are **not replication bodies
and not encrypted**: they are counted arrays of 16-byte items. This was added
as `src/roblox/replication/join_items.rs` with unit tests; `envelope.rs` now
emits `RepEvent::JoinItemList` instead of feeding those bytes to the name
harvester. See `docs/GAME-JOIN-RE.md` §2 for the byte evidence (four session
captures; `a7` mostly persistent, `90/8a` per-session).

This change is deliberately limited to the **container** that was proven; it
does not claim item semantics.

## 4. PARTIALLY DECODED — `8d 01` outer records; bulk blob still open

This is no longer a black box, but it is **not fully decoded**. Below, the
addresses in the first subsection were read in the static pass of this session;
the data-level counts were independently re-checked by me on the live corpus.

### 4.1 Handler map (static; addresses == file offsets)

| address | role |
|---|---|
| `0x6237bab` | `ClientReplicator` first-level dispatcher |
| `0xb2bda8` | relative int32 jump table, ids `0x81..0xa6`; target = table + entry |
| `0x83` → `0x62381e1`, `0x8d` → `0x6237de9`, `0x97` → `0x6237ff4` | real cases |
| `0x9b`, `0x93`, `0x85`, `0x8a`, … → `0x6238ffd` | common no-op (`mov $1,%r13d` + epilogue) |
| `0x625f1de` | `Replicator::deserializeStreamAsync` (log site `0x6260a48`) |
| `0x626fec2` | `8d` case |
| `0x626f53a` | the `8d01` body loop; `0x626f5ac` reads the first `u8` (tag) |

I re-decoded the table myself: 0x8d entry `91275329` → `0x6237de9`, 0x83 entry
`91276345` → `0x62381e1`, 0x9b entry → `0x6238ffd` (no-op). `0x83 16` does not
have a static handler address: that path goes through the replicator vtable
(`+0x520`/`+0x518`) and is not yet identified.

### 4.2 Proven inner header of `8d01` bodies

The static loop at `0x626f53a` reads the first byte and splits it as
`tag & 0x0f`, `(tag >> 4) & 3`, `(tag >> 6) & 3`; the layout reported by that
pass is:

```
u8 tag        bits0-3 = n, bits4-5 = int width (0 => 8-bit),
              bits6-7 = hi2
3 ints        (width selected by tag bits4-5)
u8 flag       (0 or 1)
... bulk      (not decoded)
```

My independent corpus checks over all 2 999 `8d01` bodies agree with the
header-level claims:

* `tag & 0x30 == 0` in **2 999/2 999** bodies (so the three ints are 8-bit);
* `flag` is `0` in 2 495 and `1` in **504**, i.e. `flag ∈ {0,1}` in
  **2 999/2 999**;
* 460 of the 504 `flag == 1` bodies are exactly **6 bytes** long; the rest are
  longer (bulk data).

`n` (tag low nibble) is 4 in 2 399 bodies and 1 in 600 bodies; its meaning is
**not** established.

### 4.3 What is still NOT decoded

* The **bulk blob after `flag == 0`** (and the long `flag == 1` cases). It is
  not zstd: the `zstd -d` path rejects it at every offset. The static pass
  points at consumers `0x5d2d8d8` / `0x1ccffee`, but neither is identified.
* The meaning of `n`, `hi2`, `enc` (`8d01` header) and of the `83 0d` `u32 A`
  field / `0x0c` tail.
* The `0x83 16` vtable target and its per-field layout.

### 4.4 Other established facts (negative results)

* The inflated bodies are **plaintext**, not a second cipher: `8316` starts
  `26 f2 15 42 84 62 19 be …`, `830d` contains readable GUIDs/tags.
* The old build's bit reader (`0x40ad67e`, `+0xc` total bits, `+0x14` position,
  `+0x18` buffer) was **not found at that offset** in 2.748.1527 by the static
  pass. The stream primitives it uses in this build are the
  `0x379f5b2/0x379f67c/0x379f1a4/0x379f410` family (u8/u16/u32/u64),
  `0x379e526` (advance), `0x379ea58` (LEB128) and `0x637d298` (tagged
  8/16/32-bit reads).
* Magicless zstd, zlib/raw-deflate, lzma, bz2 and lz4 do not decode the bodies
  (checked at every offset); only the zstd magic path does.
* The repository's existing `body.rs` cluster parser (`u32 len | u32 kind`)
  was **not** observed in the live inflated bodies and remains unverified.

Next decisive step: identify the bulk-blob consumer (`0x5d2d8d8` /
`0x1ccffee`) or the `83 16` vtable target, and design the bit reader against
the 26 MB corpus. A correct outer decoder must consume whole `8d01` bodies
without desynchronising on at least two independent samples.

## 5. Reproduce the corpus and the parser

Artifacts (all outside the repo):

```
/storage/rblx_debug/session-crypto/logs/hook_open2.log   live app session
/storage/rblx_debug/session-crypto/logs/hook_open3.log   second live session
/tmp/opencode/relre/hook_open2.js / hook_open3.js        Frida hooks
/tmp/opencode/relre/parse_log.py                         datagram walker + split reassembly
/tmp/opencode/relre/corpus/                              3625 inflated bodies:
  8d01_0001..2999.bin, 8316_0001..448.bin, 830d_0001..178.bin
/tmp/opencode/relre/schema_dict.bin                      the live 97 0d inflated (330985 B)
/tmp/opencode/relre/agent-bitpack.md                     the static 8d01 analysis report
/tmp/opencode/relre/bitpack_verify.py                    runnable header/inner verifier
```

Inflate a body exactly (libzstd; the application message may carry 1–2 trailing
bytes):

```python
import ctypes, ctypes.util
z = ctypes.CDLL(ctypes.util.find_library("zstd"))
z.ZSTD_findFrameCompressedSize.restype = ctypes.c_size_t
z.ZSTD_getFrameContentSize.restype = ctypes.c_ulonglong
z.ZSTD_decompress.restype = ctypes.c_size_t
# frame = msg[15:] for 8d01; msg[11:] for 83 16; msg[14:] for 83 0d
fs = z.ZSTD_findFrameCompressedSize(buf, len(frame))
cs = z.ZSTD_getFrameContentSize(buf, fs)
out = ctypes.create_string_buffer(cs)
z.ZSTD_decompress(out, cs, buf, fs)   # exact frame, trailing bytes ignored
```

Reassemble the live session in both directions:

```bash
python3 /tmp/opencode/relre/parse_log.py \
  /storage/rblx_debug/session-crypto/logs/hook_open2.log
```

The live `8d01` header was checked by `bitpack_verify.py` against all 2 999
messages; the corpus files were produced by the same script (hash-checked
against their raw messages).

## 6. Test status / repo changes

* `cargo fmt --check` — clean.
* `cargo build` — ok.
* `CARGO_INCREMENTAL=0 cargo test -j1` — **98 lib + 17 integration, 0 failed**.
  (A plain parallel `cargo test` can hit a rustc 1.100 ICE while linking
  `tests/parse.rs`; it is a compiler bug, not a test failure.)
* New decoder: `src/roblox/replication/join_items.rs`; event
  `RepEvent::JoinItemList`; routing in `envelope.rs` before the zstd scan.
* `src/roblox/replication/body.rs` was **not** changed by this work; its
  `u32 len | u32 kind` cluster model remains unverified against the live bodies.
* Nothing was committed.
