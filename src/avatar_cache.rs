//! Disk cache for avatar bitmaps. Keys are FNV-1a hashes of the thumbnail
//! URL; the payload is the decoded square RGBA buffer.

use std::path::PathBuf;
use ureq::Agent;

pub fn cdn_agent() -> Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(crate::roblox::client::UA)
        .build()
}

pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cache_path(url: &str) -> Option<PathBuf> {
    crate::config::avatar_cache_dir()
        .map(|d| d.join(format!("{:016x}.rgba", fnv1a64(url.as_bytes()))))
}

/// Load a cached decoded avatar (48x48 RGBA8).
pub fn load(url: &str) -> Option<Vec<u8>> {
    let p = cache_path(url)?;
    let data = std::fs::read(p).ok()?;
    let expected = (crate::theme::AVATAR_SIZE * crate::theme::AVATAR_SIZE * 4) as usize;
    if data.len() == expected {
        Some(data)
    } else {
        None
    }
}

pub fn store(url: &str, rgba: &[u8]) {
    if let Some(p) = cache_path(url) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, rgba);
    }
}
