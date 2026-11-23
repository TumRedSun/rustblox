//! Phases 1-2 of the handshake: the OCR1 and OCR2 request/reply matrices
//! (fork ids first, vanilla ids as the direct-dial fallback) and the crypto
//! OCR2 path.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{
	build_ocr2, hex_head, RBX_CAPABILITY_MODERN, RBX_CAPABILITY_ROBLOX,
	RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_ID_OPEN_CONNECTION_REQUEST_2, RBX_OCR1_EPOCH,
	RBX_OFFLINE_PROBE, RBX_PROTOCOL_VERSION,
};
use crate::net::raknet::{
	ForkGen, RakClient, RakError, ID_OPEN_CONNECTION_REQUEST_1, ID_OPEN_CONNECTION_REQUEST_2,
	OFFLINE_MAGIC,
};
use crate::net::udmux::{self, Envelope, BARE};
use crate::netlog;
use std::net::Ipv4Addr;
use std::time::Duration;

/// One OCR1 matrix step: envelope, offline id, protocol byte.
type P1Variant = (Envelope, u8, u8);
/// One OCR2 matrix step: offline id, SupportedVersion, Capabilities.
type P2Variant = (u8, u32, u64);

/// Phase 1: OPEN_CONNECTION_REQUEST_1 -> REPLY_1 (MTU probing).
///
/// Matrix = envelope x offline id x protocol byte, fork shapes first
/// (0x7B + proto 5 — what the modern proxy actually answers), vanilla ids as
/// the direct-dial fallback.
pub(super) fn phase1(
	c: &mut RakClient,
	pinned: &Option<Envelope>,
	probe_pong: bool,
	protocol_versions: &[u8],
	udmux_token: Option<(&str, u64)>,
	timeout: Duration,
) -> Result<(u64, u16, bool), RakError> {
	let variants = phase1_variants(pinned, c.client_tag, protocol_versions, udmux_token);
	let reply = c.offline_roundtrip(
		&variants,
		timeout,
		"request-1",
		|(env, id, proto), out, mtu| {
			out.push(*id);
			out.extend_from_slice(&OFFLINE_MAGIC);
			out.push(*proto);
			// Live capture, byte-exact: the fork OCR1 carries one more byte
			// after the protocol byte (`05 01` on the wire, then the zero
			// pad). Without it the RCC ignores the request.
			if *id == RBX_ID_OPEN_CONNECTION_REQUEST_1 {
				out.push(RBX_OCR1_EPOCH);
			}
			// MTU probe: pad to the datagram size (envelope header included
			// — the proxy judges the path MTU by the whole datagram, and
			// send_raw adds the header on top).
			out.resize((mtu as usize).saturating_sub(env.direction as usize), 0x00);
		},
		|(env, _, _)| *env,
		|c, payload| c.parse_reply1(payload),
	);
	match reply {
		Ok((reply, winner)) => Ok(apply_reply1(c, &variants[winner], reply)),
		Err(e) => {
			diagnose_path(pinned, probe_pong);
			Err(e)
		}
	}
}

/// Adopt the winning OCR1 shape for the rest of the session.
fn apply_reply1(
	c: &mut RakClient,
	variant: &P1Variant,
	reply: (u64, u16, bool),
) -> (u64, u16, bool) {
	let (env, id, proto) = *variant;
	c.envelope = if env.direction == 0 { None } else { Some(env) };
	c.fork =
		if id == RBX_ID_OPEN_CONNECTION_REQUEST_1 { ForkGen::Roblox } else { ForkGen::Vanilla };
	netlog!(
		net,
		"handshake envelope '{}' (id 0x{:02x}, protocol {}, token type 0x{:02x}, tag {}) \
         confirmed — applying it to every datagram this session",
		env.label,
		id,
		proto,
		env.cls,
		hex_head(&env.epoch, 16)
	);
	reply
}

