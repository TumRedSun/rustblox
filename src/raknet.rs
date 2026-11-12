//! RakNet client transport — the reliable-UDP protocol Roblox game servers
//! speak underneath their replication stream.
//!
//! Implements the client side of the standard RakNet wire protocol:
//! offline handshake (OPEN_CONNECTION_REQUEST/REPLY 1-2), connected
//! handshake (CONNECTION_REQUEST / ACCEPTED / NEW_INCOMING_CONNECTION),
//! reliability (reliable + reliable-ordered frames), split-frame
//! reassembly, ACK bookkeeping, resend queue and connected ping/pong.
//!
//! The application payload stream that rides on top of RakNet is Roblox's
//! replication protocol, parsed in `crate::roblox::replication`.

use crate::netlog;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Message IDs (RakNet MessageIdentifiers.h)
// ---------------------------------------------------------------------------

pub const ID_CONNECTED_PING: u8 = 0x00;
pub const ID_CONNECTED_PONG: u8 = 0x03;
pub const ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
pub const ID_OPEN_CONNECTION_REPLY_1: u8 = 0x06;
pub const ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
pub const ID_OPEN_CONNECTION_REPLY_2: u8 = 0x08;
pub const ID_CONNECTION_REQUEST: u8 = 0x09;
pub const ID_CONNECTION_REQUEST_ACCEPTED: u8 = 0x10;
pub const ID_CONNECTION_ATTEMPT_FAILED: u8 = 0x11;
pub const ID_ALREADY_CONNECTED: u8 = 0x12;
pub const ID_NEW_INCOMING_CONNECTION: u8 = 0x13;
pub const ID_NO_FREE_INCOMING_CONNECTIONS: u8 = 0x14;
pub const ID_DISCONNECTION_NOTIFICATION: u8 = 0x15;
pub const ID_CONNECTION_LOST: u8 = 0x16;
pub const ID_CONNECTION_BANNED: u8 = 0x17;

/// RakNet offline message data id (16 bytes) shared by all offline packets.
///
/// This is the canonical OFFLINE_MESSAGE_DATA_ID from RakNet's
/// MessageIdentifiers.h, and Roblox's fork keeps it byte-for-byte (confirmed
/// against the roblox-dissector `OfflineMessageID`). An earlier build shipped
/// a guessed tail (`fe` × 12) — servers silently drop handshake packets whose
/// magic doesn't match, which was exactly the "timed out during offline
/// handshake" failure.
pub const OFFLINE_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe,
    0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

// ---------------------------------------------------------------------------
// Roblox fork offline ids — the proxy answers THESE, not vanilla 0x05/0x06.
// ---------------------------------------------------------------------------
//
// The root cause of the total udmux silence: builds ce2ad87..3f5bb64 ran the
// VANILLA offline handshake (OCR1 = 0x05, OCR2 = 0x07), while Roblox's RakNet
// fork remapped the offline ids. Three independent dissectors of live client
// traffic agree on the mapping — Sala's roblox-dissector PacketReader map
// (0x7B -> OCR1, 0x7E -> REPLY_1, 0x78 -> OCR2, 0x7D -> REPLY_2), raknet-dec's
// PREAUTH_HEADERS = {0x78, 0x7B, 0x7D, 0x7E} and the Anaminus dissector's
// id table. The body keeps the vanilla layout (16-byte offline magic right
// after the id byte); servers/proxies silently drop datagrams whose first
// byte is not one of the fork ids — exactly the observed
// "1400/1377/1369 bytes on the wire, zero replies".

pub const RBX_ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x7B;
pub const RBX_ID_OPEN_CONNECTION_REPLY_1: u8 = 0x7E;
pub const RBX_ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x78;
pub const RBX_ID_OPEN_CONNECTION_REPLY_2: u8 = 0x7D;

/// The fork keeps RakNet protocol version 5 in OCR1 (Sala: "always 5").
pub const RBX_PROTOCOL_VERSION: u8 = 5;

/// Connection password in CONNECTION_REQUEST (0x09) — Sala's
/// `DefaultPasswordBytes`. (Studio sends `{0x5E, 0x11}`; our old packets had
/// no password at all.)
pub const RBX_PASSWORD: [u8; 6] = [0x37, 0x4F, 0x5E, 0x11, 0x6C, 0x45];

/// OCR2 Capabilities (Sala JoinPacketLayers.go): the base Roblox client set
/// (Basic | ServerCopiesPlayerGui3 | IHasMinDistToUnstreamed |
/// ReplicateLuau | VersionedIDSync) and the modern superset that adds
/// PositionBasedStreaming | SystemAddressIsPeerId | PubKeyExchange |
/// StreamingPrefetch. The exact value the server requires is not published —
/// the phase-2 matrix tries both.
pub const RBX_CAPABILITY_ROBLOX: u64 = 0x2CFE;
pub const RBX_CAPABILITY_MODERN: u64 = 0x01A0_3CFE;

const DATAGRAM_VALID: u8 = 0x80;
const DATAGRAM_IS_ACK: u8 = 0x40;
const DATAGRAM_IS_NAK: u8 = 0x20;

const RESEND_MS: u64 = 350;
const MAX_RESENDS: u32 = 12;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum RakError {
    Network(String),
    TimedOut(&'static str),
    Rejected(&'static str),
    AlreadyConnected,
    Banned,
    Closed,
}

impl std::fmt::Display for RakError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RakError::Network(m) => write!(f, "network: {}", m),
            RakError::TimedOut(s) => write!(f, "timed out during {}", s),
            RakError::Rejected(s) => write!(f, "connection rejected: {}", s),
            RakError::AlreadyConnected => write!(f, "already connected"),
            RakError::Banned => write!(f, "banned"),
            RakError::Closed => write!(f, "connection closed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Bit helpers
// ---------------------------------------------------------------------------

/// Write a 24-bit little-endian value.
fn put_u24(buf: &mut Vec<u8>, v: u32) {
    buf.push((v & 0xff) as u8);
    buf.push(((v >> 8) & 0xff) as u8);
    buf.push(((v >> 16) & 0xff) as u8);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = self.buf.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_le_bytes([v[0], v[1]]))
    }
    fn u16be(&mut self) -> Option<u16> {
        let v = self.buf.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([v[0], v[1]]))
    }
    fn u32be(&mut self) -> Option<u32> {
        let v = self.buf.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn u64be(&mut self) -> Option<u64> {
        let v = self.buf.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(u64::from_be_bytes([
            v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
        ]))
    }
    fn u24(&mut self) -> Option<u32> {
        let v = self.buf.get(self.pos..self.pos + 3)?;
        self.pos += 3;
        Some(v[0] as u32 | (v[1] as u32) << 8 | (v[2] as u32) << 16)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = self.buf.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let v = self.buf.get(self.pos..self.pos + 8)?;
        self.pos += 8;
        Some(u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]]))
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let v = self.buf.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(v)
    }
    fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// A connected-state frame (one inside a datagram).
///
/// Wire layout of the fork (raknet-dec `_decode_and_print`, dissecting live
/// datagrams):
///
/// ```text
/// flags u8     reliability in the TOP bits (flags >> 5), split = bit 4
/// bit_length   u16 BE (= payload bits — the old encoder omitted this
///              entirely, so the server read garbage frame boundaries)
/// msg_number   u24 LE   if reliability in {2,3,4,6,7}
/// seq_index    u24 LE   if reliability in {1,4}
/// order_index  u24 LE + order_channel u8  if reliability in {1,3,4,7}
/// split        count u32 BE, id u16 BE, index u32 BE
/// payload      (bit_length + 7) / 8 bytes
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Reliability kind straight off the wire (0..7): 0 unreliable,
    /// 1 unreliable-sequenced, 2 reliable, 3 reliable-ordered,
    /// 4 reliable-sequenced, 5 unreliable+ack-receipt, 6 reliable+ack-receipt,
    /// 7 reliable-ordered+ack-receipt.
    pub reliability: u8,
    pub has_split: bool,
    /// Message number (u24 LE) — reliability {2,3,4,6,7}; shared with the
    /// sequencing index of kinds {1,4} on the send side.
    pub seq: u32,
    /// Ordering index (u24 LE) + channel (u8) — reliability {1,3,4,7}.
    pub order_index: u32,
    pub order_channel: u8,
    pub split_id: u16,
    pub split_count: u32,
    pub split_index: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn reliable_ordered(payload: Vec<u8>) -> Frame {
        Frame {
            reliability: 3,
            has_split: false,
            seq: 0,
            order_index: 0,
            order_channel: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload,
        }
    }

    pub fn unreliable(payload: Vec<u8>) -> Frame {
        Frame {
            reliability: 0,
            has_split: false,
            seq: 0,
            order_index: 0,
            order_channel: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let flags = (self.reliability << 5) | if self.has_split { 0x10 } else { 0x00 };
        out.push(flags);
        out.extend_from_slice(&((self.payload.len() * 8) as u16).to_be_bytes());
        let reliable_msg = matches!(self.reliability, 2 | 3 | 4 | 6 | 7);
        let sequenced = matches!(self.reliability, 1 | 4);
        let ordered = matches!(self.reliability, 1 | 3 | 4 | 7);
        if reliable_msg {
            put_u24(out, self.seq);
        }
        if sequenced {
            put_u24(out, self.seq); // sequencing index shares the counter
        }
        if ordered {
            put_u24(out, self.order_index);
            out.push(self.order_channel);
        }
        if self.has_split {
            out.extend_from_slice(&self.split_count.to_be_bytes());
            out.extend_from_slice(&self.split_id.to_be_bytes());
            out.extend_from_slice(&self.split_index.to_be_bytes());
        }
        out.extend_from_slice(&self.payload);
    }

    pub fn decode(buf: &[u8]) -> Option<Frame> {
        let mut r = Reader::new(buf);
        let flags = r.u8()?;
        let reliability = flags >> 5;
        let has_split = flags & 0x10 != 0;
        let bit_length = r.u16be()? as usize;
        if bit_length < 8 {
            return None;
        }
        let mut f = Frame {
            reliability,
            has_split,
            seq: 0,
            order_index: 0,
            order_channel: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload: Vec::new(),
        };
        if matches!(reliability, 2 | 3 | 4 | 6 | 7) {
            f.seq = r.u24()?;
        }
        if matches!(reliability, 1 | 4) {
            let _seq_idx = r.u24()?;
        }
        if matches!(reliability, 1 | 3 | 4 | 7) {
            f.order_index = r.u24()?;
            f.order_channel = r.u8()?;
        }
        if has_split {
            f.split_count = r.u32be()?;
            f.split_id = r.u16be()?;
            f.split_index = r.u32be()?;
        }
        // Payload is bit-addressed: round up to whole bytes. Clamp to what
        // actually remains — a truncated tail must not panic the parser.
        let want = (bit_length + 7) / 8;
        let left = r.rest();
        f.payload = left[..want.min(left.len())].to_vec();
        Some(f)
    }
}

