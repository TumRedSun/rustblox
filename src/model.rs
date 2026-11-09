//! UI-side domain models and small pure helpers (unit-tested).

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct UserProfile {
    pub id: u64,
    pub name: String,
    pub display_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PresenceKind {
    #[default]
    Offline,
    Online,
    InGame,
    Studio,
    /// "Invisible" presence type returned by the API; shown as Offline.
    Invisible,
}

impl PresenceKind {
    /// Sort priority: in-game first, then online, studio, offline.
    pub fn rank(self) -> u8 {
        match self {
            PresenceKind::InGame => 0,
            PresenceKind::Online => 1,
            PresenceKind::Studio => 2,
            PresenceKind::Offline | PresenceKind::Invisible => 3,
        }
    }

    pub fn from_api(v: u8) -> PresenceKind {
        match v {
            0 => PresenceKind::Offline,
            1 => PresenceKind::Online,
            2 => PresenceKind::InGame,
            3 => PresenceKind::Studio,
            _ => PresenceKind::Invisible,
        }
    }

    pub fn dot_color(self) -> crate::theme::Rgba {
        match self {
            PresenceKind::Online => crate::theme::GREEN,
            PresenceKind::InGame => crate::theme::BLUE,
            PresenceKind::Studio => crate::theme::ORANGE,
            PresenceKind::Offline | PresenceKind::Invisible => crate::theme::GRAY,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Presence {
    pub kind: PresenceKind,
    pub place_id: Option<u64>,
    pub root_place_id: Option<u64>,
    pub universe_id: Option<u64>,
    /// Roblox game server job id (used later to join servers).
    pub game_id: Option<String>,
    pub last_location: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Friend {
    pub id: u64,
    pub name: String,
    pub display_name: String,
    pub is_banned: bool,
}

/// A user found through the "Add friends" search (not necessarily a friend).
#[derive(Clone, Debug, PartialEq)]
pub struct UserEntry {
    pub id: u64,
    pub name: String,
    pub display_name: String,
}

impl UserEntry {
    pub fn display_or_name(&self) -> &str {
        if self.display_name.is_empty() { &self.name } else { &self.display_name }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FriendRow {
    pub friend: Friend,
    pub presence: Presence,
    pub game_name: Option<String>,
    pub last_online: Option<i64>,
}

impl FriendRow {
    /// Status label shown in the list (English UI).
    pub fn status_label(&self) -> String {
        match self.presence.kind {
            PresenceKind::InGame => match &self.game_name {
                Some(g) => format!("Playing {}", g),
                // The games endpoint omits universes it cannot show
                // (private/age-restricted); presence `lastLocation` then
                // carries the experience name — use it as a fallback.
                None => match clean_last_location(&self.presence.last_location) {
                    Some(g) => format!("Playing {}", g),
                    None => "In game".to_string(),
                },
            },
            PresenceKind::Online => "Online".to_string(),
            PresenceKind::Studio => match clean_last_location(&self.presence.last_location) {
                Some(g) => format!("In Studio: {}", g),
                None => "In Studio".to_string(),
            },
            PresenceKind::Offline | PresenceKind::Invisible => {
                match self.last_online {
                    Some(t) => format!("Offline \u{b7} last online {}", humanize_last_online(t)),
                    None => "Offline".to_string(),
                }
            }
        }
    }

    pub fn matches_filter(&self, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        let q = q.to_lowercase();
        self.friend.name.to_lowercase().contains(&q)
            || self.friend.display_name.to_lowercase().contains(&q)
            || self.game_name.as_deref().unwrap_or("").to_lowercase().contains(&q)
            || self.presence.last_location.to_lowercase().contains(&q)
    }
}

/// Clean a presence `lastLocation` string into a possible place name.
/// Returns `None` when it does not name a place ("Website", "Studio", empty).
pub fn clean_last_location(loc: &str) -> Option<String> {
    let mut s = loc.trim();
    let lower = s.to_lowercase();
    for p in ["playing: ", "playing ", "creating ", "building ", "developing ", "visiting "] {
        if lower.starts_with(p) {
            // Prefix is ASCII, so byte offset == char offset.
            s = s[p.len()..].trim_start();
            break;
        }
    }
    let s = s.trim();
    if s.is_empty()
        || s.eq_ignore_ascii_case("website")
        || s.eq_ignore_ascii_case("studio")
        || s.eq_ignore_ascii_case("mobile website")
        || s.eq_ignore_ascii_case("in game")
    {
        None
    } else {
        Some(s.to_string())
    }
}

/// Stable sort: presence rank, then case-insensitive display name.
pub fn sort_rows(rows: &mut [FriendRow]) {
    rows.sort_by(|a, b| {
        let ka = (a.presence.kind.rank(), a.friend.display_name.to_lowercase());
        let kb = (b.presence.kind.rank(), b.friend.display_name.to_lowercase());
        ka.cmp(&kb)
    });
}

/// Parse Roblox ISO-8601 UTC timestamps ("2024-05-01T12:34:56.789Z" or without
/// fraction). Returns unix seconds. No external crates: days-from-civil.
pub fn parse_iso8601_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil algorithm.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

/// Human-friendly "time ago" string.
pub fn humanize_last_online(unix: i64) -> String {
    let now = now_unix();
    let d = (now - unix).max(0);
    if d < 60 {
        "just now".to_string()
    } else if d < 3600 {
        let m = d / 60;
        if m == 1 { "1 minute ago".into() } else { format!("{} minutes ago", m) }
    } else if d < 86400 {
        let h = d / 3600;
        if h == 1 { "1 hour ago".into() } else { format!("{} hours ago", h) }
    } else if d < 86400 * 30 {
        let dd = d / 86400;
        if dd == 1 { "1 day ago".into() } else { format!("{} days ago", dd) }
    } else {
        // Show the date for older entries.
        format_date(unix)
    }
}

/// Format unix seconds as "YYYY-MM-DD".
pub fn format_date(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    // civil_from_days (Howard Hinnant).
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Merge an API snapshot into display rows.
#[derive(Debug)]
pub struct Snapshot {
    pub friends: Vec<Friend>,
    pub presences: HashMap<u64, Presence>,
    pub games: HashMap<u64, String>,
    pub last_online: HashMap<u64, i64>,
}

pub fn build_rows(snap: &Snapshot) -> Vec<FriendRow> {
    let mut rows: Vec<FriendRow> = snap
        .friends
        .iter()
        .map(|f| FriendRow {
            friend: f.clone(),
            presence: snap.presences.get(&f.id).cloned().unwrap_or_default(),
            game_name: snap
                .presences
                .get(&f.id)
                .and_then(|p| p.universe_id)
                .and_then(|u| snap.games.get(&u).cloned()),
            last_online: snap.last_online.get(&f.id).copied(),
        })
        .collect();
    sort_rows(&mut rows);
    rows
}

/// Deterministic color for the avatar placeholder, from the user id.
pub fn avatar_placeholder_color(id: u64) -> crate::theme::Rgba {
    const PALETTE: [crate::theme::Rgba; 6] = [
        [0x33, 0x5F, 0xFF, 0xFF],
        [0x00, 0xB0, 0x6F, 0xFF],
        [0xF2, 0x8A, 0x2E, 0xFF],
        [0x8B, 0x5C, 0xF6, 0xFF],
        [0x00, 0xA2, 0xFF, 0xFF],
        [0xE2, 0x56, 0x4E, 0xFF],
    ];
    PALETTE[(id % 6) as usize]
}
