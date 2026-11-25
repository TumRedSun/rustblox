//! In-memory diagnostic log ring buffer shared by every thread.
//!
//! The network worker, game session and UI all append here (via the
//! [`crate::netlog!`] macro or direct [`push`] calls). Settings → Logs
//! renders the buffer and the "Copy logs" button hands the whole text to
//! the system clipboard, so a failed join can be reported with full
//! context even on devices without stderr access.
//!
//! The buffer is always on: entries are cheap and capped, and stderr
//! echoing stays gated behind RUSTBLOX_DEBUG.
//!
//! Layout: the ring buffer + formatting here, persistent files and exports
//! in `file.rs`, the panic hook and crash recovery in `crash.rs`.

mod crash;
mod file;

pub use crash::{install_panic_hook, load_previous_crash};
pub use file::{add_mirror_file, export_logs, init_file, log_file_path};

use self::file::file_write;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;

/// Maximum number of stored lines.
pub const CAP: usize = 400;
/// Hard cap per line — raw bodies are truncated before pushing, this is the
/// final safety net so one giant line cannot blow up the UI layout.
pub const MAX_LINE_CHARS: usize = 240;

pub(super) static LOGS: Mutex<Option<VecDeque<String>>> = Mutex::new(None);

/// UTC time-of-day as `[HH:MM:SS]` (no chrono dependency; the date is not
/// needed for live diagnostics and keeps lines short).
pub(super) fn stamp() -> String {
	let secs = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	let tod = secs % 86_400;
	let mut s = String::with_capacity(10);
	let _ = write!(s, "[{:02}:{:02}:{:02}]", tod / 3600, (tod % 3600) / 60, tod % 60);
	s
}

/// Truncate one line to [`MAX_LINE_CHARS`] with an ellipsis marker.
pub(super) fn clamp_line(line: &str) -> String {
	if line.chars().count() <= MAX_LINE_CHARS {
		return line.to_string();
	}
	let mut s: String = line.chars().take(MAX_LINE_CHARS).collect();
	s.push('…');
	s
}

/// Split `line` on newlines into non-blank parts.
pub(super) fn non_blank_parts(line: &str) -> impl Iterator<Item = &str> {
	line.lines().map(str::trim_end).filter(|part| !part.is_empty())
}

/// Core push: split `line` on newlines, clamp each part, stamp and append
/// to `buf`, evicting the oldest entries past `CAP`.
pub(super) fn push_into(buf: &mut VecDeque<String>, stamp: &str, kind: &str, line: &str) {
	for part in non_blank_parts(line) {
		if buf.len() >= CAP {
			buf.pop_front();
		}
		buf.push_back(format!("{} [{}] {}", stamp, kind, clamp_line(part)));
	}
}

/// Append one entry (multi-line input is split; blank lines are dropped).
pub fn push(kind: &str, line: &str) {
	let Ok(mut guard) = LOGS.lock() else { return };
	let buf = guard.get_or_insert_with(|| VecDeque::with_capacity(CAP));
	let stamped = stamp();
	push_into(buf, &stamped, kind, line);
	// Mirror to the file (best effort — never panics, never blocks the UI
	// for long: the file handle is buffered-free and flushed per line).
	for part in non_blank_parts(line) {
		file_write(&format!("{} [{}] {}\n", stamped, kind, clamp_line(part)));
	}
}

/// Like [`push`], but the FILE copy keeps the full unclamped text while the
/// ring buffer still shows the truncated form. Use for error payloads that
/// wgpu and friends make multi-KB: the UI cannot render them, but they are
/// exactly what the Downloads export must preserve.
pub fn push_unclamped(kind: &str, line: &str) {
	let Ok(mut guard) = LOGS.lock() else { return };
	let buf = guard.get_or_insert_with(|| VecDeque::with_capacity(CAP));
	let stamped = stamp();
	push_into(buf, &stamped, kind, line);
	for part in non_blank_parts(line) {
		file_write(&format!("{} [{}] {}\n", stamped, kind, part));
	}
}

/// Snapshot of the buffer (oldest first).
pub fn lines() -> Vec<String> {
	match LOGS.lock() {
		Ok(guard) => guard.as_ref().map(|b| b.iter().cloned().collect()).unwrap_or_default(),
		Err(_) => Vec::new(),
	}
}

/// The whole buffer joined with newlines (for the clipboard).
pub fn text() -> String {
	let mut out = String::new();
	if let Ok(guard) = LOGS.lock() {
		if let Some(buf) = guard.as_ref() {
			for line in buf.iter() {
				out.push_str(line);
				out.push('\n');
			}
		}
	}
	out
}

/// Number of stored lines.
pub fn len() -> usize {
	LOGS.lock().ok().and_then(|g| g.as_ref().map(|b| b.len())).unwrap_or(0)
}

/// Drop every entry.
pub fn clear() {
	if let Ok(mut guard) = LOGS.lock() {
		if let Some(buf) = guard.as_mut() {
			buf.clear();
		}
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
