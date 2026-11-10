//! RustBlox library crate: GUI (pixels + winit), networking (ureq + rustls)
//! and models for the official Roblox web APIs.

pub mod app;
pub mod avatar_cache;
pub mod config;
pub mod draw;
pub mod font;
pub mod login_ui;
pub mod menu_ui;
pub mod model;
pub mod net;
pub mod roblox;
pub mod theme;
pub mod ui;

use crate::config::Session;
use crate::net::UserEvent;

/// Shared runner: spawns the network worker and hands control to the winit
/// event loop. Platform harnesses (desktop binary in `src/main.rs`, mobile
/// crate in `mobile/`) configure the event loop however their platform
/// requires and then delegate here.
pub fn run_with(event_loop: winit::event_loop::EventLoop<UserEvent>, session: Option<Session>) {
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
    let session = config::load_session();
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build the winit event loop");
    run_with(event_loop, session);
}
