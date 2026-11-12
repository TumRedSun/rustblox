#!/usr/bin/env python3
"""Live UDMUX crypto-handshake probe (Python twin of src/roblox/early.rs).

Replicates the Nov-12 working-capture chain against a live join and reports
exactly which stage answers, so the Rust client's handshake can be iterated
without a full client build. Read-only apart from UDP datagrams it sends.

Stages: auth ticket -> join -> OCR1 (RUPP `01 11 01`) -> REPLY_1 -> crypto
OCR2 (`78 | magic | 03 5a 00 5d 00 05 | pub | 11 21 | preauth | box(93) |
"UniqueNumber" | 16`) -> OpenReply2 (`7d | magic | 01 2c 00 3a 03 | pub |
box(53) | "UniqueNumber" | 16`) -> boxed CONNECTION_REQUEST (52 = 24 + 28).

Usage:
  python3 tools/udmux_probe.py [placeId] [--ocr2-payload HEX] [--preauth HEX]
                              [--aad env|none] [--tagged|--untagged]

Env overrides mirror the client: RUSTBLOX_PREAUTH (hex 33B),
RUSTBLOX_OCR2_PAYLOAD (hex, padded to 65B). The .cookie file is read from
/tmp/opencode/.cookie (the throwaway test account).
"""
import argparse
import base64
import hashlib
import json
import os
import socket
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

MAGIC = bytes.fromhex('00 ff ff 00 fe fe fe fe fd fd fd fd 12 34 56 78')
CAPTURED_PREAUTH = bytes.fromhex(
    '38bcf50273c87e2940184b68ff8d874506b3f310027167d1ef43ef3bea78609fa4')
RBX_PASSWORD = bytes.fromhex('374f5e116c45')
OCR1_ID, REPLY1_ID = 0x7b, 0x7e
OCR2_ID, OPEN_REPLY2_ID = 0x78, 0x7d
CONNECTION_REQUEST_ID = 0x09


def http(url, body=None, headers=None, cookie=None, csrf=None):
    h = {'User-Agent': 'Roblox/WinInet', 'Content-Type': 'application/json'}
    if headers:
        h.update(headers)
    if cookie:
        h['Cookie'] = '.ROBLOSECURITY=' + cookie
    if csrf:
        h['X-CSRF-Token'] = csrf
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers=h,
                                 method='POST' if data else 'GET')
    try:
        r = urllib.request.urlopen(req, timeout=15)
        return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def join(place, cookie):
    s, h, b = http('https://auth.roblox.com/v1/authentication-ticket', body={},
                   headers={'Referer': f'https://www.roblox.com/games/{place}/',
                            'Origin': 'https://www.roblox.com'}, cookie=cookie)
    if s == 403:
        s, h, b = http('https://auth.roblox.com/v1/authentication-ticket', body={},
                       headers={'Referer': f'https://www.roblox.com/games/{place}/',
                                'Origin': 'https://www.roblox.com'},
                       cookie=cookie, csrf=h.get('x-csrf-token'))
    ticket = h.get('rbx-authentication-ticket', '')
    print(f'ticket status {s} len {len(ticket)}')
    s, h, b = http('https://gamejoin.roblox.com/v1/join-game',
                   body={'placeId': place, 'isTeleport': False,
                         'isImmersiveAdsTeleport': False,
                         'gameJoinAttemptId': '11111111-2222-4333-8444-555555555555',
                         'authenticationTicket': ticket},
                   headers={'Referer': f'https://www.roblox.com/games/{place}/',
                            'Origin': 'https://www.roblox.com'},
                   cookie=cookie, csrf=h.get('x-csrf-token'))
    v = json.loads(b)
    if not v.get('jobId'):
        print('JOIN REFUSED:', b[:300].decode(errors='replace'))
        sys.exit(1)
    js = v['joinScript']
    eepk = base64.b64decode(urllib.parse.unquote(js['EphemeralEarlyPubKey']))
    token = base64.b64decode(js['TokenValue'] + '==')
    proxy = (js['UdmuxEndpoints'][0]['Address'], int(js['UdmuxEndpoints'][0]['Port']))
    rcc = (js['MachineAddress'], int(js.get('ServerPort', proxy[1])))
    print(f"job {v['jobId'][:8]} rcc {rcc[0]}:{rcc[1]} proxy {proxy[0]}:{proxy[1]}")
    print(f"eepk {eepk.hex()} token {token.hex()} pepper {js['PepperId']}")
    return dict(rcc=rcc, proxy=proxy, eepk=eepk, token=token,
                pepper=js['PepperId'])


