//! Tests transcribed from the Nov-12 live capture and re-verified live on
//! 24 Nov (the server accepts the box; its own steady frames open).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::super::*;
use super::{nonce_bytes, STEADY_BASE};

fn sample_crypto() -> EarlyCrypto {
	EarlyCrypto::from_join(
		"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
		Vec::new(),
		OCR2_AUTH_VER,
	)
	.unwrap()
}

#[test]
fn counter_framing_overhead_is_exactly_18_bytes() {
	// envelope(31) + box: the counter box adds ct..hint(2)+tag(16) = 18
	// bytes over the plaintext.
	let mut ec = sample_crypto();
	// A loopback peer shares the SAME key in both directions.
	ec.session_tx = Some(ec.tx);
	ec.session_rx = Some(ec.tx);
	let pt = vec![9u8; CONNECTION_REQUEST_PT];
	let boxed = ec.seal_counter(&pt);
	assert_eq!(boxed.len(), CONNECTION_REQUEST_PT + 18);
	// The hint = the low 16 bits of the steady base (0x6e55 → 55 6e LE),
	// exactly the capture's first post-ORP2 hint bytes.
	assert_eq!(&boxed[pt.len()..pt.len() + 2], &[0x55, 0x6e]);
	let (back, counter) = ec.open_counter(&boxed).expect("loopback must open");
	assert_eq!(back, pt);
	assert_eq!(counter, STEADY_BASE);
	// The second datagram bumps the counter; the hint follows.
	let boxed2 = ec.seal_counter(&pt);
	assert_eq!(&boxed2[pt.len()..pt.len() + 2], &[0x56, 0x6e]);
	assert!(ec.open_counter(&boxed2).is_some());
	assert_ne!(ec.tx, ec.rx);
}

#[test]
fn counter_nonce_spells_unique_number() {
	// nonce = LE64(base + counter) || "mbeR": for the base this reads
	// ASCII "UniqueNumbeR" (the engine's own nonce builder writes
	// `0x754E657571696E55` and the `0x52...` suffix).
	let n = nonce_bytes(STEADY_BASE);
	assert_eq!(&n[..8], b"UniqueNu");
	assert_eq!(&n[8..], b"mbeR");
	// One step increments the LOW byte of the LE u64, i.e. the first nonce
	// byte ('U' 0x55 -> 'V' 0x56); the ASCII tail stays put.
	let n1 = nonce_bytes(STEADY_BASE + 1);
	assert_eq!(&n1[..8], &[0x56, 0x6e, 0x69, 0x71, 0x75, 0x65, 0x4e, 0x75]);
}

#[test]
fn counter_base_and_hint_scanning() {
	// The hint carries the low 16 bits; the opener reconstructs the value
	// from base + delta and scans the high bits, so a few lost datagrams
	// (gap < 0x10000) still open.
	let mut ec = sample_crypto();
	ec.session_tx = Some(ec.tx);
	ec.session_rx = Some(ec.tx);
	let pt = vec![7u8; 20];
	let boxed = ec.seal_counter(&pt);
	let (back, _) = ec.open_counter(&boxed).expect("must open");
	assert_eq!(back, pt);
}
