//! Crash diagnostics: the panic hook and recovery of panics from a previous
//! session.

use super::file::{export_files, file_write, unix_secs};
use super::{clamp_line, lines, log_file_path, push, push_into, stamp, CAP, LOGS};

/// Surface panics from earlier sessions: scan the persistent log files and
/// re-push every `[panic]` entry into the ring buffer (buffer only — they
/// are not echoed back into the file), so a crash is visible and copyable
/// in Settings → Logs on the next launch even though the old process is
/// gone. Called once at startup, right after [`init_file`](super::init_file).
///
/// Additionally, panics not yet reported are exported to the platform's
/// user-visible Downloads location as `rustblox-crash-<unix>.log` — the
/// no-root, no-PC way to get a full crash report off a phone (the internal
/// log is unreadable for the user, and Android ≥ 11 hides Android/data).
/// A `[app] crash report exported …` marker line in the file keeps the
/// export one-shot per panic.
pub fn load_previous_crash() {
	let Some(path) = log_file_path() else { return };
	let Some(dir) = path.parent() else { return };
	let mut crashes: Vec<String> = Vec::new();
	let mut unexported = 0usize;
	for name in ["client.prev.log", "client.log"] {
		let Ok(data) = std::fs::read_to_string(dir.join(name)) else { continue };
		for line in data.lines() {
			if line.contains(" [panic] ") {
				crashes.push(line.to_string());
				unexported += 1;
			} else if line.contains("crash report exported") {
				// Everything before the last export marker is already off
				// the device.
				unexported = 0;
			}
		}
	}
	if unexported > 0 {
		let name = format!("rustblox-crash-{}.log", unix_secs());
		let exported = export_files(&name);
		push(
			"app",
			&if exported {
				format!(
					"crash report exported to Downloads as {name} ({} new panic line(s))",
					unexported
				)
			} else {
				"crash report export unavailable (no Downloads backend) — use Copy logs".to_string()
			},
		);
	}
	if crashes.is_empty() {
		return;
	}
	let total = crashes.len();
	let Ok(mut guard) = LOGS.lock() else { return };
	let buf = guard.get_or_insert_with(|| std::collections::VecDeque::with_capacity(CAP));
	push_into(
		buf,
		&stamp(),
		"app",
		&format!(
			"previous session crashed! {total} panic line(s) recovered from {}",
			path.display()
		),
	);
	// Newest five panics, chronological order.
	for line in crashes.iter().rev().take(5).rev() {
		if buf.len() >= CAP {
			buf.pop_front();
		}
		buf.push_back(clamp_line(line));
	}
}

/// Install the panic hook: every panic is stamped with thread, message and
/// location into the persistent log file (plus the last buffered lines for
/// context) BEFORE the default hook prints to stderr. Without this a crash
/// took the whole diagnostic ring buffer with it — exactly the un-reportable
/// "client just crashed" case.
///
/// Idempotent: platform entry points call it as early as possible, then
/// [`run_with`](crate::run_with) calls it again — the second call is a no-op
/// (otherwise wrapping hooks would double-write every panic).
pub fn install_panic_hook() {
	use std::sync::atomic::{AtomicBool, Ordering};
	static INSTALLED: AtomicBool = AtomicBool::new(false);
	if INSTALLED.swap(true, Ordering::SeqCst) {
		return;
	}
	let default = std::panic::take_hook();
	std::panic::set_hook(Box::new(move |info| {
		let thread = std::thread::current();
		let name = thread.name().unwrap_or("<unnamed>").to_string();
		let loc = info
			.location()
			.map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
			.unwrap_or_else(|| "<unknown>".to_string());
		let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
			(*s).to_string()
		} else if let Some(s) = info.payload().downcast_ref::<String>() {
			s.clone()
		} else {
			"<non-string panic payload>".to_string()
		};
		file_write(&format!(
			"{} [panic] thread '{}' panicked at {}: {}\n",
			stamp(),
			name,
			loc,
			msg
		));
		// Context: the last lines we buffered (direct file write — the LOGS
		// mutex could be the one the panicking thread held).
		for line in lines().iter().rev().take(40).rev() {
			file_write(&format!("{}\n", line));
		}
		default(info);
	}));
}
