//! Background network worker.
//!
//! The UI thread never blocks: commands are queued to the worker thread, and
//! results are delivered back through the winit `EventLoopProxy` so the event
//! loop wakes up immediately. Avatar downloads run on a small thread pool and
//! are cached on disk keyed by the thumbnail URL. Note that avatars are
//! fetched from the CDN *without* the session cookie (it must never leak to
//! third-party CDN hosts).
//!
//! Layout: `command.rs` is the command enum, `event.rs` the events sent back
//! to the UI, `tasks/` the command handlers and refresh pipelines.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod command;
mod event;
mod tasks;

pub use command::Command;
pub use event::{ImageKind, UserEvent, WorkerEvent};

use crate::config::Session;
use crate::roblox::social;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use winit::event_loop::EventLoopProxy;

/// Number of avatar/icon download threads.
const IMAGE_WORKERS: usize = 4;

/// Control handle for the background worker thread.
pub struct Worker {
	/// Command channel into the worker thread.
	pub tx: Sender<Command>,
}

/// One queued image download (user avatar or place icon).
struct ImageJob {
	kind: ImageKind,
	url: String,
}

/// Dedup map shared with the worker: target id -> last URL enqueued.
type PushedMap = Arc<Mutex<HashMap<u64, String>>>;

/// Lock a shared map, recovering from a poisoned mutex instead of panicking
/// (a worker panic must not take the whole UI down with another panic).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
	m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Spawn the command worker plus the image download pool.
pub fn spawn(proxy: EventLoopProxy<UserEvent>, session: Option<Session>) -> Worker {
	let (tx, rx) = channel::<Command>();
	let proxy = Arc::new(proxy);

	// --- Image download pool (avatars + place icons) -----------------------
	let (job_tx, job_rx) = channel::<ImageJob>();
	let job_rx = Arc::new(Mutex::new(job_rx));
	for _ in 0..IMAGE_WORKERS {
		spawn_image_worker(&job_rx, &proxy);
	}

	// --- Main worker --------------------------------------------------------
	let avatars: PushedMap = Arc::default();
	let icons: PushedMap = Arc::default();
	let main = {
		let proxy = Arc::clone(&proxy);
		let avatars = Arc::clone(&avatars);
		let icons = Arc::clone(&icons);
		std::thread::Builder::new().name("rustblox-worker".into()).spawn(move || {
			let mut ctx = tasks::Ctx::new(proxy, job_tx, avatars, icons);
			ctx.restore_session(session);
			ctx.run(&rx);
		})
	};
	// A failed spawn leaves the channel without a reader: commands then fail
	// to send, which the UI already tolerates — never panic here.
	if let Err(e) = main {
		crate::logs::push("app", &format!("worker thread spawn failed: {}", e));
	}

	Worker { tx }
}

/// One image-pool thread: disk cache first, CDN decode second, then hand the
/// RGBA bitmap to the UI.
fn spawn_image_worker(
	job_rx: &Arc<Mutex<Receiver<ImageJob>>>,
	proxy: &Arc<EventLoopProxy<UserEvent>>,
) {
	let job_rx = Arc::clone(job_rx);
	let proxy = Arc::clone(proxy);
	let spawned = std::thread::Builder::new().name("rustblox-images".into()).spawn(move || loop {
		let Ok(job) = lock(&job_rx).recv() else {
			return;
		};
		let size = match job.kind {
			ImageKind::User(_) => crate::theme::AVATAR_SIZE,
			ImageKind::Place(_) => crate::theme::PLACE_ICON_PX,
		};
		if let Some(bytes) = crate::avatar_cache::load(&job.url, size) {
			let _ =
				proxy.send_event(UserEvent::ImageReady { kind: job.kind, rgba: Arc::new(bytes) });
			continue;
		}
		let agent = crate::avatar_cache::cdn_agent();
		if let Ok(rgba) = social::download_image(&agent, &job.url, size) {
			crate::avatar_cache::store(&job.url, &rgba);
			let _ =
				proxy.send_event(UserEvent::ImageReady { kind: job.kind, rgba: Arc::new(rgba) });
		}
	});
	if let Err(e) = spawned {
		crate::logs::push("app", &format!("image worker spawn failed: {}", e));
	}
}