def crypto_kx(sk: X25519PrivateKey, peer_pk: bytes, client_pk: bytes):
    """libsodium crypto_kx client schedule: BLAKE2b-512(shared||pk||peer)."""
    shared = sk.exchange(X25519PublicKey.from_public_bytes(peer_pk))
    h = hashlib.blake2b(shared + client_pk + peer_pk, digest_size=64).digest()
    return h[32:], h[:32]  # (tx c2s, rx s2c)


def nonce_bytes(counter: int) -> bytes:
    """Post-OpenReply2 nonce: u64 LE counter || 'mbeR' (ref/raknet-decryption,
    confirmed by the dump+pcap sequential counter hints)."""
    return struct.pack('<Q', counter) + b'mbeR'


def seal_counter(key: bytes, counter: int, pt: bytes) -> bytes:
    """Post-OR2 wire: ct || hint(u16 LE = counter & 0xffff) || tag(16)."""
    box = ChaCha20Poly1305(key).encrypt(nonce_bytes(counter), pt, b'')
    ct, tag = box[:-16], box[-16:]
    return ct + struct.pack('<H', counter & 0xFFFF) + tag


def open_counter(key: bytes, wire: bytes, base: int = 0, window: int = 8):
    if len(wire) < 18:
        return None
    ct, rest = wire[:-18], wire[-18:]
    hint = struct.unpack('<H', rest[:2])[0]
    box = ct + rest[2:]
    diff = (hint - (base & 0xFFFF)) & 0xFFFF
    for k in range(window + 1):
        val = (base + diff + k * 0x10000) & 0xFFFFFFFFFFFFFFFF
        try:
            return ChaCha20Poly1305(key).decrypt(nonce_bytes(val), box, b''), val
        except Exception:
            continue
    return None


def rupp_header(state, tag, rcc):
    hdr = bytes([1, 0, 0, 0x1f, 1, 0x11, state]) + tag
    hdr += bytes([2, 6]) + socket.inet_aton(rcc[0]) + struct.pack('>H', rcc[1])
    return hdr


