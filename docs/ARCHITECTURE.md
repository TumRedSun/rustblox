# Architecture

Living document. Updated as each subsystem is refactored — this is not a
one-time plan, it is the current, enforced set of rules the codebase is
migrated toward, subsystem by subsystem.

## Status

| Subsystem | State |
|---|---|
| Tooling (fmt/clippy/lints/hooks) | Done (this doc, `rustfmt.toml`, `clippy.toml`, `Cargo.toml [lints]`, `.githooks/`) |
| Network: `crypto` (was `roblox::early`) | **Done** — `src/net/crypto/{mod,counter,handshake,keyring}.rs`, tests split out, `#![deny(unwrap/expect/panic)]` locally |
| Network: `udmux` (was `roblox::udmux`) | **Done** — `src/net/udmux/{mod,envelope,token,netstack}.rs`, same lint policy |
| Network: `raknet` (was `src/raknet.rs`) | **Done** — `src/net/raknet/{mod,tests,handshake/{mod,connect,reply,build,tests},wire/{mod,tests,reliability,reliability/tests}}.rs`, same lint policy. `connect.rs` (564 lines), `reply.rs` (354) and `reliability.rs` (487) exceed the 350-line cap — documented exceptions, see below. |
| HTTP/Roblox API (`join`, `client`, `auth`, `dns`, `types`, `errors`) | Not started |
| UI (`app`, `menu_ui`, `ui`, `login_ui`, `draw`, `font`) | Not started |
| Render (`gfx`, `world`, `game`, `model`) | Not started |
| Mobile (`mobile/`) | Not started |

## Module layout rules

- **File length: 350 lines max** (including doc comments and blank lines).
  Chosen over a tighter 250 because every `pub` item now carries a mandatory
  doc comment (see below) — doc comments cost vertical space without adding
  complexity, and a tighter cap would force splitting simple files for no
  reason other than comment overhead. Enforced by a pre-commit check (no
  native clippy lint for "lines per file" exists).
- **4 files per folder max — subfolders do not count against this limit.**
  A folder may hold `mod.rs` + up to 3 siblings, plus as many subfolders as
  needed, each with its own 4-file budget. Split by **responsibility**, not
  by raw size: wire-framing is separate from reliability bookkeeping,
  handshake flow is separate from the connected session, crypto primitives
  are separate from the key-schedule that uses them.
- **Function length: 60 lines max**, enforced by
  `clippy::too_many_lines` (`clippy.toml` → `too-many-lines-threshold = 60`).
- **KISS/DRY/SOLID**: when a module is migrated, actively remove
  now-dead fallback paths and duplicated logic rather than carrying them
  into the new layout unchanged. Example done during tooling setup: a dead
  `Reader::u32` method in `raknet.rs` was deleted rather than kept "just in
  case" (YAGNI) — code that isn't called gets removed, not preserved.

## Tests

Unit tests live in a **separate file**, not inline at the bottom of the
module they test. Pattern:

```rust
// src/net/raknet/handshake.rs
#[cfg(test)]
#[path = "handshake/tests.rs"]
mod tests;
```

This keeps test code physically out of the file being read/edited, while
staying part of the same crate (tests see private items, `cargo test` works
normally — no separate-crate linking restrictions like true integration
tests in `tests/` have). A `tests.rs` sibling (or `tests/` subfolder for a
module split across several files) does **not** count against the parent
folder's 4-file cap, same as any other subfolder.

True black-box integration tests (crate's public API only, e.g.
`tests/parse.rs`) stay in the top-level `tests/` directory as before.

## Comments and docs

- All comments and doc comments: **English**, no exceptions (verified:
  `grep -rP '//.*[а-яА-ЯёЁ]' src/` returns zero hits as of this doc).
- Every `pub` item (function, struct, field, enum, trait) gets a `///` doc
  comment in rustdoc/JavaDoc spirit: a one-line summary, and where relevant
  a `# Errors` section (what makes a `Result` err), a `# Panics` section
  (what makes it panic — should be rare, see lints below), and `# Arguments`
  only when the parameter names alone don't make usage obvious.

## Formatting