// ---------------------------------------------------------------------------
// Resend bookkeeping
// ---------------------------------------------------------------------------

struct OutFrame {
    #[allow(dead_code)]
    reliability_index: u32,
    bytes: Vec<u8>, // encoded frame (header + payload)
    sent: Instant,
    tries: u32,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RakState {
    Connecting,
    Connected,
    Disconnected,
}

/// Which offline-id generation won the handshake: the Roblox fork ids
/// (0x7B/0x7E/0x78/0x7D, big-endian, XOR addresses) or vanilla RakNet
/// (0x05/0x06/0x07/0x08 — kept as fallback for direct dials like TeamTest
/// and local servers). Phase 3 shares its message ids across generations,
/// but the address field order and endianness differ — replies are parsed
/// with the layout of whoever won phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkGen {
    Roblox,
    Vanilla,
}

pub struct RakClient {
    socket: UdpSocket,
    pub server: SocketAddr,
    pub guid: u64,
    pub server_guid: u64,
    pub mtu: u16,
    pub state: RakState,
    pub ping_ms: Option<f32>,
    /// Datagram envelope confirmed by the handshake (the live-capture client
    /// shape `01 00 00 1f 01 11 <cls> <tag> 02 06 <rcc ip:port>`, a legacy
    /// probe candidate, or bare/raw when no envelope is needed). Applied to
    /// every datagram sent after it was confirmed — the proxy binds the
    /// client->RCC mapping to the exact clear-header shape.
    envelope: Option<crate::roblox::udmux::Envelope>,
    /// The 16-byte tag we put in our pre-connect RUPP headers (client's own
    /// choice — the live capture shows it matches no join-script field).
    client_tag: [u8; 16],
    /// The server's 16-byte tag, learned from any incoming RUPP header.
    /// Once the connection is up, both sides send the SAME tag.
    server_tag: Option<[u8; 16]>,
    /// Offline-id generation that answered phase 1.
    fork: ForkGen,

    // Send side
    reliability_index: u32,
    order_index: u32,
    datagram_seq: u32,
    split_id: u16,
    out_queue: HashMap<u32, OutFrame>,
    frame_buf: Vec<u8>, // frames awaiting a datagram flush

    // Receive side
    received_datagrams: HashSet<u32>,
    ack_buffer: Vec<u32>,
    delivered_ordered: u32,
    ordered_hold: HashMap<u32, Frame>,
    splits: HashMap<u16, SplitState>,

    // Assembled application payloads waiting to be returned by poll().
    pending: VecDeque<Vec<u8>>,

    // Ping
    last_ping: Instant,
    ping_time: Option<(u64, Instant)>,

    last_recv: Instant,
    buf: Vec<u8>,
}

struct SplitState {
    count: u32,
    parts: HashMap<u32, Vec<u8>>,
    first_seen: Instant,
}

impl RakClient {
    /// Perform the full offline + connected handshake against a game server.
    ///
    /// `udmux_token` carries the join script's per-session routing material
    /// (`(TokenValue, PepperId)`) when the dial goes through a UDMUX proxy.
    /// `rcc` is the PRIVATE RCC `ip:port` from the join script's
    /// `ServerConnections` — on a live capture (Nov 2026) the client RUPP
    /// header embeds it verbatim (`02 06 <ip4> <port BE16>`) and the proxy
    /// routes on that, so when it is known we skip the shape probing and pin
    /// `udmux::client_envelope` directly; the legacy token-derived probe
    /// matrix stays as the fallback.
    pub fn connect(
        host: &str,
        port: u16,
        protocol_versions: &[u8],
        handshake_timeout: Duration,
        udmux_token: Option<(&str, u64)>,
        rcc: Option<(std::net::Ipv4Addr, u16)>,
    ) -> Result<RakClient, RakError> {
        let addr: SocketAddr = (host, port)
            .to_socket_addrs()
            .map_err(|e| RakError::Network(e.to_string()))?
            .next()
            .ok_or(RakError::Network("host did not resolve".into()))?;
        let socket = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
            .map_err(|e| RakError::Network(e.to_string()))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| RakError::Network(e.to_string()))?;

        let guid: u64 = {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            (t.as_nanos() as u64) ^ 0x52555354424c4f58 // "RUSTBLOX"
        };

        let mut c = RakClient {
            socket,
            server: addr,
            guid,
            server_guid: 0,
            mtu: 1400,
            state: RakState::Connecting,
            ping_ms: None,
            envelope: None,
            client_tag: [0u8; 16],
            server_tag: None,
            fork: ForkGen::Roblox,
            reliability_index: 0,
            order_index: 0,
            datagram_seq: 0,
            split_id: 0,
            out_queue: HashMap::new(),
            frame_buf: Vec::new(),
            received_datagrams: HashSet::new(),
            ack_buffer: Vec::new(),
            delivered_ordered: 0,
            ordered_hold: HashMap::new(),
            splits: HashMap::new(),
            pending: VecDeque::new(),
            last_ping: Instant::now(),
            ping_time: None,
            last_recv: Instant::now(),
            buf: vec![0u8; 65536],
        };

        // Phase 0 (udmux sessions): pin the datagram ENVELOPE.
        //
        // The live capture (`sober_ro.pcap` + the 8-datagram handshake dump,
        // Nov 2026) shows the exact client header: `01 00 00 1f 01 11 01
        // <tag:16> 02 06 <rcc ip4> <port BE16>`, with a client-chosen tag and
        // the private RCC address in the mux field. When the join script
        // gives us that address we pin the measured shape without probing;
        // the legacy token-derived ping probe stays as fallback for sessions
        // where the private address is unavailable.
        //
        // The pre-connect tag is 16 bytes of the client's own choosing — the
        // capture proves it matches no join-script field.
        {
            let mut t = (guid
                ^ std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64)
                | 1;
            for chunk in c.client_tag.chunks_mut(8) {
                t = t.wrapping_add(0x9e3779b97f4a7c15);
                let mut z = t;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                z ^= z >> 31;
                let n = chunk.len();
                chunk.copy_from_slice(&z.to_le_bytes()[..n]);
            }
        }
        let known_shape = match (udmux_token, rcc) {
            (Some(_), Some((ip, port))) => {
                let env = crate::roblox::udmux::client_envelope(&c.client_tag, (ip, port));
                netlog!(
                    net,
                    "envelope: pinning the live-capture client shape (rcc {}:{}, tag {}) — skipping the probe",
                    ip,
                    port,
                    hex_head(&env.epoch, 16)
                );
                Some(env)
            }
            _ => None,
        };
        let envelopes: Vec<crate::roblox::udmux::Envelope> = match udmux_token {
            None => vec![crate::roblox::udmux::BARE],
            Some((tok, pepper)) => crate::roblox::udmux::envelope_candidates(tok, pepper),
        };
        let pinned = if known_shape.is_some() {
            known_shape
        } else if envelopes.len() > 1 {
            let p = c.probe_reachability(&envelopes, Duration::from_millis(700));
            match &p {
                Some(env) => netlog!(
                    net,
                    "envelope probe: '{}' drew a pong — pinning it for the session",
                    env.label
                ),
                None => netlog!(
                    net,
                    "envelope probe: no pong in any RUPP shape — trying the OCR1 matrix anyway"
                ),
            }
            p
        } else {
            None
        };

