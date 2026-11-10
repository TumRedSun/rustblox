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
use std::sync::Mutex;

/// Maximum number of stored lines.
pub const CAP: usize = 400;
/// Hard cap per line — raw bodies are truncated before pushing, this is the
/// final safety net so one giant line cannot blow up the UI layout.
pub const MAX_LINE_CHARS: usize = 240;

static LOGS: Mutex<Option<VecDeque<String>>> = Mutex::new(None);

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
    push_into(buf, &stamp(), kind, line);
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
