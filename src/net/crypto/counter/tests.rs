//! Tests transcribed from the Nov-12 live capture (handshake_full.txt).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::super::*;
use super::nonce_bytes;

#[test]
fn counter_framing_overhead_is_exactly_18_bytes() {
	// The dump's pkt4: envelope(31) + box(52) → the counter box adds
	// exactly ct..hint(2)+tag(16) = 18 bytes over the plaintext.
	let mut ec = EarlyCrypto::from_join(
		"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
		Vec::new(),
		OCR2_AUTH_VER,
	)
	.unwrap();
	// A loopback peer shares the SAME key in both directions (the wire
	// semantics are tx on one side, rx on the other).
	ec.session_tx = Some(ec.tx);
	ec.session_rx = Some(ec.tx);
	let pt = vec![9u8; CONNECTION_REQUEST_PT];
	let boxed = ec.seal_counter(&pt);
	assert_eq!(boxed.len(), CONNECTION_REQUEST_PT + 18);
	// The hint = the low 16 bits of the counter (base 0 → 0).
	assert_eq!(&boxed[pt.len()..pt.len() + 2], &[0, 0]);
	// Round-trips through the scanning opener.
	let (back, counter) = ec.open_counter(&boxed).expect("loopback must open");
	assert_eq!(back, pt);
	assert_eq!(counter, 0);
	// The second datagram bumps the counter; the hint follows.
	let boxed2 = ec.seal_counter(&pt);
	assert_eq!(&boxed2[pt.len()..pt.len() + 2], &[1, 0]);
	assert!(ec.open_counter(&boxed2).is_some());
	// The two directions really use different keys (client tx != rx).
	assert_ne!(ec.tx, ec.rx);
}

#[test]
fn counter_nonce_uses_the_mber_suffix() {
	// nonce = u64 LE counter || "mber" — verified against the live
	// engine's own nonce-builder code (Ghidra decompile, Nov 2026): the
	// immediate `0x7265626d` is the ASCII bytes "mber" little-endian,
	// lowercase r (not "mbeR" as the pcap-only read once assumed).
	let n = nonce_bytes(1);
	assert_eq!(&n[..8], &1u64.to_le_bytes());
	assert_eq!(&n[8..], b"mber");
}

#[test]
fn counter_base_env_and_hint_scanning() {
	// The dump session's first post-OR2 counter is 0x6e55 — a nonzero
	// negotiated base. With a nonzero base the hint carries the low 16
	// bits and the opener reconstructs the value from base + hint.
	let mut ec = EarlyCrypto::from_join(
		"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
		Vec::new(),
		OCR2_AUTH_VER,
	)
	.unwrap();
	ec.nonce_base = 0x6e55;
	ec.session_tx = Some(ec.tx);
	ec.session_rx = Some(ec.tx);
	let pt = vec![7u8; 20];
	let boxed = ec.seal_counter(&pt);
	let hint = u16::from_le_bytes([boxed[20], boxed[21]]);
	assert_eq!(hint, 0x6e55);
	let (back, counter) = ec.open_counter(&boxed).expect("base 0x6e55 must open");
	assert_eq!(back, pt);
	assert_eq!(counter, 0x6e55);
}
