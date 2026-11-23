//! Frame drawing and animation scheduling for the app window.

mod hud;

use crate::app::state::{GameView, Screen, UiCtx};
use crate::app::App;
use crate::draw::FB;
use crate::theme;
use std::time::{Duration, Instant};

impl App {
	pub(in crate::app) fn draw_frame(&mut self) {
		let Some(gfx) = self.gfx.as_mut() else { return };
		{
			let mut fb = FB::gpu(self.fbw, self.fbh);
			// Immediate-mode hit testing: the map holds ONLY what the current
			// frame drew. Without this clear, hits from previously visited
			// screens/menus stay clickable at their old coordinates (e.g.
			// theme presets firing from the Home tab, or sidebar items
			// reacting while the sidebar is collapsed).
			self.hits.clear();
			fb.fill(theme::cur().bg);
			let now_ms = self.start_instant.elapsed().as_millis() as u64;
			match &mut self.screen {
				Screen::Login(view) => {
					let mut ctx = UiCtx {
						fonts: &self.fonts,
						cursor: self.cursor,
						hits: &mut self.hits,
						focus: self.focus,
						scale: self.scale,
						now_ms,
						avatars: &mut self.avatars,
						icons: &mut self.place_icons,
					};
					crate::login_ui::draw(
						view,
						&mut ctx,
						&mut fb,
						self.restoring,
						self.adding_account,
					);
				}
				Screen::Menu(view) => {
					let mut ctx = UiCtx {
						fonts: &self.fonts,
						cursor: self.cursor,
						hits: &mut self.hits,
						focus: self.focus,
						scale: self.scale,
						now_ms,
						avatars: &mut self.avatars,
						icons: &mut self.place_icons,
					};
					crate::menu_ui::draw(view, &mut ctx, &mut fb);
				}
				Screen::Game(gv) => {
					Self::draw_game_hud(
						&self.fonts,
						self.scale,
						fb.w,
						fb.h,
						gv,
						&mut fb,
						now_ms,
						&self.game_pad,
					);
				}
			}
			let prims = fb.take_prims();
			let mut atlas = self.fonts.atlas.lock().unwrap_or_else(|p| p.into_inner());
			// The 3D scene comes from the live session (if connected).
			let scene = match &self.screen {
				Screen::Game(GameView::Active { session, .. }) => {
					// Poison-tolerant: a game-thread panic must never take
					// the UI down through a poisoned mutex unwrap.
					let st = session.state.lock().unwrap_or_else(|p| p.into_inner());
					Some(st.scene.clone())
				}
				_ => None,
			};
			let _ = gfx.render(&prims, &mut atlas, scene.as_ref());
		}
	}

	pub(in crate::app) fn schedule_anim(&mut self) -> Option<Instant> {
		let now = Instant::now();
		let mut wakes: Vec<Instant> = Vec::with_capacity(3);

		let spinner = match &self.screen {
			Screen::Login(v) => v.busy,
			Screen::Menu(v) => {
				v.refreshing || v.add.searching || v.places_searching || v.requests.loading
			}
			Screen::Game(GameView::Joining { .. }) => true,
			Screen::Game(GameView::Active { .. }) => false,
		};
		if spinner {
			wakes.push(now + Duration::from_millis(90));
		}
		if matches!(self.screen, Screen::Menu(_)) {
			wakes.push(now + Duration::from_millis(1000));
		}
		wakes.into_iter().min()
	}
}
