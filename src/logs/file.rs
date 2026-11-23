//! Persistent log file(s): open/rotate at startup, mirror files for Android,
//! append-only writes and the Downloads exports.

use super::{push, text};
use std::path::PathBuf;
use std::sync::Mutex;

/// Persistent log file (opened append-mode by [`init_file`]). Every entry is
/// mirrored here so a crash cannot take the diagnostics with it — the
/// in-memory ring dies with the process, the file does not.
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// Extra append-only mirror files registered with [`add_mirror_file`]. On
/// Android the primary file lives in app-private storage (unreadable
/// without root), so the harness mirrors it into the external files dir
/// where a file manager / USB can reach it.
static LOG_MIRRORS: Mutex<Vec<std::fs::File>> = Mutex::new(Vec::new());

/// Path of the persistent log file (not created yet is fine).
pub fn log_file_path() -> Option<PathBuf> {
	crate::config::app_dir().map(|d| d.join("logs").join("client.log"))
}

/// Open the persistent log file. Rotates `client.log` to `client.prev.log`
/// when the current file grew past 1 MiB, so one buggy session cannot grow
/// it forever. Called once at startup; safe to call again (no-op while the
/// file is already open).
pub fn init_file() {
	let Some(path) = log_file_path() else { return };
	let dir = match path.parent() {
		Some(d) => d,
		None => return,
	};
	let _ = std::fs::create_dir_all(dir);
	if let Ok(meta) = std::fs::metadata(&path) {
		if meta.len() > 1024 * 1024 {
			let _ = std::fs::rename(&path, dir.join("client.prev.log"));
		}
	}
	let file = std::fs::OpenOptions::new().create(true).append(true).open(&path);
	let started = format!(
		"{} [app] --- rustblox session started (pid {}) ---\n",
		super::stamp(),
		std::process::id()
	);
	// All filesystem work above happens BEFORE taking the lock; the marker
	// write below re-locks through file_write, so the guard must be dropped
	// first (std Mutex is not reentrant — re-locking here deadlocked the
	// whole app at startup).
	let mut guard = match LOG_FILE.lock() {
		Ok(g) => g,
		Err(_) => return,
	};
	if guard.is_some() {
		return;
	}
	match file {
		Ok(f) => {
			*guard = Some(f);
			drop(guard);
			file_write(&started);
		}
		Err(e) => {
			eprintln!("[rustblox] log file unavailable ({}): {}", path.display(), e);
		}
	}
}

/// Append raw text to the log file(s). Poison-tolerant and panic-free on
/// purpose: this runs inside the panic hook.
pub(super) fn file_write(text: &str) {
	use std::io::Write;
	if let Ok(mut guard) = LOG_FILE.lock() {
		if let Some(f) = guard.as_mut() {
			let _ = f.write_all(text.as_bytes());
			let _ = f.flush();
		}
	}
	if let Ok(mut mirrors) = LOG_MIRRORS.lock() {
		for f in mirrors.iter_mut() {
			let _ = f.write_all(text.as_bytes());
			let _ = f.flush();
		}
	}
}

/// Register an additional append-only mirror file (e.g. the Android
/// external-files log). Best effort: storage failures are reported to
/// stderr and ignored. Applies the same 1 MiB rotation as [`init_file`].
pub fn add_mirror_file(path: &std::path::Path) {
	if let Some(dir) = path.parent() {
		let _ = std::fs::create_dir_all(dir);
	}
	if let Ok(meta) = std::fs::metadata(path) {
		if meta.len() > 1024 * 1024 {
			let _ = std::fs::rename(path, path.with_extension("prev.log"));
		}
	}
	match std::fs::OpenOptions::new().create(true).append(true).open(path) {
		Ok(f) => {
			if let Ok(mut guard) = LOG_MIRRORS.lock() {
				guard.push(f);
			}
		}
		Err(e) => eprintln!("[rustblox] log mirror unavailable ({}): {}", path.display(), e),
	}
}

/// Seconds since the UNIX epoch — compact unique suffix for export names
/// (no chrono dependency).
pub(super) fn unix_secs() -> u64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0)
}

/// Collect the persistent log files (old + current; falls back to the
/// in-memory ring when the files are unreadable).
pub(super) fn collect_log_text() -> Vec<u8> {
	let Some(path) = log_file_path() else { return text().into_bytes() };
	let Some(dir) = path.parent() else { return text().into_bytes() };
	let mut out = Vec::new();
	for name in ["client.prev.log", "client.log"] {
		if let Ok(data) = std::fs::read_to_string(dir.join(name)) {
			out.extend_from_slice(data.as_bytes());
			if !data.ends_with('\n') {
				out.push(b'\n');
			}
		}
	}
	if out.is_empty() {
		return text().into_bytes();
	}
	out
}

/// Save the whole persistent log into the platform's Downloads location as
/// `rustblox-client-<unix>.log`. Returns the export name on success.
pub fn export_logs() -> Option<String> {
	let name = format!("rustblox-client-{}.log", unix_secs());
	if crate::platform::export_log(&name, &collect_log_text()) {
		push("app", &format!("log exported to Downloads as {name}"));
		Some(name)
	} else {
		push("app", "log export failed (no Downloads backend on this platform)");
		None
	}
}

/// Save the log under a crash-report name (called from
/// [`load_previous_crash`](super::load_previous_crash)); appends the one-shot
/// marker to the log file.
pub(super) fn export_files(name: &str) -> bool {
	let ok = crate::platform::export_log(name, &collect_log_text());
	if ok {
		push("app", &format!("crash report exported to Downloads as {name}"));
	}
	ok
}
