//! Lightweight logging for the network/render internals.
//!
//! Disabled by default; set RUSTBLOX_DEBUG=1 (or RUSTBLOX_DEBUG=net|game|gfx)
//! to see diagnostics. Everything goes to stderr.

use std::sync::atomic::{AtomicBool, Ordering};

static NET_ENABLED: AtomicBool = AtomicBool::new(false);
static GAME_ENABLED: AtomicBool = AtomicBool::new(false);
static GFX_ENABLED: AtomicBool = AtomicBool::new(false);

fn init() {
    let v = std::env::var("RUSTBLOX_DEBUG").unwrap_or_default().to_lowercase();
    let all = v == "1" || v == "true" || v == "all";
    NET_ENABLED.store(all || v.contains("net"), Ordering::Relaxed);
    GAME_ENABLED.store(all || v.contains("game"), Ordering::Relaxed);
    GFX_ENABLED.store(all || v.contains("gfx"), Ordering::Relaxed);
}

pub fn enabled(kind: &str) -> bool {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(init);
    match kind {
        "net" => NET_ENABLED.load(Ordering::Relaxed),
        "game" => GAME_ENABLED.load(Ordering::Relaxed),
        "gfx" => GFX_ENABLED.load(Ordering::Relaxed),
        _ => false,
    }
}

#[macro_export]
macro_rules! netlog {
    // Wrapped in a block so the macro is a single expression: match arms in
    // raknet.rs use it in value position.
    (net, $($arg:tt)*) => {{
        $crate::logs::push("net", &format!($($arg)*));
        if $crate::netlog_enabled("net") { eprintln!("[rustblox/net] {}", format_args!($($arg)*)); }
    }};
    (info, $($arg:tt)*) => {{
        $crate::logs::push("net", &format!($($arg)*));
        if $crate::netlog_enabled("net") { eprintln!("[rustblox/net] {}", format_args!($($arg)*)); }
    }};
    (debug, $($arg:tt)*) => {{
        $crate::logs::push("net", &format!($($arg)*));
        if $crate::netlog_enabled("net") { eprintln!("[rustblox/net] {}", format_args!($($arg)*)); }
    }};
    (game, $($arg:tt)*) => {{
        $crate::logs::push("game", &format!($($arg)*));
        if $crate::netlog_enabled("game") { eprintln!("[rustblox/game] {}", format_args!($($arg)*)); }
    }};
    (gfx, $($arg:tt)*) => {{
        $crate::logs::push("gfx", &format!($($arg)*));
        if $crate::netlog_enabled("gfx") { eprintln!("[rustblox/gfx] {}", format_args!($($arg)*)); }
    }};
}