- `rustfmt.toml`: `hard_tabs = true`, `newline_style = "Unix"`. Every file
  LF-terminated, indented with tabs.
- `.gitattributes`: `* text eol=lf` — Unix line endings enforced at the git
  layer too (normalizes line endings on checkin regardless of a
  contributor's local `core.autocrlf`).

## Lints

Two layers:

- **`clippy.toml`** — thresholds: `too-many-lines-threshold = 60`,
  `cognitive-complexity-threshold = 15`, `type-complexity-threshold = 200`.
- **`Cargo.toml [lints]`** — hard levels:
  - `[lints.rust]`: `dead_code = "deny"`, `unused = "deny"` — no unused
    anything, no code that nothing calls, enforced at every build (not just
    CI).
  - `[lints.clippy]`: `unwrap_used`, `expect_used`, `panic`,
    `unimplemented`, `todo` — currently `"warn"`, **not yet `"deny"`**.
    Reason: 143 existing `.unwrap()`/`.expect()` calls as of this doc, 103
    of them in the network subsystem about to be rewritten anyway
    (`raknet.rs` 44, `join.rs` 28, `udmux.rs` 18, `early.rs` 13). Flipping to
    `deny` today would fail the build crate-wide for pre-existing debt, not
    new code.
    **Policy going forward**: every module, when migrated to the new
    `src/net/...` layout (or any other subsystem's new layout), must replace
    its `unwrap`/`expect`/`panic` calls with proper `Result` propagation
    (or a documented, deliberate exception) **and** add
    `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`
    at the top of that specific new module, so the module can't regress once
    cleaned. **Correction (crypto migration, Nov 23):** the original wording
    here said `forbid`, but `forbid` cannot be locally overridden even for a
    reviewed, documented exception (e.g. a `ChaCha20Poly1305::encrypt` call
    on a fixed-size buffer that is provably infallible) — `deny` is used
    instead, with `#[allow(clippy::expect_used)]` plus a comment on each
    specific exception. Test code is exempt per-module via
    `#[allow(clippy::unwrap_used, clippy::expect_used)]` on the `tests` mod
    — a panic in a test is a correct, intended failure signal. Once every
    subsystem is migrated and denies these locally, the crate-wide
    `Cargo.toml` level flips from `"warn"` to `"deny"` as the final step.

## Pre-commit hook

`.githooks/pre-commit` runs on every commit (after
`git config core.hooksPath .githooks`, a one-time per-clone setup step —
not automatic, since `.git/hooks` itself isn't tracked by git):

1. `cargo fmt --check` — fails the commit if anything is unformatted.
2. `cargo clippy --all-targets -- -D warnings` — fails the commit on any
   clippy warning, not just the ones already hard-denied in `Cargo.toml`.

## Dependencies

- `cargo audit`: clean as of this doc — zero known vulnerabilities. One
  unmaintained-crate warning (`ttf-parser`, pulled in transitively through
  `ab_glyph`) — not actionable today, worth revisiting if the font stack
  ever changes.
- Duplicate crate versions in the dependency tree (`foldhash`, `hashbrown`,
  `getrandom`, `rustix`, `thiserror`, `webpki-roots`, …) are all transitive,
  pulled in through `wgpu`/`arboard`'s own dependency choices — not
  something this project controls or should try to pin around.

## Target layout — network subsystem

**Naming collision found during the crypto migration**: `src/net.rs`
already existed as the UI's background worker thread (login/API/avatar
commands, nothing to do with RakNet transport). Renamed to `src/worker.rs`
(2 call sites: `lib.rs`, `app.rs`) to free `src/net/` for the transport tree
below, per KISS — the worker's old name never described what it did either.

```
src/net/
  mod.rs                 — subsystem doc, re-exports (DONE)
  crypto/                — DONE
    mod.rs                — EarlyCrypto struct, seal/open, crypto_kx, consts
    handshake.rs          — build_ocr2/parse_reply2/open_handshake_box, Reply2Fields
    counter.rs            — seal_counter/open_counter, nonce framing
    keyring.rs            — ClientPublicKeyData keyring JSON parser
  raknet/                 — DONE
    mod.rs                — message ids, OFFLINE_MAGIC, RakError, RakState,
                            ForkGen, RakClient struct, SplitState, idle/close
    tests.rs              — offline_magic + the two_clients_loopback integration test
    handshake/
      mod.rs              — RBX_* consts, send_raw, strip_incoming,
                            open_handshake_box, parse_reply2_crypto, local_addr_for
      connect.rs          — RakClient::connect (the whole offline+connected
                            handshake state machine — 564 lines, see exception below)
      reply.rs            — offline_roundtrip, probe_reachability, wait_readable,
                            parse_reply1/2, handle_accepted (354 lines)
      build.rs            — build_ocr2, netstack_probe, hex_head
      tests.rs            — rbx_ocr2_layout + the tag-mirroring send_raw tests
    wire/
      mod.rs              — Reader, Frame, put_u24, frame_encoded_len, address codecs
      tests.rs            — Frame + address round-trip tests
      reliability.rs      — ACK/NACK, send window, split reassembly, tick/poll
                            (487 lines, see exception below)
      reliability/tests.rs — ack_encoding
  udmux/                  — DONE
    mod.rs                — proxy table, is_private/proxy_for
    envelope.rs           — Envelope, RUPP frame/parse, envelope_candidates
    token.rs              — b64_decode/percent_decode
    netstack.rs           — NetStackConfig blob parser
```

No separate `socket.rs` was carved out: the `UdpSocket` is one `RakClient`
field used only through `send_raw`/`poll`/the recv loops in `handshake`/
`wire::reliability` — not a reusable abstraction, so a dedicated file would
only add indirection. Noted as a deliberate non-split, not a gap.

Exact per-file line counts are finalized at split time, not fixed here in
advance — if `handshake.rs` alone would exceed 350 lines once extracted, it
gets its own subfolder (e.g. `handshake/{mod,ocr,connection_request}.rs`)
rather than a size exception. The crypto split needed exactly this once:
`build_ocr2`/`parse_reply2`/`open_handshake_box` moved out of `mod.rs` into
`handshake.rs` (a second `impl EarlyCrypto` block in a different file —
legal in Rust, and exactly how `counter.rs` and `handshake.rs` each add
their own `impl EarlyCrypto`) once `mod.rs` alone passed 350 lines.

**Known leftover from the `udmux` migration**: `envelope::rupp_parse`
still returns a 4-tuple (`(u8, [u8; 16], [u8; 8], usize)`), which trips
clippy's `type_complexity` style note (not a hard error). Its call sites
in `net::raknet::handshake::reply` and `net::raknet::wire::reliability`
destructure it positionally; turning it into a named struct is still open,
just no longer blocked on a pending migration — it is pure follow-up
cleanup now that both sides are in the same subsystem.

**Line-budget exceptions from the `raknet` migration** (all pre-existing
logic, moved verbatim — not new complexity introduced by the split):
- `handshake/connect.rs` (564 lines): `RakClient::connect` is one
  sequential state machine (envelope pinning -> OCR1/REPLY_1 ->
  OCR2/REPLY_2 -> CONNECTION_REQUEST/ACCEPTED) where each phase's output
  feeds the next; splitting it into helper methods would scatter the
  actual control flow across files without reducing real complexity.
- `handshake/reply.rs` (354 lines) and `wire/reliability.rs` (487 lines):
  both are a few lines over, holding a tightly-related group of methods
  (the offline request/reply loop; the full send/resend/ACK/reassembly
  path) that splitting further would fragment into oddly-small pieces for
  little clarity gain. Revisit if either grows further.

Two methods also picked up `pub(crate)` or `pub(in crate::net::raknet)`
visibility (`send_raw`, `strip_incoming`, `RakClient::out_queue`) solely so
sibling modules (`handshake`, `wire::reliability`) could call/reach them —
narrower than `pub` but wider than Rust's default private-to-the-defining-
module rule requires for a single split-impl file (which only needs
visibility to descendant modules, the technique `net::crypto` uses). This
is the one place the `raknet` split needed slightly broader visibility than
`net::crypto` did, because `handshake` and `wire::reliability` are siblings
calling into each other rather than one file's children.
