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
