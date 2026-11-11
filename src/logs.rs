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

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

/// Maximum number of stored lines.
pub const CAP: usize = 400;
/// Hard cap per line — raw bodies are truncated before pushing, this is the
/// final safety net so one giant line cannot blow up the UI layout.
pub const MAX_LINE_CHARS: usize = 240;

static LOGS: Mutex<Option<VecDeque<String>>> = Mutex::new(None);
/// Persistent log file (opened append-mode by [`init_file`]). Every entry is
/// mirrored here so a crash cannot take the diagnostics with it — the
/// in-memory ring dies with the process, the file does not.
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// Extra append-only mirror files registered with [`add_mirror_file`]. On
/// Android the primary file lives in app-private storage (unreadable
/// without root), so the harness mirrors it into the external files dir
/// where a file manager / USB can reach it.
static LOG_MIRRORS: Mutex<Vec<std::fs::File>> = Mutex::new(Vec::new());

/// UTC time-of-day as `[HH:MM:SS]` (no chrono dependency; the date is not
/// needed for live diagnostics and keeps lines short).
fn stamp() -> String {
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
fn clamp_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let mut s: String = line.chars().take(MAX_LINE_CHARS).collect();
    s.push('…');
    s
}

/// Core push: split `line` on newlines, clamp each part, stamp and append
/// to `buf`, evicting the oldest entries past `CAP`.
fn push_into(buf: &mut VecDeque<String>, stamp: &str, kind: &str, line: &str) {
    for part in line.lines() {
        let part = part.trim_end();
        if part.is_empty() {
            continue;
        }
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
    for part in line.lines() {
        let part = part.trim_end();
        if part.is_empty() {
            continue;
        }
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
    for part in line.lines() {
        let part = part.trim_end();
        if part.is_empty() {
            continue;
        }
        file_write(&format!("{} [{}] {}\n", stamped, kind, part));
    }
}

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
        stamp(),
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
fn file_write(text: &str) {
    use std::io::Write;
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(f) = guard.as_mut() {
            let _ = f.write_all(text.as_bytes());
            let _ = f.flush();
        }
    }
    if let Ok(mut mirrors) = LOG_MIRRORS.lock() {
        for f in mirrors.iter_mut() {
            use std::io::Write;
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
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Collect the persistent log files (old + current; falls back to the
/// in-memory ring when the files are unreadable).
fn collect_log_text() -> Vec<u8> {
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
/// [`load_previous_crash`]); appends the one-shot marker to the log file.
fn export_files(name: &str) -> bool {
    let ok = crate::platform::export_log(name, &collect_log_text());
    if ok {
        push("app", &format!("crash report exported to Downloads as {name}"));
    }
    ok
}
/// Surface panics from earlier sessions: scan the persistent log files and
/// re-push every `[panic]` entry into the ring buffer (buffer only — they
/// are not echoed back into the file), so a crash is visible and copyable
/// in Settings → Logs on the next launch even though the old process is
/// gone. Called once at startup, right after [`init_file`].
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
    let buf = guard.get_or_insert_with(|| VecDeque::with_capacity(CAP));
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
mod tests {
    use super::*;

    #[test]
    fn push_into_split_clamp_and_cap() {
        let mut buf = VecDeque::new();
        push_into(&mut buf, "[00:00:00]", "test", "hello\nworld\n\n");
        assert_eq!(buf.len(), 2);
        assert!(buf[0].contains("[test] hello"), "{buf:?}");
        assert!(buf[1].contains("world"), "{buf:?}");

        // Overlong lines are clamped with an ellipsis (the "[stamp] [kind] "
        // prefix adds up to ~20 chars on top of the clamped body).
        push_into(&mut buf, "[00:00:01]", "test", &"x".repeat(2000));
        let last = buf.back().unwrap();
        assert!(last.chars().count() <= MAX_LINE_CHARS + 20, "{}", last.chars().count());
        assert!(last.ends_with('…'), "{last:?}");

        // Ring buffer capacity: oldest entries are evicted.
        for i in 0..(CAP + 50) {
            push_into(&mut buf, "[00:00:02]", "test", &format!("line {i}"));
        }
        assert_eq!(buf.len(), CAP);
        assert!(buf.back().unwrap().contains(&format!("line {}", CAP + 49)));
        // 453 entries in total: the first 3 (hello/world/xxx) and lines
        // 0..49 were evicted, so "line 50" now fronts the buffer.
        assert!(buf.front().unwrap().contains("line 50"), "{:?}", buf.front());
    }

    #[test]
    fn stamp_format() {
        let s = stamp();
        assert!(s.starts_with('[') && s.ends_with(']'), "{s}");
        assert_eq!(s.len(), 10);
        for (i, c) in s.chars().enumerate() {
            match i {
                3 | 6 => assert_eq!(c, ':', "{s}"),
                1 | 2 | 4 | 5 | 7 | 8 => assert!(c.is_ascii_digit(), "{s}"),
                _ => {}
            }
        }
    }
}
