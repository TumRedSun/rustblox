#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::super::{put_u24, Reader};
use super::{DATAGRAM_IS_ACK, DATAGRAM_VALID};

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
