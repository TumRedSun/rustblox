//! RakNet wire encode/decode: the frame layout inside a datagram, the byte
//! reader used by every parser in this subsystem, and the two address-block
//! shapes (Roblox fork XOR-obfuscated vs vanilla RakNet).
//!
//! The reliable/ordered/split bookkeeping that uses these primitives lives
//! in [`reliability`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod reliability;

use std::net::SocketAddr;

/// Write a 24-bit little-endian value.
pub(super) fn put_u24(buf: &mut Vec<u8>, v: u32) {
	buf.push((v & 0xff) as u8);
	buf.push(((v >> 8) & 0xff) as u8);
	buf.push(((v >> 16) & 0xff) as u8);
}

/// A small cursor over a byte slice — every handshake and connected-phase
/// parser in this subsystem reads through one of these.
pub struct Reader<'a> {
	buf: &'a [u8],
	pos: usize,
}

impl<'a> Reader<'a> {
	pub fn new(buf: &'a [u8]) -> Self {
		Reader { buf, pos: 0 }
	}
	pub fn u8(&mut self) -> Option<u8> {
		let v = *self.buf.get(self.pos)?;
		self.pos += 1;
		Some(v)
	}
	pub fn u16(&mut self) -> Option<u16> {
		let v = self.buf.get(self.pos..self.pos + 2)?;
		self.pos += 2;
		Some(u16::from_le_bytes([v[0], v[1]]))
	}
	pub fn u16be(&mut self) -> Option<u16> {
		let v = self.buf.get(self.pos..self.pos + 2)?;
		self.pos += 2;
		Some(u16::from_be_bytes([v[0], v[1]]))
	}
	pub fn u32be(&mut self) -> Option<u32> {
		let v = self.buf.get(self.pos..self.pos + 4)?;
		self.pos += 4;
		Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
	}
	pub fn u64be(&mut self) -> Option<u64> {
		let v = self.buf.get(self.pos..self.pos + 8)?;
		self.pos += 8;
		Some(u64::from_be_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]]))
	}
	pub fn u24(&mut self) -> Option<u32> {
		let v = self.buf.get(self.pos..self.pos + 3)?;
		self.pos += 3;
		Some(v[0] as u32 | (v[1] as u32) << 8 | (v[2] as u32) << 16)
	}
	pub fn u64(&mut self) -> Option<u64> {
		let v = self.buf.get(self.pos..self.pos + 8)?;
		self.pos += 8;
		Some(u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]]))
	}
	pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
		let v = self.buf.get(self.pos..self.pos + n)?;
		self.pos += n;
		Some(v)
	}
	pub fn rest(&self) -> &'a [u8] {
		&self.buf[self.pos.min(self.buf.len())..]
	}
}

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

/// Resend bookkeeping for one outgoing reliable frame.
pub(super) struct OutFrame {
	#[allow(dead_code)]
	pub(super) reliability_index: u32,
	pub(super) bytes: Vec<u8>, // encoded frame (header + payload)
	pub(super) sent: std::time::Instant,
	pub(super) tries: u32,
}

pub(super) fn frame_encoded_len(f: &Frame) -> usize {
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

/// Roblox fork address block (Sala ExtendedReader::readAddress):
/// `family(0x04) | every IP byte XOR 0xFF | port u16 BE` — IP first, then
/// the port. The old encoder wrote the vanilla order (port first) without
/// the XOR, so every address the RCC parsed came out as 0.0.0.0.
pub(super) fn read_address_rbx(r: &mut Reader) -> Option<SocketAddr> {
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

pub(super) fn write_address_rbx(out: &mut Vec<u8>, addr: SocketAddr) {
	out.push(4);
	match addr.ip() {
		std::net::IpAddr::V4(v4) => out.extend(v4.octets().map(|b| b ^ 0xFF)),
		std::net::IpAddr::V6(v6) => out.extend(v6.octets().map(|b| b ^ 0xFF)),
	}
	out.extend_from_slice(&addr.port().to_be_bytes());
}

/// Legacy vanilla address block (direct-dial fallback): family | port BE |
/// raw IP — the shape the old parser spoke.
pub(super) fn read_address(r: &mut Reader) -> Option<SocketAddr> {
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
		Some(SocketAddr::new(std::net::IpAddr::V6(o.into()), u16::from_be(port)))
	}
}

pub(super) fn write_address(out: &mut Vec<u8>, addr: SocketAddr) {
	out.push(4);
	out.extend_from_slice(&addr.port().to_be_bytes());
	match addr.ip() {
		std::net::IpAddr::V4(v4) => out.extend_from_slice(&v4.octets()),
		std::net::IpAddr::V6(v6) => out.extend_from_slice(&v6.octets()),
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