        // Phase 1: OPEN_CONNECTION_REQUEST_1 -> REPLY_1 (MTU probing).
        //
        // Matrix = envelope × offline id × protocol byte, FORK SHAPES FIRST
        // (0x7B + proto 5 — what the modern proxy actually answers), vanilla
        // ids as fallback for direct dials. With a pinned envelope all of
        // its shapes go first; without one bare shapes lead (a direct dial
        // must not inherit a token header), then each envelope candidate ×
        // the fork shape — enough to notice a shape that answers OCR1 but
        // not pings.
        let mut p1_variants: Vec<(crate::roblox::udmux::Envelope, u8, u8)> = Vec::new();
        match &pinned {
            Some(env) => {
                p1_variants.push((*env, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
                for &pr in protocol_versions {
                    if pr != RBX_PROTOCOL_VERSION {
                        p1_variants.push((*env, RBX_ID_OPEN_CONNECTION_REQUEST_1, pr));
                    }
                }
                p1_variants.push((*env, ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
            }
            None => {
                p1_variants.push((
                    crate::roblox::udmux::BARE,
                    RBX_ID_OPEN_CONNECTION_REQUEST_1,
                    RBX_PROTOCOL_VERSION,
                ));
                p1_variants.push((
                    crate::roblox::udmux::BARE,
                    ID_OPEN_CONNECTION_REQUEST_1,
                    RBX_PROTOCOL_VERSION,
                ));
                for &pr in protocol_versions {
                    if pr != RBX_PROTOCOL_VERSION {
                        p1_variants.push((
                            crate::roblox::udmux::BARE,
                            RBX_ID_OPEN_CONNECTION_REQUEST_1,
                            pr,
                        ));
                    }
                }
                if envelopes.len() > 1 {
                    for env in envelopes.iter().skip(1) {
                        p1_variants.push((
                            *env,
                            RBX_ID_OPEN_CONNECTION_REQUEST_1,
                            RBX_PROTOCOL_VERSION,
                        ));
                    }
                }
            }
        }
        let reply1 = match c.offline_roundtrip(
            &p1_variants,
            handshake_timeout,
            "request-1",
            |(env, id, proto), out, mtu| {
                out.push(*id);
                out.extend_from_slice(&OFFLINE_MAGIC);
                out.push(*proto);
                // MTU probe: pad to the datagram size (envelope header
                // included — the proxy judges the path MTU by the whole
                // datagram, and send_raw adds the header on top).
                out.resize((mtu as usize).saturating_sub(env.direction as usize), 0x00);
            },
            |(env, _, _)| *env,
            |c, payload| c.parse_reply1(payload),
        ) {
            Ok((r, winner)) => {
                let (env, id, proto) = p1_variants[winner];
                c.envelope = if env.direction == 0 { None } else { Some(env) };
                c.fork = if id == RBX_ID_OPEN_CONNECTION_REQUEST_1 {
                    ForkGen::Roblox
                } else {
                    ForkGen::Vanilla
                };
                netlog!(
                    net,
                    "handshake envelope '{}' (id 0x{:02x}, protocol {}) confirmed — applying it to every datagram this session",
                    env.label,
                    id,
                    proto
                );
                r
            }
            Err(e) => {
                // The ping probe already ran in phase 0, so the failure is
                // classifiable without re-probing: a pinned envelope that
                // still fails OCR1 means the proxy routed us but the RCC
                // rejected the handshake shape; total silence means the
                // proxy dropped every candidate.
                match &pinned {
                    Some(env) => netlog!(
                        net,
                        "path diagnosis: envelope '{}' got a pong but no OCR1 reply — \
                         the proxy routes this shape, the RCC rejects the handshake \
                         (protocol byte, MTU or required encryption)",
                        env.label
                    ),
                    None => netlog!(
                        net,
                        "path diagnosis: no envelope shape answered pings or OCR1 — the proxy \
                         drops unauthenticated datagrams (stale session port, firewall, or \
                         a routing shape we did not try)"
                    ),
                }
                return Err(e);
            }
        };
        let (server_guid, mtu, use_security) = reply1;
        netlog!(net, "REPLY_1: guid={} mtu={} security={}", server_guid, mtu, use_security);
        c.server_guid = server_guid;
        c.mtu = mtu;

        // Phase 2: OPEN_CONNECTION_REQUEST_2 -> REPLY_2.
        //
        // The fork's OCR2 has no security cookie and appends two fields the
        // vanilla packet does not carry: SupportedVersion (u32 BE — value
        // not published, matrix 5/1/0) and Capabilities (u64 BE — base
        // client set vs the modern superset). The server address block is
        // XOR-obfuscated (Sala readAddress: family, ip^0xFF, port BE).
        let mut p2_variants: Vec<(u8, u32, u64)> = Vec::new();
        for &sv in &[5u32, 1, 0] {
            p2_variants.push((RBX_ID_OPEN_CONNECTION_REQUEST_2, sv, RBX_CAPABILITY_MODERN));
            p2_variants.push((RBX_ID_OPEN_CONNECTION_REQUEST_2, sv, RBX_CAPABILITY_ROBLOX));
        }
        // Vanilla fallback (direct dials): the legacy OCR2 shape.
        p2_variants.push((ID_OPEN_CONNECTION_REQUEST_2, 0, 0));
        let guid = c.guid;
        // The envelope pinned in phase 1 (Copy) — every later datagram,
        // including OCR2, must ride the shape the proxy accepted.
        let pinned_env = c.envelope;
        let (reply2, _) = c.offline_roundtrip(
            &p2_variants,
            handshake_timeout,
            "request-2",
            |(id, sv, caps), out, mtu| {
                *out = build_ocr2(*id, *sv, *caps, addr, mtu, guid);
            },
            move |_| pinned_env.unwrap_or(crate::roblox::udmux::BARE),
            |c, payload| c.parse_reply2(payload),
        )?;
        netlog!(net, "REPLY_2: guid={} mtu={}", reply2.0, reply2.1);
        c.server_guid = reply2.0;

        // Phase 3: CONNECTION_REQUEST -> ACCEPTED -> NEW_INCOMING_CONNECTION.
        let deadline = Instant::now() + handshake_timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(RakError::TimedOut("connection request"));
            }
            let mut out = Vec::with_capacity(64);
            out.push(ID_CONNECTION_REQUEST);
            out.extend_from_slice(&c.guid.to_be_bytes());
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            out.extend_from_slice(&t.to_be_bytes());
            out.push(0); // useSecurity = false
            // The fork checks this 6-byte password (Sala Packet09:
            // DefaultPasswordBytes) — without it the RCC refuses silently.
            out.extend_from_slice(&RBX_PASSWORD);
            if let Err(e) = c.send_raw(&out) {
                netlog!(net, "CONNECTION_REQUEST send failed: {}", e);
            } else {
                netlog!(net, "CONNECTION_REQUEST sent");
            }

            if let Some(payload) = c.wait_readable(deadline) {
                // A 0-byte UDP datagram is legal — payload.first() keeps the
                // match panic-free (payload[0] here took the process down).
                match payload.split_first() {
                    Some((&ID_CONNECTION_REQUEST_ACCEPTED, _)) => {
                        c.handle_accepted(&payload)?;
                        break;
                    }
                    Some((&ID_CONNECTION_ATTEMPT_FAILED, _)) => return Err(RakError::Rejected("attempt failed")),
                    Some((&ID_NO_FREE_INCOMING_CONNECTIONS, _)) => return Err(RakError::Rejected("server full")),
                    Some((&ID_CONNECTION_BANNED, _)) => return Err(RakError::Banned),
                    Some((&ID_ALREADY_CONNECTED, _)) => return Err(RakError::AlreadyConnected),
                    Some((&other, _)) => netlog!(net, "got id 0x{:02x} while connecting", other),
                    None => netlog!(net, "ignoring empty datagram during handshake"),
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }

        c.state = RakState::Connected;
        c.last_recv = Instant::now();
        // Connected phase (live capture): the client switches its header to
        // class byte 0x02 and to the SERVER's 16-byte tag — both sides then
        // frame every datagram with the same session tag.
        if let Some(env) = c.envelope {
            if env.direction == 0x1f {
                let mut e2 = env;
                e2.cls = 0x02;
                if let Some(st) = c.server_tag {
                    e2.epoch = st;
                    netlog!(net, "envelope: adopting the server session tag {}", hex_head(&st, 16));
                }
                c.envelope = Some(e2);
            }
        }
        Ok(c)
    }

    /// Send one datagram to the server, applying the confirmed RUPP
    /// envelope. ALL session traffic must ride the exact clear-header shape
    /// the proxy accepted during the handshake — bare packets would no
    /// longer map to our client->RCC binding (if an envelope was what got
    /// us in). During the handshake every outgoing datagram is logged as
    /// hex (first 32 bytes, exactly as framed for the wire) — one look at
    /// the log shows which shape left the machine.
    fn send_raw(&self, buf: &[u8]) -> std::io::Result<()> {
        let owned;
        let out: &[u8] = match &self.envelope {
            None => buf,
            Some(env) if env.direction == 0 => buf,
            Some(env) => {
                owned = crate::roblox::udmux::rupp_frame(env, buf);
                &owned
            }
        };
        let res = self.socket.send_to(out, self.server).map(|_| ());
        if self.state == RakState::Connecting {
            netlog!(net, "out[{}] {}", out.len(), hex_head(out, 32));
        }
        res
    }

    /// Detect a RUPP-framed incoming datagram, log its clear header and
    /// return the inner bytes. Raw datagrams pass through unchanged — the
    /// server answers inside the envelope of its direction (0x1f), so this
    /// runs on every recv path.
    fn strip_incoming(&mut self, payload: Vec<u8>, ctx: &str) -> Vec<u8> {
        if let Some((dir, epoch, mux, hlen)) = crate::roblox::udmux::rupp_parse(&payload) {
            netlog!(
                net,
                "{}: RUPP reply dir 0x{:02x} epoch {} mux {} body {} bytes",
                ctx,
                dir,
                hex_head(&epoch, 16),
                hex_head(&mux, 8),
                payload.len() - hlen
            );
            // Live capture: the server announces its 16-byte session tag in
            // every 0x17/0x1f-framed reply; once connected both sides send
            // that SAME tag, so remember it (bare 0x04 frames carry none).
            if dir == 0x17 || dir == 0x1f {
                self.server_tag = Some(epoch);
            }
            payload[hlen..].to_vec()
        } else {
            payload
        }
    }

    /// Generic offline request/reply round.
    ///
    /// Every variant gets an equal slice of `timeout` (with mid-window
    /// retransmits for packet loss) — an earlier build handed the WHOLE
    /// budget to the first variant, so "4 protocol attempts" in the failure
    /// message really sent only the first packet shape. Any datagram that
    /// arrives is logged (first bytes) whether or not it parses, so a wrong
    /// reply shape is visible in the client log alone. `env_of` selects the
    /// datagram envelope for a variant (phase 1 probes several).
    fn offline_roundtrip<P, R, F>(
        &mut self,
        variants: &[P],
        timeout: Duration,
        label: &str,
        mut build: impl FnMut(&P, &mut Vec<u8>, u16),
        env_of: impl Fn(&P) -> crate::roblox::udmux::Envelope,
        mut parse: F,
    ) -> Result<(R, usize), RakError>
    where
        F: FnMut(&mut RakClient, &[u8]) -> Option<R>,
    {
        let deadline = Instant::now() + timeout;
        let per_variant = {
            let t = timeout / (variants.len().max(1) as u32);
            t.clamp(Duration::from_millis(600), Duration::from_millis(2500))
        };
        for (i, v) in variants.iter().enumerate() {
            let window_end = (Instant::now() + per_variant).min(deadline);
            let mut next_send = Instant::now();
            let mut sends = 0u32;
            let mut out = Vec::with_capacity(self.mtu as usize);
            while Instant::now() < window_end {
                if Instant::now() >= next_send {
                    out.clear();
                    build(v, &mut out, self.mtu);
                    // send_raw frames the body with this variant's envelope;
                    // phases after phase 1 always see their pinned envelope.
                    let env = env_of(v);
                    self.envelope = if env.direction == 0 { None } else { Some(env) };
                    sends += 1;
                    if let Err(e) = self.send_raw(&out) {
                        netlog!(net, "handshake {}: attempt {} send #{} failed: {}", label, i + 1, sends, e);
                    } else if sends == 1 {
                        netlog!(net, "handshake {}: attempt {} — {} bytes on the wire (retransmitting every 500 ms)", label, i + 1, out.len());
                    }
                    next_send += Duration::from_millis(500);
                }
                match self.socket.recv_from(&mut self.buf) {
                    Ok((n, from)) => {
                        let payload = self.buf[..n].to_vec();
                        if from != self.server {
                            netlog!(
                                net,
                                "handshake {}: datagram from unexpected addr {} ({} bytes, first byte 0x{:02x})",
                                label,
                                from,
                                n,
                                payload.first().copied().unwrap_or(0)
                            );
                            continue;
                        }
                        self.last_recv = Instant::now();
                        let payload = self.strip_incoming(payload, label);
                        if let Some(r) = parse(self, &payload) {
                            netlog!(net, "handshake {}: attempt {} got a valid reply after {} send(s)", label, i + 1, sends);
                            return Ok((r, i));
                        }
                        netlog!(
                            net,
                            "handshake {}: attempt {} — unparsed datagram ({} bytes: {})",
                            label,
                            i + 1,
                            n,
                            hex_head(&payload, 24)
                        );
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(8));
                    }
                    Err(e) => {
                        netlog!(net, "handshake {}: recv failed: {}", label, e);
                        break;
                    }
                }
            }
        }
        Err(RakError::TimedOut("offline handshake"))
    }

    /// Reachability probe: RakNet unconnected pings (0x01 / 0x02) with the
    /// standard magic, replayed through EVERY envelope candidate (bare +
    /// token-derived RUPP headers). Returns the envelope that drew a reply.
    /// This separates "the network path is dead" from "the handshake packet
    /// shape is rejected" AND pins down which clear-header shape the proxy
    /// expects.
    fn probe_reachability(
        &mut self,
        envelopes: &[crate::roblox::udmux::Envelope],
        per_env: Duration,
    ) -> Option<crate::roblox::udmux::Envelope> {
        for env in envelopes {
            self.envelope = if env.direction == 0 { None } else { Some(*env) };
            for pid in [0x01u8, 0x02u8] {
                let mut pkt = Vec::with_capacity(1 + 16 + 8);
                pkt.push(pid);
                pkt.extend_from_slice(&OFFLINE_MAGIC);
                pkt.extend_from_slice(&self.guid.to_le_bytes());
                if let Err(e) = self.send_raw(&pkt) {
                    netlog!(net, "envelope probe: ping 0x{:02x} via '{}' send failed: {}", pid, env.label, e);
                    continue;
                }
                let deadline = Instant::now() + per_env;
                while Instant::now() < deadline {
                    match self.socket.recv_from(&mut self.buf) {
                        Ok((n, from)) => {
                            if from != self.server {
                                continue;
                            }
                            self.last_recv = Instant::now();
                            let payload = self.strip_incoming(self.buf[..n].to_vec(), "envelope probe");
                            netlog!(
                                net,
                                "envelope probe: reply to 0x{:02x} via '{}' ({} bytes: {})",
                                pid,
                                env.label,
                                payload.len(),
                                hex_head(&payload, 24)
                            );
                            return Some(*env);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(8));
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        self.envelope = None;
        None
    }

    /// Block until a datagram arrives (nonblocking socket + sleep polling).
    /// Datagrams from other addresses are logged — a proxy answering from a
    /// different source port used to be silently discarded here.
    fn wait_readable(&mut self, deadline: Instant) -> Option<Vec<u8>> {
        while Instant::now() < deadline {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, from)) if from == self.server => {
                    self.last_recv = Instant::now();
                    return Some(self.strip_incoming(self.buf[..n].to_vec(), "recv"));
                }
                Ok((n, from)) => {
                    netlog!(
                        net,
                        "datagram from unexpected addr {} while connected to {} ({} bytes, first byte 0x{:02x})",
                        from,
                        self.server,
                        n,
                        self.buf.first().copied().unwrap_or(0)
                    );
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(8));
                }
                Err(_) => return None,
            }
        }
        None
    }

    fn parse_reply1(&mut self, payload: &[u8]) -> Option<(u64, u16, bool)> {
        // Both offline-id generations answer with the same field set; the
        // fork writes it BIG-endian (Sala Packet06: GUID u64BE, bool, MTU
        // u16BE) — the old parser read LE, so even a correct reply would
        // have parsed as garbage.
        match payload.first() {
            Some(&RBX_ID_OPEN_CONNECTION_REPLY_1) | Some(&ID_OPEN_CONNECTION_REPLY_1) => {}
            _ => return None,
        }
        let mut r = Reader::new(payload);
        let _id = r.u8()?;
        let _magic = r.bytes(16)?;
        let server_guid = r.u64be()?;
        let use_security = r.u8()? != 0;
        if use_security && payload[0] == ID_OPEN_CONNECTION_REPLY_1 {
            // Vanilla-only security cookie; the fork's REPLY_1 has none.
            let _cookie = r.u32be()?;
        }
        let mtu = r.u16be()?;
        Some((server_guid, mtu.max(400), use_security))
    }

    fn parse_reply2(&mut self, payload: &[u8]) -> Option<(u64, u16)> {
        let mut r = Reader::new(payload);
        match r.u8()? {
            RBX_ID_OPEN_CONNECTION_REPLY_2 => {
                let _magic = r.bytes(16)?;
                let server_guid = r.u64be()?;
                let _addr = read_address_rbx(&mut r)?;
                let mtu = r.u16be()?;
                let _security = r.u8()?;
                if r.rest().len() >= 12 {
                    let sv = r.u32be()?;
                    let caps = r.u64be()?;
                    netlog!(
                        net,
                        "REPLY_2 tail: supportedVersion={} capabilities=0x{:x}",
                        sv,
                        caps
                    );
                }
                Some((server_guid, mtu.max(400)))
            }
            ID_OPEN_CONNECTION_REPLY_2 => {
                let _magic = r.bytes(16)?;
                let server_guid = r.u64be()?;
                let _addr = read_address(&mut r)?;
                let mtu = r.u16be()?;
                Some((server_guid, mtu.max(400)))
            }
            _ => None,
        }
    }

    fn handle_accepted(&mut self, payload: &[u8]) -> Result<(), RakError> {
        let mut r = Reader::new(payload);
        let _id = r.u8();
        // ACCEPTED carries the client's external address (the server's view
        // of us), a system index, 10 address slots and two timestamps. The
        // fork writes addresses XOR-obfuscated (ip^0xFF, port BE) and all
        // multi-byte fields big-endian.
        let (client_addr, read_addr, sys_idx, rd_u64) = match self.fork {
            ForkGen::Roblox => (
                read_address_rbx(&mut r),
                read_address_rbx as fn(&mut Reader) -> Option<SocketAddr>,
                r.u16be().unwrap_or(0),
                (|r: &mut Reader| r.u64be().unwrap_or(0)) as fn(&mut Reader) -> u64,
            ),
            ForkGen::Vanilla => (
                read_address(&mut r),
                read_address as fn(&mut Reader) -> Option<SocketAddr>,
                r.u16().unwrap_or(0),
                (|r: &mut Reader| r.u64().unwrap_or(0)) as fn(&mut Reader) -> u64,
            ),
        };
        let _system_index = sys_idx;
        for _ in 0..10 {
            let _ = read_addr(&mut r);
        }
        let request_time = rd_u64(&mut r);
        let _server_time = rd_u64(&mut r);
        netlog!(
            net,
            "CONNECTION_REQUEST_ACCEPTED received (client external addr {:?})",
            client_addr
        );

        // Reply with the FULL NEW_INCOMING_CONNECTION the fork expects
        // (Sala Packet13): our external address + 10 address slots + the two
        // timestamps — the old two-address body was not parseable by the
        // RCC and left the connection dead right after the handshake.
        let mut out = Vec::with_capacity(96);
        out.push(ID_NEW_INCOMING_CONNECTION);
        match self.fork {
            ForkGen::Roblox => {
                write_address_rbx(&mut out, client_addr.unwrap_or_else(|| self.local_addr_for()));
                // 10 slots: our local address first, zero placeholders after
                // (vanilla fills them with the host's LAN addresses).
                write_address_rbx(&mut out, self.local_addr_for());
                for _ in 0..9 {
                    out.push(4);
                    out.extend_from_slice(&[0xFF; 4]); // 0.0.0.0 XORed
                    out.extend_from_slice(&0u16.to_be_bytes());
                }
                out.extend_from_slice(&request_time.to_be_bytes());
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                out.extend_from_slice(&now.to_be_bytes());
            }
            ForkGen::Vanilla => {
                write_address(&mut out, self.local_addr_for());
                write_address(&mut out, self.server);
            }
        }
        let _ = self.send_raw(&out);
        Ok(())
    }

    fn local_addr_for(&self) -> SocketAddr {
        self.socket
            .local_addr()
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
    }

    // -----------------------------------------------------------------
    // Sending
    // -----------------------------------------------------------------

    /// Queue a reliable-ordered frame (Roblox replication data).
    pub fn send_reliable(&mut self, payload: &[u8]) {
        self.send_frame(Frame::reliable_ordered(payload.to_vec()), true);
    }

    /// Queue an unreliable datagram frame.
    pub fn send_unreliable(&mut self, payload: &[u8]) {
        self.send_frame(Frame::unreliable(payload.to_vec()), false);
    }

    fn send_frame(&mut self, mut frame: Frame, resend: bool) {
        if self.state != RakState::Connected {
            return;
        }
        let seq = self.reliability_index;
        self.reliability_index = self.reliability_index.wrapping_add(1);
        frame.seq = seq;
        if matches!(frame.reliability, 1 | 3 | 4 | 7) {
            frame.order_index = self.order_index;
            self.order_index = self.order_index.wrapping_add(1);
        }

        // Split if it does not fit a datagram (leave room for datagram hdr).
        let max_payload = (self.mtu as usize).saturating_sub(4 + 16 + 64);
        if frame.payload.len() > max_payload {
            let chunks = frame.payload.chunks(max_payload).count() as u32;
            let split_id = self.split_id;
            self.split_id = self.split_id.wrapping_add(1);
            for (i, chunk) in frame.payload.chunks(max_payload).enumerate() {
                let f = Frame {
                    reliability: frame.reliability,
                    has_split: true,
                    seq,
                    order_index: frame.order_index,
                    order_channel: frame.order_channel,
                    split_id,
                    split_count: chunks,
                    split_index: i as u32,
                    payload: chunk.to_vec(),
                };
                let mut bytes = Vec::new();
                f.encode(&mut bytes);
                self.out_queue.insert(
                    seq,
                    OutFrame { reliability_index: seq, bytes: bytes.clone(), sent: Instant::now(), tries: 0 },
                );
                self.frame_buf.extend_from_slice(&bytes);
            }
            let _ = resend;
        } else {
            let mut bytes = Vec::with_capacity(frame.payload.len() + 8);
            frame.encode(&mut bytes);
            if matches!(frame.reliability, 2 | 3 | 4 | 6 | 7) {
                self.out_queue.insert(
                    seq,
                    OutFrame { reliability_index: seq, bytes: bytes.clone(), sent: Instant::now(), tries: 0 },
                );
            }
            self.frame_buf.extend_from_slice(&bytes);
        }
    }

    /// Flush queued frames into datagrams (call every tick). Frames are
    /// packed whole — a frame never spans datagrams.
    pub fn flush(&mut self) {
        if self.frame_buf.is_empty() {
            return;
        }
        if self.state != RakState::Connected {
            self.frame_buf.clear();
            return;
        }
        let buf = std::mem::take(&mut self.frame_buf);
        let max = (self.mtu as usize).saturating_sub(4);
        let mut dg: Vec<u8> = Vec::with_capacity(max);
        let mut pos = 0usize;
        let mut seq = self.datagram_seq;
        while pos < buf.len() {
            let flen = Frame::decode(&buf[pos..])
                .map(|f| frame_encoded_len(&f))
                .unwrap_or(buf.len() - pos);
            if !dg.is_empty() && dg.len() + 4 + flen > max {
                self.send_datagram(&dg, &mut seq);
                dg.clear();
            }
            let end = (pos + flen).min(buf.len());
            dg.extend_from_slice(&buf[pos..end]);
            pos = end;
        }
        if !dg.is_empty() {
            self.send_datagram(&dg, &mut seq);
        }
        self.datagram_seq = seq;
    }

    fn send_datagram(&mut self, dg: &[u8], seq: &mut u32) {
        let mut pkt = Vec::with_capacity(dg.len() + 4);
        pkt.push(DATAGRAM_VALID | 0x04);
        put_u24(&mut pkt, *seq);
        pkt.extend_from_slice(dg);
        let _ = self.send_raw(&pkt);
        *seq = seq.wrapping_add(1);
    }

    /// Resend overdue reliable frames; drop dead ones.
    pub fn tick(&mut self) {
        if self.state != RakState::Connected {
            return;
        }
        // Ping every second.
        if self.last_ping.elapsed() > Duration::from_secs(1) {
            self.last_ping = Instant::now();
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let mut out = vec![ID_CONNECTED_PING];
            out.extend_from_slice(&t.to_le_bytes());
            self.send_frame(Frame::unreliable(out), false);
            self.ping_time = Some((t, Instant::now()));
            netlog!(net, "ping sent");
        }

        // Resends.
        let now = Instant::now();
        let dead: Vec<u32> = self
            .out_queue
            .iter()
            .filter(|(_, f)| f.tries >= MAX_RESENDS)
            .map(|(k, _)| *k)
            .collect();
        for k in dead {
            self.out_queue.remove(&k);
            netlog!(net, "dropping dead reliable frame {}", k);
            self.state = RakState::Disconnected;
        }
        for (_, f) in self.out_queue.iter_mut() {
            if f.sent.elapsed() > Duration::from_millis(RESEND_MS) {
                f.sent = now;
                f.tries += 1;
                let bytes = f.bytes.clone();
                self.frame_buf.extend_from_slice(&bytes);
            }
        }
        self.flush();

        // Expire stale split assemblies.
        let stale: Vec<u16> = self
            .splits
            .iter()
            .filter(|(_, s)| s.first_seen.elapsed() > Duration::from_secs(30))
            .map(|(k, _)| *k)
            .collect();
        for k in stale {
            self.splits.remove(&k);
        }
    }

    // -----------------------------------------------------------------
    // Receiving
    // -----------------------------------------------------------------

    /// Pump the socket: returns the next application payload if available.
    /// Handles ACKs, pings, splits and ordered delivery internally.
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        if self.state != RakState::Connected {
            return None;
        }
        if self.pending.is_empty() {
            let recv = self.socket.recv_from(&mut self.buf);
            let n = match recv {
                Ok((n, from)) if from == self.server => {
                    self.last_recv = Instant::now();
                    n
                }
                Ok(_) => return None,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return None,
                Err(_) => return None,
            };
            let pkt: Vec<u8> = self.strip_incoming(self.buf[..n].to_vec(), "poll");
            self.handle_packet(&pkt);
        }
        self.pending.pop_front()
    }

    fn handle_packet(&mut self, pkt: &[u8]) {
        let first = match pkt.first() {
            Some(&f) => f,
            None => return,
        };
        if first & DATAGRAM_VALID == 0 {
            // Connected-level message ids (ping/pong/disconnect).
            self.handle_control(pkt);
            return;
        }
        if first & DATAGRAM_IS_ACK != 0 {
            self.handle_ack(pkt);
            return;
        }
        if first & DATAGRAM_IS_NAK != 0 {
            return;
        }
        let mut r = Reader::new(pkt);
        let _flags = match r.u8() {
            Some(v) => v,
            None => return,
        };
        let dg_seq = match r.u24() {
            Some(v) => v,
            None => return,
        };
        if !self.received_datagrams.insert(dg_seq) {
            // Duplicate datagram; re-ack but do not process.
            self.ack_buffer.push(dg_seq);
            self.send_acks();
            return;
        }
        self.ack_buffer.push(dg_seq);
        self.send_acks();

        let rest = r.rest();
        let mut pos = 0usize;
        while pos < rest.len() {
            match Frame::decode(&rest[pos..]) {
                Some(f) => {
                    let flen = frame_encoded_len(&f);
                    pos += flen;
                    if f.has_split {
                        if let Some(assembled) = self.reassemble(f) {
                            self.push_ordered_payload(assembled);
                        }
                    } else if matches!(f.reliability, 1 | 3 | 4 | 7) {
                        if let Some(p) = self.deliver_ordered(f) {
                            self.push_ordered_payload(p);
                        }
                    } else {
                        self.pending.push_back(f.payload);
                    }
                }
                None => {
                    netlog!(net, "garbled frame at {} ({} bytes left)", pos, rest.len() - pos);
                    break;
                }
            }
        }
    }

    /// Push an ordered-delivery payload; drain held follow-up frames too.
    fn push_ordered_payload(&mut self, first: Vec<u8>) {
        self.pending.push_back(first);
        loop {
            let next_seq = {
                // deliver_ordered tracks delivered count internally; held
                // frames are drained by sequence below.
                match self.ordered_hold.keys().min() {
                    Some(k) => *k,
                    None => return,
                }
            };
            if let Some(f) = self.ordered_hold.remove(&next_seq) {
                self.pending.push_back(f.payload);
            } else {
                return;
            }
        }
    }

    fn handle_control(&mut self, pkt: &[u8]) -> Option<Vec<u8>> {
        match pkt[0] {
            ID_CONNECTED_PING => {
                // Echo pong.
                if pkt.len() >= 9 {
                    let mut out = vec![ID_CONNECTED_PONG];
                    out.extend_from_slice(&pkt[1..9]);
                    let _ = self.send_raw(&out);
                }
                None
            }
            ID_CONNECTED_PONG => {
                if pkt.len() >= 9 && self.ping_time.is_some() {
                    let sent_ms = self.ping_time.as_ref().map(|(t, _)| *t).unwrap_or(0);
                    let _ = pkt[1..9];
                    let elapsed = self
                        .ping_time
                        .take()
                        .map(|(_, at)| at.elapsed().as_secs_f32() * 1000.0)
                        .unwrap_or(0.0);
                    let _ = sent_ms;
                    self.ping_ms = Some(elapsed);
                    netlog!(net, "pong: {:.1} ms", elapsed);
                }
                None
            }
            ID_DISCONNECTION_NOTIFICATION | ID_CONNECTION_LOST => {
                netlog!(net, "server disconnected us (0x{:02x})", pkt[0]);
                self.state = RakState::Disconnected;
                None
            }
            other => {
                // Some servers push small in-band control messages; surface
                // them to the caller for logging/forwarding.
                netlog!(net, "control id 0x{:02x} ({} bytes)", other, pkt.len());
                self.pending.push_back(pkt.to_vec());
                None
            }
        }
    }

    fn handle_ack(&mut self, pkt: &[u8]) {
        let mut r = Reader::new(pkt);
        let _flags = r.u8();
        // The record count is BIG-endian (raknet-dec: `count = u16be()`;
        // Anaminus notes it as "probably an oversight" in the original
        // RakNet code). The record VALUES are u24 LE.
        let count = r.u16be().unwrap_or(0);
        for _ in 0..count {
            let kind = match r.u8() {
                Some(k) => k,
                None => break,
            };
            let (lo, hi) = match kind {
                0 => match (r.u24(), r.u24()) {
                    (Some(a), Some(b)) => (a, b),
                    _ => break,
                },
                _ => match r.u24() {
                    Some(a) => (a, a),
                    None => break,
                },
            };
            for seq in lo..=hi.min(lo + 8192) {
                self.out_queue.remove(&seq);
            }
        }
    }

    fn send_acks(&mut self) {
        if self.ack_buffer.is_empty() {
            return;
        }
        // Compress into [lo..hi] ranges.
        self.ack_buffer.sort_unstable();
        self.ack_buffer.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for &v in &self.ack_buffer {
            match ranges.last_mut() {
                Some((_, hi)) if *hi + 1 == v => *hi = v,
                _ => ranges.push((v, v)),
            }
        }
        let mut out = vec![DATAGRAM_VALID | DATAGRAM_IS_ACK];
        out.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
        for (lo, hi) in ranges {
            if lo != hi {
                out.push(0);
                put_u24(&mut out, lo);
                put_u24(&mut out, hi);
            } else {
                out.push(1);
                put_u24(&mut out, lo);
            }
        }
        let _ = self.send_raw(&out);
        self.ack_buffer.clear();
    }

    fn reassemble(&mut self, f: Frame) -> Option<Vec<u8>> {
        let entry = self
            .splits
            .entry(f.split_id)
            .or_insert_with(|| SplitState {
                count: f.split_count,
                parts: HashMap::new(),
                first_seen: Instant::now(),
            });
        entry.parts.insert(f.split_index, f.payload);
        if entry.parts.len() == entry.count as usize {
            let st = self.splits.remove(&f.split_id)?;
            let mut data = Vec::new();
            for i in 0..st.count {
                data.extend_from_slice(st.parts.get(&i)?);
            }
            Some(data)
        } else {
            None
        }
    }

    fn deliver_ordered(&mut self, f: Frame) -> Option<Vec<u8>> {
        // Ordering key: the fork's order_index (u24 LE + channel), not the
        // message number — reliable-unordered frames skip ordering entirely.
        if (f.order_index as i64) < (self.delivered_ordered as i64) {
            // Already delivered (duplicate).
            return None;
        }
        let key = f.order_index;
        self.ordered_hold.insert(key, f);
        if let Some(next) = self.ordered_hold.remove(&self.delivered_ordered) {
            self.delivered_ordered = self.delivered_ordered.wrapping_add(1);
            let payload = next.payload;
            // Drain consecutive held frames.
            loop {
                match self.ordered_hold.remove(&self.delivered_ordered) {
                    Some(n) => {
                        self.delivered_ordered = self.delivered_ordered.wrapping_add(1);
                        self.pending.push_back(n.payload);
                    }
                    None => break,
                }
            }
            return Some(payload);
        }
        None
    }

    /// Number of payloads buffered behind the ordered delivery head.
    pub fn pending_ordered(&self) -> usize {
        self.ordered_hold.len()
    }

    /// Connection idle time (used by the session to detect death).
    pub fn idle(&self) -> Duration {
        self.last_recv.elapsed()
    }

    pub fn close(&mut self) {
        if self.state == RakState::Connected {
            let out = [ID_DISCONNECTION_NOTIFICATION];
            let _ = self.send_raw(&out);
            self.flush();
        }
        self.state = RakState::Disconnected;
    }
}

fn frame_encoded_len(f: &Frame) -> usize {
    let mut n = 1 + 2; // flags + bit_length
    if matches!(f.reliability, 2 | 3 | 4 | 6 | 7) {
        n += 3; // msg number
    }
    if matches!(f.reliability, 1 | 4) {
        n += 3; // sequencing index
    }
    if matches!(f.reliability, 1 | 3 | 4 | 7) {
        n += 4; // order index + channel
    }
    if f.has_split {
        n += 4 + 2 + 4;
    }
    n + f.payload.len()
}

/// Build an OCR2 datagram body for either offline-id generation (the
/// phase-2 matrix step). The fork shape (Sala Packet07Layer):
/// `[0x78][magic][addr: family, ip^0xFF, port BE][mtu BE][guid BE]
/// [supportedVersion u32 BE][capabilities u64 BE]`; vanilla keeps the
/// legacy two-field tail.
fn build_ocr2(
    id: u8,
    supported_version: u32,
    caps: u64,
    server: SocketAddr,
    mtu: u16,
    client_guid: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.push(id);
    out.extend_from_slice(&OFFLINE_MAGIC);
    match id {
        RBX_ID_OPEN_CONNECTION_REQUEST_2 => {
            write_address_rbx(&mut out, server);
            out.extend_from_slice(&mtu.to_be_bytes());
            out.extend_from_slice(&client_guid.to_be_bytes());
            out.extend_from_slice(&supported_version.to_be_bytes());
            out.extend_from_slice(&caps.to_be_bytes());
        }
        _ => {
            write_address(&mut out, server);
            out.extend_from_slice(&mtu.to_be_bytes());
            out.extend_from_slice(&client_guid.to_be_bytes());
        }
    }
    out
}

/// Roblox fork address block (Sala ExtendedReader::readAddress):
/// `family(0x04) | every IP byte XOR 0xFF | port u16 BE` — IP first, then
/// the port. The old encoder wrote the vanilla order (port first) without
/// the XOR, so every address the RCC parsed came out as 0.0.0.0.
fn read_address_rbx(r: &mut Reader) -> Option<SocketAddr> {
    let family = r.u8()?;
    if family == 4 {
        let ip = r.bytes(4)?;
        let port = r.u16be()?;
        Some(SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                ip[0] ^ 0xFF,
                ip[1] ^ 0xFF,
                ip[2] ^ 0xFF,
                ip[3] ^ 0xFF,
            )),
            port,
        ))
    } else {
        let ip = r.bytes(16)?;
        let mut o = [0u8; 16];
        o.copy_from_slice(ip);
        o.iter_mut().for_each(|b| *b ^= 0xFF);
        let port = r.u16be()?;
        Some(SocketAddr::new(std::net::IpAddr::V6(o.into()), port))
    }
}

