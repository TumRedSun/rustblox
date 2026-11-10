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
pub const OFFLINE_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe, 0xfe,
];

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

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub reliable: bool,
    pub sequenced: bool,
    pub has_split: bool,
    pub seq: u32,        // reliability index (ordering for ReliableOrdered)
    pub split_id: u16,
    pub split_count: u32,
    pub split_index: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn reliable_ordered(payload: Vec<u8>) -> Frame {
        Frame {
            reliable: true,
            sequenced: false,
            has_split: false,
            seq: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload,
        }
    }

    pub fn unreliable(payload: Vec<u8>) -> Frame {
        Frame {
            reliable: false,
            sequenced: false,
            has_split: false,
            seq: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        // flags: bit4 = split; bits 0..2 = reliability kind
        // 2 = reliable, 3 = reliable ordered, 0 = unreliable
        let rel: u8 = if self.reliable {
            if self.sequenced { 3 } else { 2 }
        } else {
            0
        };
        let flags = rel | if self.has_split { 0x10 } else { 0x00 };
        out.push(flags);
        if self.reliable {
            put_u24(out, self.seq);
        }
        if self.has_split {
            out.extend_from_slice(&self.split_count.to_le_bytes());
            out.extend_from_slice(&self.split_id.to_le_bytes());
            out.extend_from_slice(&self.split_index.to_le_bytes());
        }
        out.extend_from_slice(&self.payload);
    }

    pub fn decode(buf: &[u8]) -> Option<Frame> {
        let mut r = Reader::new(buf);
        let flags = r.u8()?;
        let rel = flags & 0x07;
        let has_split = flags & 0x10 != 0;
        let reliable = matches!(rel, 2 | 3 | 4 | 5 | 6 | 7);
        let mut f = Frame {
            reliable,
            sequenced: matches!(rel, 1 | 5),
            has_split,
            seq: 0,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload: Vec::new(),
        };
        if reliable {
            f.seq = r.u24()?;
        }
        if f.sequenced {
            let _seq_idx = r.u24()?;
        }
        if has_split {
            f.split_count = r.u32()?;
            f.split_id = r.u16()?;
            f.split_index = r.u32()?;
        }
        f.payload = r.rest().to_vec();
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

pub struct RakClient {
    socket: UdpSocket,
    pub server: SocketAddr,
    pub guid: u64,
    pub server_guid: u64,
    pub mtu: u16,
    pub state: RakState,
    pub ping_ms: Option<f32>,

    // Send side
    reliability_index: u32,
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
    pub fn connect(
        host: &str,
        port: u16,
        protocol_versions: &[u8],
        handshake_timeout: Duration,
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
            reliability_index: 0,
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

        // Phase 1: OPEN_CONNECTION_REQUEST_1 -> REPLY_1 (MTU probing).
        let reply1 = c.offline_roundtrip(
            protocol_versions,
            handshake_timeout,
            |protocol, out, mtu| {
                out.push(ID_OPEN_CONNECTION_REQUEST_1);
                out.extend_from_slice(&OFFLINE_MAGIC);
                out.push(*protocol);
                // MTU probe: pad to the datagram size.
                out.resize(mtu as usize, 0x00);
            },
            |c, payload| c.parse_reply1(payload),
        )?;
        let (server_guid, mtu, use_security) = reply1;
        netlog!(net, "REPLY_1: guid={} mtu={} security={}", server_guid, mtu, use_security);
        c.server_guid = server_guid;
        c.mtu = mtu;

        // Phase 2: OPEN_CONNECTION_REQUEST_2 -> REPLY_2.
        let guid = c.guid;
        let reply2 = c.offline_roundtrip(
            &[()],
            handshake_timeout,
            move |_, out, _mtu| {
                out.push(ID_OPEN_CONNECTION_REQUEST_2);
                out.extend_from_slice(&OFFLINE_MAGIC);
                // server address (family 4)
                out.push(4);
                out.extend_from_slice(&addr.port().to_le_bytes());
                match addr.ip() {
                    std::net::IpAddr::V4(v4) => out.extend_from_slice(&v4.octets()),
                    std::net::IpAddr::V6(v6) => out.extend_from_slice(&v6.octets()),
                }
                out.extend_from_slice(&mtu.to_le_bytes());
                out.extend_from_slice(&guid.to_le_bytes());
            },
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
            out.extend_from_slice(&c.guid.to_le_bytes());
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            out.extend_from_slice(&t.to_le_bytes());
            out.push(0); // useSecurity = false
            let _ = c.socket.send_to(&out, c.server);
            netlog!(net, "CONNECTION_REQUEST sent");

            if let Some(payload) = c.wait_readable(deadline) {
                if payload[0] == ID_CONNECTION_REQUEST_ACCEPTED {
                    c.handle_accepted(&payload)?;
                    break;
                }
                match payload[0] {
                    ID_CONNECTION_ATTEMPT_FAILED => return Err(RakError::Rejected("attempt failed")),
                    ID_NO_FREE_INCOMING_CONNECTIONS => return Err(RakError::Rejected("server full")),
                    ID_CONNECTION_BANNED => return Err(RakError::Banned),
                    ID_ALREADY_CONNECTED => return Err(RakError::AlreadyConnected),
                    _ => netlog!(net, "got id 0x{:02x} while connecting", payload[0]),
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }

        c.state = RakState::Connected;
        c.last_recv = Instant::now();
        Ok(c)
    }

    /// Generic offline request/reply round with per-attempt timeout.
    fn offline_roundtrip<P, R, F>(
        &mut self,
        variants: &[P],
        timeout: Duration,
        mut build: impl FnMut(&P, &mut Vec<u8>, u16),
        mut parse: F,
    ) -> Result<R, RakError>
    where
        F: FnMut(&mut RakClient, &[u8]) -> Option<R>,
    {
        let deadline = Instant::now() + timeout;
        for v in variants {
            loop {
                if Instant::now() >= deadline {
                    return Err(RakError::TimedOut("offline handshake"));
                }
                let mut out = Vec::with_capacity(self.mtu as usize);
                build(v, &mut out, self.mtu);
                let _ = self.socket.send_to(&out, self.server);
                if let Some(payload) = self.wait_readable(deadline) {
                    if let Some(r) = parse(self, &payload) {
                        return Ok(r);
                    }
                } else {
                    break;
                }
                std::thread::sleep(Duration::from_millis(80));
            }
        }
        Err(RakError::TimedOut("offline handshake"))
    }

    /// Block until a datagram arrives (nonblocking socket + sleep polling).
    fn wait_readable(&mut self, deadline: Instant) -> Option<Vec<u8>> {
        while Instant::now() < deadline {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, from)) if from == self.server => {
                    self.last_recv = Instant::now();
                    return Some(self.buf[..n].to_vec());
                }
                Ok(_) => continue,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(8));
                }
                Err(_) => return None,
            }
        }
        None
    }

    fn parse_reply1(&mut self, payload: &[u8]) -> Option<(u64, u16, bool)> {
        if payload.first() != Some(&ID_OPEN_CONNECTION_REPLY_1) {
            return None;
        }
        let mut r = Reader::new(payload);
        let _id = r.u8()?;
        let _magic = r.bytes(16)?;
        let server_guid = r.u64()?;
        let use_security = r.u8()? != 0;
        if use_security {
            let _cookie = r.u32()?;
        }
        let mtu = r.u16()?;
        Some((server_guid, mtu.max(400), use_security))
    }

    fn parse_reply2(&mut self, payload: &[u8]) -> Option<(u64, u16)> {
        if payload.first() != Some(&ID_OPEN_CONNECTION_REPLY_2) {
            return None;
        }
        let mut r = Reader::new(payload);
        let _id = r.u8()?;
        let _magic = r.bytes(16)?;
        let server_guid = r.u64()?;
        // server address
        let family = r.u8()?;
        let _port = r.u16()?;
        let _ip = r.bytes(if family == 4 { 4 } else { 16 })?;
        let mtu = r.u16()?;
        Some((server_guid, mtu.max(400)))
    }

    fn handle_accepted(&mut self, payload: &[u8]) -> Result<(), RakError> {
        let mut r = Reader::new(payload);
        let _id = r.u8();
        let _client_addr = read_address(&mut r);
        let _system_index = r.u16().unwrap_or(0);
        for _ in 0..10 {
            let _ = read_address(&mut r);
        }
        let _request_time = r.u64().unwrap_or(0);
        let _server_time = r.u64().unwrap_or(0);
        netlog!(net, "CONNECTION_REQUEST_ACCEPTED received");

        // Reply with NEW_INCOMING_CONNECTION (client addr + server addr).
        let mut out = Vec::with_capacity(24);
        out.push(ID_NEW_INCOMING_CONNECTION);
        write_address(&mut out, self.local_addr_for());
        write_address(&mut out, self.server);
        let _ = self.socket.send_to(&out, self.server);
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

        // Split if it does not fit a datagram (leave room for datagram hdr).
        let max_payload = (self.mtu as usize).saturating_sub(4 + 16 + 64);
        if frame.payload.len() > max_payload {
            let chunks = frame.payload.chunks(max_payload).count() as u32;
            let split_id = self.split_id;
            self.split_id = self.split_id.wrapping_add(1);
            for (i, chunk) in frame.payload.chunks(max_payload).enumerate() {
                let f = Frame {
                    reliable: true,
                    sequenced: false,
                    has_split: true,
                    seq,
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
            if frame.reliable {
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
        let _ = self.socket.send_to(&pkt, self.server);
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
            let pkt: Vec<u8> = self.buf[..n].to_vec();
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
                    } else if f.reliable {
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
                    let _ = self.socket.send_to(&out, self.server);
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
        let count = r.u16().unwrap_or(0);
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
        out.extend_from_slice(&(ranges.len() as u16).to_le_bytes());
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
        let _ = self.socket.send_to(&out, self.server);
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
        if (f.seq as i64) < (self.delivered_ordered as i64) {
            // Already delivered (duplicate).
            return None;
        }
        self.ordered_hold.insert(f.seq, f);
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
            let _ = self.socket.send_to(&out, self.server);
            self.flush();
        }
        self.state = RakState::Disconnected;
    }
}

fn frame_encoded_len(f: &Frame) -> usize {
    let mut n = 1;
    if f.reliable {
        n += 3;
    }
    if f.sequenced {
        n += 3;
    }
    if f.has_split {
        n += 4 + 2 + 4;
    }
    n + f.payload.len()
}

fn read_address(r: &mut Reader) -> Option<SocketAddr> {
    let family = r.u8()?;
    if family == 4 {
        let port = r.u16()?;
        let ip = r.bytes(4)?;
        Some(SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])),
            port,
        ))
    } else {
        let port = r.u16()?;
        let ip = r.bytes(16)?;
        let mut o = [0u8; 16];
        o.copy_from_slice(ip);
        Some(SocketAddr::new(std::net::IpAddr::V6(o.into()), port))
    }
}