/// Classify a silent OCR1 phase from the evidence collected so far.
fn diagnose_path(pinned: &Option<Envelope>, probe_pong: bool) {
	match (pinned, probe_pong) {
		(Some(env), true) => netlog!(
			net,
			"path diagnosis: envelope '{}' got a probe pong but no OCR1 reply — \
             the proxy routes this shape, the RCC rejects the handshake \
             (protocol byte, MTU or required encryption)",
			env.label
		),
		(Some(_), false) => netlog!(
			net,
			"path diagnosis: no reply to any OCR1 shape and no ping probe ran — \
             the proxy dropped every datagram (TokenTlv gate) or the path is dead"
		),
		(None, _) => netlog!(
			net,
			"path diagnosis: no envelope shape answered pings or OCR1 — the proxy \
             drops unauthenticated datagrams (stale session port, firewall, or \
             a routing shape we did not try)"
		),
	}
}

/// Build the OCR1 matrix: pinned envelope shapes first, bare shapes when no
/// envelope was confirmed.
fn phase1_variants(
	pinned: &Option<Envelope>,
	client_tag: [u8; 16],
	protocol_versions: &[u8],
	udmux_token: Option<(&str, u64)>,
) -> Vec<P1Variant> {
	match pinned {
		Some(env) => pinned_variants(*env, client_tag, protocol_versions),
		None => bare_variants(protocol_versions, udmux_token),
	}
}