def strip_rupp(d):
    if len(d) >= 7 and d[0] == 1 and d[1] == 0 and d[2] == 0 and d[3] in (4, 0x17, 0x1f):
        return d[d[3]:], d[3]
    return d, 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('place', nargs='?', type=int, default=537413528)
    ap.add_argument('--ocr2-payload', default=os.environ.get('RUSTBLOX_OCR2_PAYLOAD', ''))
    ap.add_argument('--preauth', default=os.environ.get('RUSTBLOX_PREAUTH', ''))
    ap.add_argument('--aad', choices=['none', 'env'], default='none')
    ap.add_argument('--state2', action='store_true',
                    help='send OCR2 with class byte 0x02 instead of 0x01')
    args = ap.parse_args()

    cookie = open('/tmp/opencode/.cookie').read().strip()
    j = join(args.place, cookie)

    sk = X25519PrivateKey.generate()
    pub = sk.public_key().public_bytes(serialization.Encoding.Raw,
                                       serialization.PublicFormat.Raw)
    tx, rx = crypto_kx(sk, j['eepk'], pub)
    print(f'our pub {pub.hex()}\ntx {tx.hex()}\nrx {rx.hex()}')

    preauth = bytes.fromhex(args.preauth) if args.preauth else CAPTURED_PREAUTH
    pt65 = (bytes.fromhex(args.ocr2_payload) if args.ocr2_payload else b'')[:65]
    pt65 = pt65 + b'\x00' * (65 - len(pt65))

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(1.2)
    dest = j['proxy']

    def send(hdr_state, tag, body):
        pkt = rupp_header(hdr_state, tag, j['rcc']) + body
        sock.sendto(pkt, dest)
        return pkt

    def recv():
        try:
            d, a = sock.recvfrom(65535)
            return d
        except socket.timeout:
            return None

    # --- OCR1 (exactly the capture: 7b magic 05 01, padded to 1160) ---
    tag = j['token']  # the capture answers OCR1 with any 16B tag; the
                      # session TokenValue is the documented candidate.
    body = bytes([OCR1_ID]) + MAGIC + bytes([0x05, 0x01])
    body += b'\x00' * (1160 - 31 - len(body))
    r = None
    for tries in range(4):
        send(1, tag, body)
        r = recv()
        if r is not None:
            break
    if r is None:
        print('OCR1: NO REPLY — proxy/RCC never answered, abort')
        return
    inner, hlen = strip_rupp(r)
    print(f'REPLY_1 ({len(r)}B, rupp hlen {hlen}): {inner.hex()}')
    if inner[0] != REPLY1_ID:
        print('unexpected OCR1 reply id — continuing anyway')

    # --- crypto OCR2 (capture shape) ---
    aad = bytearray([OCR2_ID]) + MAGIC + bytes([0x03, 0x5a, 0x00, 0x5d, 0x00, 0x05])
    aad += pub + bytes([0x11, len(preauth)]) + preauth
    assert len(aad) == 90
    nonce = os.urandom(12)
    box = nonce + ChaCha20Poly1305(tx).encrypt(nonce, pt65, bytes(aad))
    assert len(box) == 93
    unique = os.urandom(16)
    body2 = bytes(aad) + box + b'UniqueNumber' + unique
    body2 += b'\x00' * (1160 - 31 - len(body2))
    state = 2 if args.state2 else 1
    pkt = send(state, tag, body2)
    print(f'OCR2 sent ({len(pkt)}B, state {state}): aad={bytes(aad).hex()[:40]}.. box={box.hex()[:24]}..')
    r = recv()
    if r is None:
        print('OCR2: NO REPLY — the preauth blob or box content likely differs '
              '(try --ocr2-payload / --preauth / --state2)')
        return
    inner, hlen = strip_rupp(r)
    print(f'OPEN_REPLY2 ({len(r)}B, rupp hlen {hlen}): {inner.hex()}')
    if inner[0] != OPEN_REPLY2_ID:
        print('unexpected OCR2 reply id — stopping')
        return
    server_pub = inner[22:54]
    un_at = inner.rfind(b'UniqueNumber')
    server_box = inner[54:un_at] if un_at > 0 else inner[54:]
    server_unique = inner[un_at + 12:un_at + 28] if un_at > 0 else b''
    print(f'reply2 server_pub {server_pub.hex()} box {len(server_box)}B unique {server_unique.hex()}')
    # Adopt the session keys.
    stx, srx = crypto_kx(sk, server_pub, pub)
    print(f'session tx {stx.hex()}\nsession rx {srx.hex()}')
    # The reply2 box is nonce-ON-THE-WIRE (like the OCR2 box): first 12B =
    # random nonce, unlike the sequential counter hints after it.
    opened = False
    for kn, key in (('session-rx', srx), ('early-rx', rx), ('session-tx', stx), ('early-tx', tx)):
        for an, aad_v in (('empty', b''), ('prefix54', inner[:54])):
            try:
                pt = ChaCha20Poly1305(key).decrypt(server_box[:12], server_box[12:], aad_v)
                print(f'REPLY2 BOX OPENED (nonce-on-wire) with {kn}/aad={an}: {pt.hex()}')
                opened = True
                break
            except Exception:
                pass
        if opened:
            break
    if not opened:
        # Also try the counter framing in case the phase split is wrong.
        for kn, key in (('session-rx', srx), ('early-rx', rx)):
            hit = open_counter(key, server_box)
            if hit:
                print(f'REPLY2 BOX OPENED (counter) with {kn}: pt={hit[0].hex()} counter={hit[1]:#x}')
                opened = True
                break
    if not opened:
        print('reply2 box: no key/framing matched (content stays opaque)')

    # --- boxed CONNECTION_REQUEST (capture pkt4: 52 = ct34 + hint2 + tag16) ---
    # The dump's pkt4 plaintext is 34 bytes; the vanilla CR is 24 — the fork
    # carries 10 more (layout unresolved). Both sizes are tried.
    cr24 = bytes([CONNECTION_REQUEST_ID]) + struct.pack('>Q', 0x1122334455667788)
    cr24 += struct.pack('>Q', int(time.time() * 1000)) + b'\x00' + RBX_PASSWORD
    assert len(cr24) == 24
    cr34 = cr24 + b'\x00' * 10  # variant: the fork's 34-byte plaintext
    counter = 0
    for name, cr in (('24B vanilla', cr24), ('34B fork-size', cr34)):
        for kn, key in (('session-tx', stx), ('early-tx', tx)):
            box3 = seal_counter(key, counter, cr)
            send(state, tag, box3)
            r = recv()
            print(f'boxed CR {name} with {kn} counter {counter} -> '
                  + (f'{len(r)}B: {strip_rupp(r)[0].hex()[:64]}' if r else 'NO REPLY'))
            if r:
                inner, _ = strip_rupp(r)
                op = False
                for okn, okey in (('session-rx', srx), ('early-rx', rx)):
                    hit = open_counter(okey, inner)
                    if hit:
                        print(f'  reply box OPENED with {okn}: pt={hit[0].hex()} counter={hit[1]:#x}')
                        op = True
                        break
                if op:
                    counter += 1


if __name__ == '__main__':
    main()