fn write_address(out: &mut Vec<u8>, addr: SocketAddr) {
    out.push(4);
    out.extend_from_slice(&addr.port().to_le_bytes());
    match addr.ip() {
        std::net::IpAddr::V4(v4) => out.extend_from_slice(&v4.octets()),
        std::net::IpAddr::V6(v6) => out.extend_from_slice(&v6.octets()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_reliable() {
        let f = Frame {
            reliable: true,
            sequenced: false,
            has_split: false,
            seq: 0x123456,
            split_id: 0,
            split_count: 0,
            split_index: 0,
            payload: vec![1, 2, 3, 4, 5],
        };
        let mut b = Vec::new();
        f.encode(&mut b);
        let g = Frame::decode(&b).unwrap();
        assert!(g.reliable && !g.has_split);
        assert_eq!(g.seq, 0x123456);
        assert_eq!(g.payload, vec![1, 2, 3, 4, 5]);
        assert_eq!(b.len(), frame_encoded_len(&g));
    }

    #[test]
    fn frame_roundtrip_split() {
        let f = Frame {
            reliable: true,
            sequenced: false,
            has_split: true,
            seq: 7,
            split_id: 9,
            split_count: 5,
            split_index: 3,
            payload: vec![0xaa; 11],
        };
        let mut b = Vec::new();
        f.encode(&mut b);
        let g = Frame::decode(&b).unwrap();
        assert!(g.has_split);
        assert_eq!(g.split_count, 5);
        assert_eq!(g.split_index, 3);
        assert_eq!(g.payload, vec![0xaa; 11]);
        assert_eq!(b.len(), frame_encoded_len(&g));
    }

    #[test]
    fn frame_unreliable_roundtrip() {
        let f = Frame::unreliable(vec![9, 8, 7]);
        let mut b = Vec::new();
        f.encode(&mut b);
        let g = Frame::decode(&b).unwrap();
        assert!(!g.reliable);
        assert_eq!(g.seq, 0);
        assert_eq!(g.payload, vec![9, 8, 7]);
    }

    #[test]
    fn ack_encoding() {
        // Mirror what send_acks produces for a range.
        let mut out = vec![DATAGRAM_VALID | DATAGRAM_IS_ACK];
        out.extend_from_slice(&2u16.to_le_bytes());
        out.push(0);
        put_u24(&mut out, 3);
        put_u24(&mut out, 6);
        out.push(1);
        put_u24(&mut out, 10);
        let mut r = Reader::new(&out);
        assert_eq!(r.u8().unwrap() & DATAGRAM_IS_ACK, DATAGRAM_IS_ACK);
        assert_eq!(r.u16().unwrap(), 2);
        assert_eq!(r.u8().unwrap(), 0);
        assert_eq!(r.u24().unwrap(), 3);
        assert_eq!(r.u24().unwrap(), 6);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u24().unwrap(), 10);
    }

    #[test]
    fn two_clients_loopback() {
        // Spin a fake RakNet server on localhost and run the full handshake,
        // then exchange a reliable frame both ways.
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(false).unwrap();
        let sport = server.local_addr().unwrap().port();
        let _ = sport;

        let server_thread = std::thread::spawn(move || {
            let mut buf = vec![0u8; 2048];
            // REQUEST_1
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], ID_OPEN_CONNECTION_REQUEST_1);
            assert_eq!(&buf[1..17], &OFFLINE_MAGIC);
            server
                .send_to(
                    &[
                        &[ID_OPEN_CONNECTION_REPLY_1],
                        &OFFLINE_MAGIC[..],
                        &1234567u64.to_le_bytes()[..],
                        &[0],
                        &1400u16.to_le_bytes()[..],
                    ]
                    .concat(),
                    peer,
                )
                .unwrap();
            // REQUEST_2
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], ID_OPEN_CONNECTION_REQUEST_2);
            let _ = n;
            server
                .send_to(
                    &[
                        &[ID_OPEN_CONNECTION_REPLY_2],
                        &OFFLINE_MAGIC[..],
                        &1234567u64.to_le_bytes()[..],
                        &[4],
                        &0u16.to_le_bytes()[..],
                        &[127, 0, 0, 1],
                        &1400u16.to_le_bytes()[..],
                    ]
                    .concat(),
                    peer,
                )
                .unwrap();
            // CONNECTION_REQUEST
            let (n, peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], ID_CONNECTION_REQUEST);
            let _ = n;
            // ACCEPTED: client addr + systemIndex + 10 addrs + 2 times
            let mut acc = vec![ID_CONNECTION_REQUEST_ACCEPTED];
            acc.push(4);
            acc.extend_from_slice(&0u16.to_le_bytes());
            acc.extend_from_slice(&[127, 0, 0, 1]);
            for _ in 0..10 {
                acc.push(4);
                acc.extend_from_slice(&0u16.to_le_bytes());
                acc.extend_from_slice(&[0, 0, 0, 0]);
            }
            acc.extend_from_slice(&0u64.to_le_bytes());
            acc.extend_from_slice(&0u64.to_le_bytes());
            server.send_to(&acc, peer).unwrap();
            // NEW_INCOMING_CONNECTION
            let (n, _peer) = server.recv_from(&mut buf).unwrap();
            assert_eq!(buf[0], ID_NEW_INCOMING_CONNECTION);
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
                    // Reply: our own datagram with a reliable frame.
                    let mut dg = vec![0x84];
                    put_u24(&mut dg, 0);
                    let rf = Frame {
                        reliable: true,
                        sequenced: false,
                        has_split: false,
                        seq: 0,
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
            RakClient::connect("127.0.0.1", sport, &[10, 5], Duration::from_secs(5))
                .expect("handshake failed");
        assert_eq!(client.state, RakState::Connected);
        assert_eq!(client.server_guid, 1234567);

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
