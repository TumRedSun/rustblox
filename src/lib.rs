//! RustBlox library crate: GPU-accelerated UI + 3D scene rendering
//! (wgpu + winit), networking (ureq + rustls + RakNet UDP) and models for
//! the official Roblox web APIs.

pub mod app;
pub mod avatar_cache;
pub mod config;
pub mod draw;
pub mod font;
pub mod game;
pub mod gfx;
pub mod login_ui;
pub mod logs;
pub mod menu_ui;
pub mod model;
pub mod net;
pub mod netlog;
pub mod platform;
pub mod raknet;
pub mod roblox;
pub mod theme;
pub mod ui;
pub mod world;

pub use netlog::enabled as netlog_enabled;

use crate::config::Session;
use crate::net::UserEvent;

/// Shared runner: spawns the network worker and hands control to the winit
/// event loop. Platform harnesses (desktop binary in `src/main.rs`, mobile
/// crate in `mobile/`) configure the event loop however their platform
/// requires and then delegate here.
pub fn run_with(event_loop: winit::event_loop::EventLoop<UserEvent>, session: Option<Session>) {
    // Crash-proofing first: the panic hook + persistent log file must be up
    // before any other subsystem can panic. On Android this resolves the
    // app-private data dir (the APP handle is set by the harness before it
    // calls us), on desktop the regular config dir.
    logs::install_panic_hook();
    logs::init_file();
    if let Some(p) = logs::log_file_path() {
        logs::push("app", &format!("log file: {}", p.display()));
    }
    // Startup environment block ("debug" lines): pins every log to the
    // exact build and hardware it came from — the first thing to ask for
    // when a join or a render misbehaves on someone else's machine.
    logs::push(
        "debug",
        &format!(
            "rustblox v{} ({} profile, git {}) — {} {}",
            env!("CARGO_PKG_VERSION"),
            if cfg!(debug_assertions) { "debug" } else { "release" },
            env!("RUSTBLOX_BUILD_SHA"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        ),
    );
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    logs::push("debug", &format!("cpu threads: {threads}"));
    // Session type (x11/wayland/tty) explains clipboard/paste and window
    // quirks on Linux; harmless to skip when the variable is unset.
    for var in ["XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            logs::push("debug", &format!("env {var}={v}"));
        }
    }
    // Make a crash from the previous session visible in Settings → Logs.
    logs::load_previous_crash();

    let proxy = event_loop.create_proxy();
    let worker = net::spawn(proxy, session.clone());
    let mut app = app::App::new(worker.tx, session.is_some());
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("rustblox: event loop error: {e}");
        std::process::exit(1);
    }
}

/// Default entry point: builds a plain (non-activity-bound) event loop.
/// Used by the desktop binary and by the iOS harness — winit's iOS backend
/// needs no extra platform handle.
pub fn run_desktop() {
    // Crash-proofing from the very first line — event loop construction and
    // config loading below can panic too. run_with re-calls both; they are
    // idempotent. (The iOS harness enters through here as well.)
    logs::install_panic_hook();
    logs::init_file();
    let session = config::load_session();
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build the winit event loop");
    run_with(event_loop, session);
}
