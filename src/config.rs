//! Persistence: the active session (`session.json`), the saved account list
//! for quick switching (`accounts.json`, both 0600 on Unix) and UI settings
//! (`settings.json` — theme, custom colors, recommendations toggle).

use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub security_token: String,
    pub user_id: u64,
    pub username: String,
    pub display_name: String,
}

pub fn app_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rustblox"))
}

pub fn avatar_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("rustblox").join("avatars"))
}

fn session_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("session.json"))
}

fn accounts_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("accounts.json"))
}

fn settings_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("settings.json"))
}

/// Write a file with owner-only permissions (0600) on Unix.
fn write_private(path: &std::path::Path, data: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(data)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    fs::write(path, data)?;
    Ok(())
}

pub fn save_session(s: &Session) -> io::Result<()> {
    let dir = app_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config dir"))?;
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(s)?;
    write_private(&dir.join("session.json"), &json)
}

pub fn load_session() -> Option<Session> {
    let path = session_path()?;
    let data = fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

pub fn clear_session() {
    if let Some(p) = session_path() {
        let _ = fs::remove_file(p);
    }
}

// ---------------------------------------------------------------------------
// Saved accounts (quick switching, Discord-style)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AccountsFile {
    #[serde(default)]
    pub accounts: Vec<Session>,
}

pub fn load_accounts() -> Vec<Session> {
    match accounts_path().and_then(|p| fs::read(p).ok()) {
        Some(data) => serde_json::from_slice::<AccountsFile>(&data)
            .map(|f| f.accounts)
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

fn save_accounts(list: &[Session]) -> io::Result<()> {
    let dir = app_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config dir"))?;
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(&AccountsFile { accounts: list.to_vec() })?;
    write_private(&dir.join("accounts.json"), &json)
}

/// Insert or replace (by user id) an account and persist.
pub fn upsert_account(s: &Session) {
    let mut list = load_accounts();
    match list.iter().position(|a| a.user_id == s.user_id) {
        Some(i) => list[i] = s.clone(),
        None => list.push(s.clone()),
    }
    if let Err(e) = save_accounts(&list) {
        eprintln!("warn: could not persist accounts: {e}");
    }
}

/// Remove an account by user id and persist.
pub fn remove_account(user_id: u64) {
    let mut list = load_accounts();
    let before = list.len();
    list.retain(|a| a.user_id != user_id);
    if list.len() != before {
        if let Err(e) = save_accounts(&list) {
            eprintln!("warn: could not persist accounts: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// UI settings (theme, recommendations, …)
// ---------------------------------------------------------------------------

/// A place the user was recently seen playing (local tracking fallback for
/// the discovery feed's "Continue playing" row).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentPlace {
    pub universe_id: u64,
    pub root_place_id: u64,
    pub name: String,
    /// Unix seconds of the last time we saw the user in this place.
    pub ts: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Active theme id: "dark" | "light" | "gruvbox" | "nord" | "custom".
    pub theme: String,
    /// The user-edited theme used when `theme == "custom"`.
    pub custom: Option<Theme>,
    /// Show the Roblox recommendation feed on the Home screen.
    pub show_recommendations: bool,
    /// Locally tracked recently played places (fallback source).
    pub recent_places: Vec<RecentPlace>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "dark".into(),
            custom: None,
            show_recommendations: true,
            recent_places: Vec::new(),
        }
    }
}

pub fn load_settings() -> Settings {
    match settings_path().and_then(|p| fs::read(p).ok()) {
        Some(data) => serde_json::from_slice(&data).unwrap_or_default(),
        None => Settings::default(),
    }
}

pub fn save_settings(s: &Settings) {
    if let Some(dir) = app_dir() {
        let _ = fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_vec_pretty(s) {
            if let Err(e) = fs::write(dir.join("settings.json"), json) {
                eprintln!("warn: could not persist settings: {e}");
            }
        }
    }
}

/// Record that the user was last seen in a place right now (dedup by root
/// place id, most recent first, capped). Saves only when something changed.
pub fn note_recent_place(universe_id: u64, root_place_id: u64, name: &str) {
    if root_place_id == 0 {
        return;
    }
    let mut st = load_settings();
    let ts = crate::model::now_unix();
    if let Some(e) = st
        .recent_places
        .iter_mut()
        .find(|e| e.root_place_id == root_place_id)
    {
        if e.ts == ts && e.name == name {
            return; // nothing changed
        }
        e.ts = ts;
        e.universe_id = if universe_id != 0 { universe_id } else { e.universe_id };
        if !name.is_empty() {
            e.name = name.to_string();
        }
    } else {
        st.recent_places.insert(
            0,
            RecentPlace {
                universe_id,
                root_place_id,
                name: name.to_string(),
                ts,
            },
        );
        st.recent_places.truncate(12);
    }
    st.recent_places.sort_by(|a, b| b.ts.cmp(&a.ts));
    save_settings(&st);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let st = Settings {
            theme: "gruvbox".into(),
            custom: Some(crate::theme::NORD),
            show_recommendations: false,
            recent_places: vec![RecentPlace {
                universe_id: 1,
                root_place_id: 2,
                name: "Test".into(),
                ts: 123,
            }],
        };
        let json = serde_json::to_string(&st).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme, "gruvbox");
        assert_eq!(back.custom, Some(crate::theme::NORD));
        assert!(!back.show_recommendations);
        assert_eq!(back.recent_places.len(), 1);
    }

    #[test]
    fn settings_defaults_are_lenient() {
        // An empty object must deserialize with sane defaults.
        let st: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(st.theme, "dark");
        assert!(st.show_recommendations);
        assert!(st.recent_places.is_empty());
    }
}