fn write_address_rbx(out: &mut Vec<u8>, addr: SocketAddr) {
    out.push(4);
    match addr.ip() {
        std::net::IpAddr::V4(v4) => out.extend(v4.octets().map(|b| b ^ 0xFF)),
        std::net::IpAddr::V6(v6) => out.extend(v6.octets().map(|b| b ^ 0xFF)),
    }
    out.extend_from_slice(&addr.port().to_be_bytes());
}

/// Legacy vanilla address block (direct-dial fallback): family | port BE |
/// raw IP — the shape the old parser spoke.
fn read_address(r: &mut Reader) -> Option<SocketAddr> {
    let family = r.u8()?;
    if family == 4 {
        // Ports ride in network byte order (they come straight out of
        // sockaddr_in on both ends of vanilla RakNet and its forks).
        let port = r.u16()?;
        let ip = r.bytes(4)?;
        Some(SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])),
            u16::from_be(port),
        ))
    } else {
        let port = r.u16()?;
        let ip = r.bytes(16)?;
        let mut o = [0u8; 16];
        o.copy_from_slice(ip);
        Some(SocketAddr::new(
            std::net::IpAddr::V6(o.into()),
            u16::from_be(port),
        ))
    }
}

fn write_address(out: &mut Vec<u8>, addr: SocketAddr) {
    out.push(4);
    out.extend_from_slice(&addr.port().to_be_bytes());
    match addr.ip() {
        std::net::IpAddr::V4(v4) => out.extend_from_slice(&v4.octets()),
        std::net::IpAddr::V6(v6) => out.extend_from_slice(&v6.octets()),
    }
}

