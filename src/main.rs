//! RustBlox — an open-source (GPL-3.0) Roblox client built in Rust.
//!
//! Milestone 1: sign in with password/2FA or a session cookie and browse your
//! friends list with live presence statuses. The client talks to the official
//! Roblox web APIs (auth/users/friends/presence/thumbnails/games) — the same
//! endpoints the official website and clients use.

use rustblox::config;
use rustblox::net::{self, UserEvent};

fn main() {
    // Diagnostics first: persistent log file + panic hook, so a crash
    // leaves a report on disk instead of dying silently with the
    // in-memory log ring.
    rustblox::logs::init_file();
    rustblox::logs::install_panic_hook();
    let session = config::load_session();
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build the winit event loop");
    let proxy = event_loop.create_proxy();
    let worker = net::spawn(proxy, session.clone());
    let mut app = rustblox::app::App::new(worker.tx, session.is_some());
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("rustblox: event loop error: {e}");
        std::process::exit(1);
    }
}
