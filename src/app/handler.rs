//! winit `ApplicationHandler` glue: lifecycle, event dispatch and timers.

use crate::app::state::{GameView, Screen, ARROW_REPEAT_MS, WINDOW_H, WINDOW_W};
use crate::app::App;
use crate::game::GamePhase;
use crate::ui::Hit;
use crate::worker::{Command, ImageKind, UserEvent, WorkerEvent};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowLevel};

impl ApplicationHandler<UserEvent> for App {
	fn suspended(&mut self, _elwt: &ActiveEventLoop) {
		// Android (and iOS) lifecycle: the activity is going away, so GPU
		// surfaces tied to the native window must be dropped. Everything is
		// recreated in `resumed`.
		self.gfx = None;
		self.window = None;
		self.request_redraw();
	}

	fn resumed(&mut self, elwt: &ActiveEventLoop) {
		let attrs = Window::default_attributes()
			.with_title("RustBlox \u{2014} Unofficial Roblox Client")
			.with_inner_size(winit::dpi::LogicalSize::new(WINDOW_W, WINDOW_H))
			.with_min_inner_size(winit::dpi::LogicalSize::new(880.0, 600.0))
			.with_window_level(WindowLevel::Normal);
		// Desktop: launch straight into borderless fullscreen (Alt+F11 or the
		// window manager still escapes it). Mobile ignores these attributes —
		// winit binds the window to the native activity surface there, where
		// the immersive-mode hook already hides the system bars.
		#[cfg(not(any(target_os = "android", target_os = "ios")))]
		let attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
		let window = Arc::new(elwt.create_window(attrs).expect("create window"));
		let size = window.inner_size();
		self.scale = window.scale_factor() as f32;
		self.fbw = size.width.max(1) as i32;
		self.fbh = size.height.max(1) as i32;
		self.gfx = Some(crate::gfx::Gfx::new(&window));
		self.window = Some(window);
		// Mobile: (re)apply fullscreen/immersive window state every time the
		// native window comes up (hide system bars, cutout mode). No-op on
		// desktop — no hooks are registered there.
		crate::platform::window_ready();
		self.request_redraw();
	}

	fn window_event(
		&mut self,
		elwt: &ActiveEventLoop,
		_window_id: winit::window::WindowId,
		event: WindowEvent,
	) {
		self.on_window_event(elwt, event);
	}

	fn user_event(&mut self, _elwt: &ActiveEventLoop, event: UserEvent) {
		match event {
			UserEvent::Worker(ev) => match ev {
				WorkerEvent::Game(ge) => self.on_game_event(ge),
				other => self.on_worker_event(other),
			},
			UserEvent::ImageReady { kind, rgba } => {
				match kind {
					ImageKind::User(id) => {
						self.avatars.insert(id, rgba);
					}
					ImageKind::Place(id) => {
						self.place_icons.insert(id, rgba);
					}
				}
				self.request_redraw();
			}
		}
	}

	fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
		let now = Instant::now();

		// Game screen: advance the camera, watch the session health and
		// redraw every frame.
		if let Screen::Game(GameView::Active { session, ctrl, last_frame, fps, .. }) =
			&mut self.screen
		{
			let dt = last_frame.elapsed().as_secs_f32().min(0.1);
			*last_frame = now;
			*fps = *fps * 0.9 + (1.0 / dt.max(1.0 / 240.0)) * 0.1;
			let st = session.state.lock().unwrap_or_else(|p| p.into_inner());
			// Spawn the character once the world (or the fallback ground)
			// gives us a place to stand.
			if !ctrl.spawned && st.pos_init {
				ctrl.place(st.local_pos);
			}
			ctrl.step(dt, &st.colliders);
			// Publish the character + camera back for the scene builder and
			// the renderer (the game thread rebuilds parts, not the camera).
			let mut st = st;
			st.local_pos = ctrl.pos;
			st.local_yaw = ctrl.yaw;
			st.scene.camera = ctrl.camera;
			let phase = st.phase.clone();
			drop(st);
			if matches!(phase, GamePhase::Failed(_) | GamePhase::Left) {
				let msg = match phase {
					GamePhase::Failed(m) => Some(m),
					_ => None,
				};
				self.leave_game(msg);
			} else {
				self.needs_redraw = true;
			}
		}

		// Auto refresh timer.
		if let Screen::Menu(v) = &mut self.screen {
			if !v.refreshing {
				if let Some(next) = v.next_refresh {
					if now >= next {
						v.refreshing = true;
						let tx = self.tx.clone();
						let _ = tx.send(Command::StartRefresh);
						self.needs_redraw = true;
					}
				}
			}
		}

		// Mobile soft keyboard: sync the focused field with the platform IME
		// state (hooks are no-ops on desktop, so this is free there).
		if self.focus.is_some() {
			if let Some(text) = crate::platform::ime_pull() {
				if let Some(f) = self.focused_field_mut() {
					if f.value != text {
						f.set_text(&text);
						self.needs_redraw = true;
					}
				}
			}
		}

		// Auto repeat for a held scroll arrow.
		let mut arrow_tick: Option<Hit> = None;
		if let Some((hit, next_tick)) = &self.arrow_hold {
			if now >= *next_tick {
				arrow_tick = Some(hit.clone());
			}
		}
		if let Some(hit) = arrow_tick {
			self.scroll_step(&hit);
			self.arrow_hold = Some((hit, now + Duration::from_millis(ARROW_REPEAT_MS)));
			self.needs_redraw = true;
		}

		if self.needs_redraw {
			self.needs_redraw = false;
			if let Some(w) = self.window.as_ref() {
				w.request_redraw();
			}
		}

		// Compute the earliest wake-up for animations / timers.
		let mut wake: Option<Instant> = self.schedule_anim();
		if let Screen::Menu(v) = &self.screen {
			if !v.refreshing {
				if let Some(next) = v.next_refresh {
					wake = Some(match wake {
						Some(w) => w.min(next),
						None => next,
					});
				}
			}
		}
		match wake {
			Some(t) if t > now => elwt.set_control_flow(ControlFlow::WaitUntil(t)),
			Some(_) => {
				// Animation tick due: redraw on the next frame.
				self.needs_redraw = true;
				if let Some(w) = self.window.as_ref() {
					w.request_redraw();
				}
				elwt.set_control_flow(ControlFlow::Poll);
			}
			None => elwt.set_control_flow(ControlFlow::Wait),
		}
	}
}
