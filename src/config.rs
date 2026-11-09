//! Session persistence. The `.ROBLOSECURITY` token is stored in the OS config
//! directory with owner-only permissions on Unix (0600). On Windows the file
//! lives under %APPDATA% (user-private ACL by default).

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

pub fn save_session(s: &Session) -> io::Result<()> {
    let dir = app_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config dir"))?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("session.json");
    let json = serde_json::to_vec_pretty(s)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(&json)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    fs::write(&path, &json)?;
    Ok(())
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