/// Diagnostic probe of the NetStack side-channel (join hypothesis (b)): the
/// join script carries `NetStackPort` + `NetStackTokenValue` — if game
/// traffic moved off the legacy udmux port, an OCR1 there should draw a
/// reply while the main channel stays silent. Sends the fork OCR1 through
/// every RUPP envelope candidate for the netstack token (plus the bare
/// vanilla fallback) and reports which shape answered. Read-only: never
/// touches session state; the verdict goes to the log.
pub fn netstack_probe(host: &str, port: u16, token: &str, per_shape: Duration) -> Option<String> {
    let addr: SocketAddr = match (host, port).to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => a,
            None => return None,
        },
        Err(e) => {
            netlog!(net, "netstack probe: resolve {}:{} failed: {}", host, port, e);
            return None;
        }
    };
    let socket = match UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }) {
        Ok(s) => s,
        Err(e) => {
            netlog!(net, "netstack probe: bind failed: {}", e);
            return None;
        }
    };
    let _ = socket.set_nonblocking(true);

    let mut shapes: Vec<(String, Vec<u8>)> = Vec::new();
    for env in crate::roblox::udmux::envelope_candidates(token, 0) {
        let mut body = Vec::with_capacity(1400);
        body.push(RBX_ID_OPEN_CONNECTION_REQUEST_1);
        body.extend_from_slice(&OFFLINE_MAGIC);
        body.push(RBX_PROTOCOL_VERSION);
        body.resize(1400usize.saturating_sub(env.direction as usize), 0x00);
        let wire = if env.direction == 0 {
            body
        } else {
            crate::roblox::udmux::rupp_frame(&env, &body)
        };
        shapes.push((format!("{}+0x7b", env.label), wire));
    }
    let mut vanilla = Vec::with_capacity(1400);
    vanilla.push(ID_OPEN_CONNECTION_REQUEST_1);
    vanilla.extend_from_slice(&OFFLINE_MAGIC);
    vanilla.push(RBX_PROTOCOL_VERSION);
    vanilla.resize(1400, 0x00);
    shapes.push(("bare+0x05".into(), vanilla));

    for (label, wire) in &shapes {
        if socket.send_to(wire, addr).is_err() {
            continue;
        }
        netlog!(net, "netstack probe: sent '{}' ({} bytes: {})", label, wire.len(), hex_head(wire, 32));
        let deadline = Instant::now() + per_shape;
        while Instant::now() < deadline {
            let mut buf = vec![0u8; 2048];
            match socket.recv_from(&mut buf) {
                Ok((n, from)) if from == addr => {
                    netlog!(
                        net,
                        "netstack probe: '{}' drew a reply ({} bytes: {})",
                        label,
                        n,
                        hex_head(&buf[..n], 32)
                    );
                    return Some(label.clone());
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(8));
                }
                Err(_) => break,
            }
        }
    }
    netlog!(
        net,
        "netstack probe: total silence across {} shapes — the NetStack channel is not answering for this session",
        shapes.len()
    );
    None
}

