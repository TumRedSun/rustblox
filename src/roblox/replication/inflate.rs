//! Payload decompression: zlib/deflate for the legacy stream, zstd for the
//! fork's big bodies.

/// zstd frame magic (`28 b5 2f fd`).
pub(super) const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub(super) fn inflate(data: &[u8]) -> Option<Vec<u8>> {
	use std::io::Read;
	let mut out = Vec::new();
	match flate2::read::ZlibDecoder::new(data).take(64 * 1024 * 1024).read_to_end(&mut out) {
		Ok(_) => Some(out),
		Err(_) => {
			// Try raw deflate as a fallback.
			let mut out2 = Vec::new();
			match flate2::read::DeflateDecoder::new(data)
				.take(64 * 1024 * 1024)
				.read_to_end(&mut out2)
			{
				Ok(_) => Some(out2),
				Err(_) => None,
			}
		}
	}
}

/// Inflate one zstd frame (`28 b5 2f fd …`). ruzstd stops at the frame end;
/// the output is capped so a hostile length cannot exhaust memory.
pub(super) fn inflate_zstd(data: &[u8]) -> Option<Vec<u8>> {
	use std::io::Read;
	let dec = ruzstd::streaming_decoder::StreamingDecoder::new(data).ok()?;
	let mut out = Vec::new();
	dec.take(32 * 1024 * 1024).read_to_end(&mut out).ok()?;
	if out.is_empty() {
		None
	} else {
		Some(out)
	}
}