/// OCR1 steps for a pinned envelope: token-type/tag variants first, then the
/// protocol variants on the leading shape, then the vanilla id fallback.
fn pinned_variants(
	env: Envelope,
	client_tag: [u8; 16],
	protocol_versions: &[u8],
) -> Vec<P1Variant> {
	let with_cls = |cls: u8| {
		let mut e = env;
		e.cls = cls;
		e
	};
	let lead = with_cls(0x02);
	let mut out = vec![
		(lead, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION),
		(with_cls(0x01), RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION),
	];
	if env.epoch != client_tag {
		let mut rnd02 = env;
		rnd02.cls = 0x02;
		rnd02.epoch = client_tag;
		let mut rnd01 = rnd02;
		rnd01.cls = 0x01;
		out.push((rnd02, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
		out.push((rnd01, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
	}
	push_extra_protocols(&mut out, lead, protocol_versions);
	out.push((lead, ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
	out
}

/// OCR1 steps without a confirmed envelope: bare fork shape, bare vanilla,
/// the protocol variants, then the remaining envelope candidates.
fn bare_variants(protocol_versions: &[u8], udmux_token: Option<(&str, u64)>) -> Vec<P1Variant> {
	let mut out = vec![
		(udmux::BARE, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION),
		(udmux::BARE, ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION),
	];
	push_extra_protocols(&mut out, udmux::BARE, protocol_versions);
	if let Some((tok, pepper)) = udmux_token {
		for env in udmux::envelope_candidates(tok, pepper).into_iter().skip(1) {
			out.push((env, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
		}
	}
	out
}

/// Append the caller-supplied protocol versions (the fork's own 5 excluded —
/// it is already in the matrix).
fn push_extra_protocols(out: &mut Vec<P1Variant>, env: Envelope, protocol_versions: &[u8]) {
	for &pr in protocol_versions {
		if pr != RBX_PROTOCOL_VERSION {
			out.push((env, RBX_ID_OPEN_CONNECTION_REQUEST_1, pr));
		}
	}
}

/// Phase 2: OPEN_CONNECTION_REQUEST_2 -> REPLY_2, crypto shape first (when
/// early crypto material exists), then the sala matrix.
pub(super) fn phase2(
	c: &mut RakClient,
	rcc: Option<(Ipv4Addr, u16)>,
	port: u16,
	timeout: Duration,
) -> Result<(), RakError> {
	let pinned_env = c.envelope;
	let rcc_port = rcc.map(|(_, p)| p).unwrap_or(port);
	let crypto_reply2 = c.early.is_some() && crypto_ocr2(c, pinned_env, rcc_port, timeout);
	let reply2 =
		if crypto_reply2 { (c.server_guid, c.mtu) } else { sala_ocr2(c, pinned_env, timeout)? };
	netlog!(net, "REPLY_2: guid={} mtu={}", reply2.0, reply2.1);
	c.server_guid = reply2.0;
	Ok(())
}

/// Fork crypto OCR2 (`78 | magic | ... | ct(93) | "UniqueNumber" | tag(16)`):
/// live-verified 2026-11-17 with probe7.py. Returns whether the server
/// accepted it; on any failure the caller falls back to the sala matrix.
fn crypto_ocr2(
	c: &mut RakClient,
	pinned_env: Option<Envelope>,
	rcc_port: u16,
	timeout: Duration,
) -> bool {
	let early_body =
		c.early.as_mut().and_then(|ec| ec.build_ocr2(c.mtu, rcc_port).map(|(_, body)| body));
	let Some(mut body) = early_body else {
		netlog!(
			net,
			"crypto OCR2 unavailable: ClientTicket paBlob missing/short ({} bytes) — \
             the server verifies it against the join call; using the sala shapes",
			c.early.as_ref().map(|ec| ec.pa_blob.len()).unwrap_or(0)
		);
		return false;
	};
	let hlen = pinned_env.map(|e| e.direction as usize).unwrap_or(0);
	body.resize(RBX_OFFLINE_PROBE.saturating_sub(hlen), 0x00);
	let res = c.offline_roundtrip(
		&[body],
		timeout,
		"request-2-crypto",
		|b, out, _mtu| {
			out.clear();
			out.extend_from_slice(b);
			// Keep the capture's total: content ends at 242 bytes, the rest
			// is the zero pad to 1160.
			out.resize(RBX_OFFLINE_PROBE.saturating_sub(hlen), 0x00);
		},
		move |_| pinned_env.unwrap_or(BARE),
		|c, payload| c.parse_reply2_crypto(payload),
	);
	match res {
		Ok((_, _)) => {
			netlog!(
				net,
				"crypto OpenReply2 accepted — session keys adopted, skipping the sala matrix"
			);
			true
		}
		Err(e) => {
			netlog!(net, "crypto OCR2 got no reply ({}); falling back to the sala shapes", e);
			false
		}
	}
}

/// The sala OCR2 matrix (fork tail fields + vanilla fallback).
fn sala_ocr2(
	c: &mut RakClient,
	pinned_env: Option<Envelope>,
	timeout: Duration,
) -> Result<(u64, u16), RakError> {
	let variants = phase2_variants();
	let addr = c.server;
	let guid = c.guid;
	let hlen = pinned_env.map(|e| e.direction as usize).unwrap_or(0);
	let (reply, _) = c.offline_roundtrip(
		&variants,
		timeout,
		"request-2",
		|(id, sv, caps), out, mtu| {
			*out = build_ocr2(*id, *sv, *caps, addr, mtu, guid);
			// The capture pads OCR2 to the same 1160-byte datagram as OCR1
			// (real content ends ~243 bytes in, the rest is zero pad) — the
			// negotiated MTU only governs the connected phase, not the
			// offline probe size.
			out.resize(RBX_OFFLINE_PROBE.saturating_sub(hlen), 0x00);
		},
		move |_| pinned_env.unwrap_or(BARE),
		|c, payload| c.parse_reply2(payload),
	)?;
	Ok(reply)
}

/// The fork's legacy OCR2 has no security cookie and appends SupportedVersion
/// (u32 BE, matrix 5/1/0) and Capabilities (u64 BE, base vs modern superset).
fn phase2_variants() -> Vec<P2Variant> {
	let mut out = Vec::new();
	for &sv in &[5u32, 1, 0] {
		out.push((RBX_ID_OPEN_CONNECTION_REQUEST_2, sv, RBX_CAPABILITY_MODERN));
		out.push((RBX_ID_OPEN_CONNECTION_REQUEST_2, sv, RBX_CAPABILITY_ROBLOX));
	}
	// Vanilla fallback (direct dials): the legacy OCR2 shape.
	out.push((ID_OPEN_CONNECTION_REQUEST_2, 0, 0));
	out
}