/// Compact hex dump for log lines.
fn hex_head(buf: &[u8], n: usize) -> String {
    buf.iter()
        .take(n)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_magic_is_the_canonical_raknet_bytes() {
        // Guards against re-guessing the magic: Roblox keeps the vanilla
        // OFFLINE_MESSAGE_DATA_ID (see the const docs). A wrong byte here
        // made every server silently drop our handshake.
        assert_eq!(
            OFFLINE_MAGIC,
            [
                0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe,
                0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
            ]
        );
    }

    #[test]
    fn frame_roundtrip_reliable() {
        let f = Frame {
            reliability: 2,
            has_split: false,
            seq: 0x123456,
            order_index: 0,
            order_channel: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload: vec![1, 2, 3, 4, 5],
        };
        let mut b = Vec::new();
        f.encode(&mut b);
        let g = Frame::decode(&b).unwrap();
        assert_eq!(g.reliability, 2);
        assert!(!g.has_split);
        assert_eq!(g.seq, 0x123456);
        assert_eq!(g.payload, vec![1, 2, 3, 4, 5]);
        assert_eq!(b.len(), frame_encoded_len(&g));
    }

    /// The exact byte layout a real server parses: reliability in the TOP
    /// flag bits, bit_length BE, msg number + order index LE, split fields
    /// BE. The old encoder put reliability in bits 0..2 and omitted
    /// bit_length entirely — the server read "unreliable" frames with
    /// garbage boundaries.
    #[test]
    fn frame_byte_layout_matches_the_fork() {
        let f = Frame {
            reliability: 3,
            has_split: false,
            seq: 0x123456,
            order_index: 7,
            order_channel: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload: vec![1, 2, 3, 4, 5],
        };
        let mut b = Vec::new();
        f.encode(&mut b);
        assert_eq!(
            b,
            vec![
                0x60, 0x00, 0x28, // flags (rel 3 << 5), bit_length = 40 BE
                0x56, 0x34, 0x12, // msg number LE
                0x07, 0x00, 0x00, 0x00, // order index LE + channel
                1, 2, 3, 4, 5,
            ]
        );
        let g = Frame::decode(&b).unwrap();
        assert_eq!(g.reliability, 3);
        assert_eq!(g.seq, 0x123456);
        assert_eq!(g.order_index, 7);
        assert_eq!(g.payload, vec![1, 2, 3, 4, 5]);
        assert_eq!(b.len(), frame_encoded_len(&g));
    }

    #[test]
    fn frame_split_fields_are_big_endian() {
        let f = Frame {
            reliability: 2,
            has_split: true,
            seq: 1,
            order_index: 0,
            order_channel: 0,
            split_id: 9,
            split_count: 5,
            split_index: 3,
            payload: vec![0xaa; 11],
        };
        let mut b = Vec::new();
        f.encode(&mut b);
        // flags 0x50 (rel 2 << 5 | split 0x10), bit_length 88 BE, msg# 1 LE,
        // count 5 BE, id 9 BE, index 3 BE, payload.
        assert_eq!(&b[0..3], &[0x50, 0x00, 88]);
        assert_eq!(&b[3..6], &[0x01, 0x00, 0x00]);
        assert_eq!(&b[6..16], &[0, 0, 0, 5, 0, 9, 0, 0, 0, 3]);
        let g = Frame::decode(&b).unwrap();
        assert_eq!((g.split_count, g.split_id, g.split_index), (5, 9, 3));
        assert_eq!(b.len(), frame_encoded_len(&g));
    }

    #[test]
    fn frame_unreliable_roundtrip() {
        let f = Frame::unreliable(vec![9, 8, 7]);
        let mut b = Vec::new();
        f.encode(&mut b);
        let g = Frame::decode(&b).unwrap();
        assert_eq!(g.reliability, 0);
        assert_eq!(g.seq, 0);
        assert_eq!(g.payload, vec![9, 8, 7]);
    }

    #[test]
    fn rbx_address_xor_roundtrip() {
        let addr: SocketAddr = "128.116.13.34:63646".parse().unwrap();
        let mut buf = Vec::new();
        write_address_rbx(&mut buf, addr);
        assert_eq!(buf[0], 4);
        assert_eq!(
            &buf[1..5],
            &[128 ^ 0xFF, 116 ^ 0xFF, 13 ^ 0xFF, 34 ^ 0xFF]
        );
        assert_eq!(&buf[5..7], &63646u16.to_be_bytes());
        let mut r = Reader::new(&buf);
        assert_eq!(read_address_rbx(&mut r), Some(addr));
    }

    /// The phase-2 packet, byte for byte: id 0x78, magic, XOR address,
    /// MTU BE, GUID BE, then the two fork-only tail fields BE.
    #[test]
    fn rbx_ocr2_layout_is_the_fork_shape() {
        let srv: SocketAddr = "10.30.2.71:63646".parse().unwrap();
        let pkt = build_ocr2(
            RBX_ID_OPEN_CONNECTION_REQUEST_2,
            5,
            RBX_CAPABILITY_MODERN,
            srv,
            1400,
            0xAABB_CCDD_EEFF_0011,
        );
        assert_eq!(pkt[0], 0x78);
        assert_eq!(&pkt[1..17], &OFFLINE_MAGIC);
        assert_eq!(pkt[17], 4);
        assert_eq!(
            &pkt[18..22],
            &[10 ^ 0xFF, 30 ^ 0xFF, 2 ^ 0xFF, 71 ^ 0xFF]
        );
        assert_eq!(&pkt[22..24], &63646u16.to_be_bytes());
        assert_eq!(&pkt[24..26], &1400u16.to_be_bytes());
        assert_eq!(&pkt[26..34], &0xAABB_CCDD_EEFF_0011u64.to_be_bytes());
        assert_eq!(&pkt[34..38], &5u32.to_be_bytes());
        assert_eq!(&pkt[38..46], &RBX_CAPABILITY_MODERN.to_be_bytes());
        assert_eq!(pkt.len(), 46);
    }

    #[test]
    fn ack_encoding() {
        // Mirror what send_acks produces for a range: the record count is
        // BE, the record VALUES are u24 LE (raknet-dec).
        let mut out = vec![DATAGRAM_VALID | DATAGRAM_IS_ACK];
        out.extend_from_slice(&2u16.to_be_bytes());
        out.push(0);
        put_u24(&mut out, 3);
        put_u24(&mut out, 6);
        out.push(1);
        put_u24(&mut out, 10);
        let mut r = Reader::new(&out);
        assert_eq!(r.u8().unwrap() & DATAGRAM_IS_ACK, DATAGRAM_IS_ACK);
        assert_eq!(r.u16be().unwrap(), 2);
        assert_eq!(r.u8().unwrap(), 0);
        assert_eq!(r.u24().unwrap(), 3);
        assert_eq!(r.u24().unwrap(), 6);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u24().unwrap(), 10);
    }

    #[test]
    fn two_clients_loopback() {
        // Spin a fake RakNet server speaking the FORK wire format on
        // localhost and run the full handshake, then exchange a reliable
        // frame both ways. Exercises the whole pipeline end to end: 0x7B ->
        // 0x7E (BE), 0x78 -> 0x7D (BE, XOR addr), 0x09 with the password ->
        // 0x10 -> full 0x13, then fork-framed data datagrams.
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(false).unwrap();
        let sport = server.local_addr().unwrap().port();
        let _ = sport;

        let server_thread = std::thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            // REQUEST_1 (fork id; the client matrix sends it first).
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], RBX_ID_OPEN_CONNECTION_REQUEST_1);
            assert_eq!(&buf[1..17], &OFFLINE_MAGIC);
            assert_eq!(buf[17], RBX_PROTOCOL_VERSION);
            let _ = n;
            server
                .send_to(
                    &[
                        &[RBX_ID_OPEN_CONNECTION_REPLY_1],
                        &OFFLINE_MAGIC[..],
                        &1234567u64.to_be_bytes()[..],
                        &[0],
                        &1400u16.to_be_bytes()[..],
                    ]
                    .concat(),
                    peer,
                )
                .unwrap();
            // REQUEST_2 (fork: family | ip^FF | port BE | mtu BE | guid BE
            // | supportedVersion BE | capabilities BE).
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], RBX_ID_OPEN_CONNECTION_REQUEST_2);
            let _ = n;
            assert_eq!(buf[17], 4); // family
            assert_eq!(buf[18], 127 ^ 0xFF); // ip[0] XOR-obfuscated
            server
                .send_to(
                    &[
                        &[RBX_ID_OPEN_CONNECTION_REPLY_2],
                        &OFFLINE_MAGIC[..],
                        &1234567u64.to_be_bytes()[..],
                        &[4],
                        &[127 ^ 0xFF, 0 ^ 0xFF, 0 ^ 0xFF, 1 ^ 0xFF],
                        &peer.port().to_be_bytes()[..],
                        &1400u16.to_be_bytes()[..],
                        &[0],                       // useSecurity
                        &5u32.to_be_bytes()[..],    // supportedVersion
                        &RBX_CAPABILITY_MODERN.to_be_bytes()[..],
                    ]
                    .concat(),
                    peer,
                )
                .unwrap();
            // CONNECTION_REQUEST: guid BE, time BE, security 0, password.
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], ID_CONNECTION_REQUEST);
            assert_eq!(&buf[18..24], &RBX_PASSWORD[..]);
            let _ = n;
            // ACCEPTED: client addr (XOR) + systemIndex BE + 10 addrs + 2
            // times BE.
            let mut acc = vec![ID_CONNECTION_REQUEST_ACCEPTED];
            acc.push(4);
            acc.extend_from_slice(&[127 ^ 0xFF, 0 ^ 0xFF, 0 ^ 0xFF, 1 ^ 0xFF]);
            acc.extend_from_slice(&peer.port().to_be_bytes());
            acc.extend_from_slice(&0u16.to_be_bytes()); // systemIndex
            for _ in 0..10 {
                acc.push(4);
                acc.extend_from_slice(&[0xFF; 4]); // 0.0.0.0 XORed
                acc.extend_from_slice(&0u16.to_be_bytes());
            }
            acc.extend_from_slice(&111u64.to_be_bytes()); // request time
            acc.extend_from_slice(&222u64.to_be_bytes()); // server time
            server.send_to(&acc, peer).unwrap();
            // NEW_INCOMING_CONNECTION — the full fork layout: 11 address
            // blocks + 16 bytes of timestamps.
            let (n, _peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], ID_NEW_INCOMING_CONNECTION);
            assert_eq!(n, 1 + 11 * 7 + 16);
            let _ = n;

            // Receive a reliable frame datagram and reply with one.
            loop {
                let (n, peer) = server.recv_from(&mut buf).unwrap();
                let pkt = &buf[..n];
                if pkt[0] == ID_DISCONNECTION_NOTIFICATION {
                    break;
                }
                if pkt[0] & 0x80 != 0 && pkt[0] & 0xC0 != 0xC0 {
                    // Data datagram: frame inside at offset 4.
                    let f = Frame::decode(&pkt[4..]).unwrap();
                    assert_eq!(f.payload, b"hello-rustblox");
                    // Reply: our own datagram with a reliable-ordered frame
                    // (encode now produces the fork layout).
                    let mut dg = vec![0x84];
                    put_u24(&mut dg, 0);
                    let rf = Frame {
                        reliability: 3,
                        has_split: false,
                        seq: 0,
                        order_index: 0,
                        order_channel: 0,
                        split_id: 0,
                        split_count: 0,
                        split_index: 0,
                        payload: b"reply-from-server".to_vec(),
                    };
                    rf.encode(&mut dg);
                    server.send_to(&dg, peer).unwrap();
                    // Then wait for the client ACK (loop again).
                    let (n2, _) = server.recv_from(&mut buf).unwrap();
                    let _ = n2;
                    break;
                }
            }
        });

        let mut client =
            RakClient::connect("127.0.0.1", sport, &[10, 5], Duration::from_secs(5), None, None)
                .expect("handshake failed");
        assert_eq!(client.state, RakState::Connected);
        assert_eq!(client.server_guid, 1234567);
        assert_eq!(client.fork, ForkGen::Roblox);

        client.send_reliable(b"hello-rustblox");
        client.flush();

        // Pump until we get the reply.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = None;
        while Instant::now() < deadline {
            if let Some(p) = client.poll() {
                got = Some(p);
                break;
            }
            client.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.as_deref(), Some(&b"reply-from-server"[..]));
        client.close();
        server_thread.join().unwrap();
    }
}
