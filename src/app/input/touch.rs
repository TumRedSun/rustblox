//! Multi-touch game pad routing (joystick, camera look, lift, leave).

use crate::app::state::{game_pad_layout, GameView, Screen};
use crate::app::App;
use winit::event::{Touch, TouchPhase};

/// Joystick deadzone: tiny deflections near the center read as zero so a
/// resting thumb does not drift the character.
fn deadzone(v: f32) -> f32 {
	if v.abs() < 0.12 {
		0.0
	} else {
		v
	}
}

impl App {
	/// Touch input on the game screen (mobile): each finger claims one role
	/// by its touch id — joystick (bottom-left circle), camera-look (any
	/// other spot), +/− buttons (bottom-right), "X" (leave, top-right).
	/// The game screen redraws every frame, so no redraw bookkeeping here.
	pub(super) fn game_touch(&mut self, touch: Touch) {
		let (w, h) = match &self.window {
			Some(win) => {
				let sz = win.inner_size();
				(sz.width as f32, sz.height as f32)
			}
			None => return,
		};
		let s = self.scale;
		let (sc, sr, up_r, down_r, leave_r) = game_pad_layout(w, h, s);
		let (x, y) = (touch.location.x as f32, touch.location.y as f32);
		let in_rect = |r: (i32, i32, i32, i32)| {
			x >= r.0 as f32 && x < (r.0 + r.2) as f32 && y >= r.1 as f32 && y < (r.1 + r.3) as f32
		};
		let stick_id = self.game_pad.stick.map(|(id, _, _)| id);
		let look_id = self.game_pad.look.map(|(id, _, _)| id);
		match touch.phase {
			TouchPhase::Started => {
				let dx = x - sc.0;
				let dy = y - sc.1;
				if dx * dx + dy * dy <= sr * sr {
					self.game_pad.stick = Some((touch.id, sc.0, sc.1));
					self.game_pad.stick_vec = (0.0, 0.0);
				} else if in_rect(up_r) {
					self.game_pad.up = Some(touch.id);
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_lift = 1.0;
					}
				} else if in_rect(down_r) {
					self.game_pad.down = Some(touch.id);
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_lift = -1.0;
					}
				} else if in_rect(leave_r) {
					self.leave_game(None);
				} else {
					self.game_pad.look = Some((touch.id, x, y));
				}
			}
			TouchPhase::Moved => {
				if stick_id == Some(touch.id) {
					let (_, cx, cy) = self.game_pad.stick.unwrap_or((touch.id, sc.0, sc.1));
					let mut vx = (x - cx) / sr;
					let mut vy = -(y - cy) / sr; // screen y grows downward
					let len = (vx * vx + vy * vy).sqrt();
					if len > 1.0 {
						vx /= len;
						vy /= len;
					}
					self.game_pad.stick_vec = (vx, vy);
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_stick = (deadzone(vx), deadzone(vy));
					}
				} else if look_id == Some(touch.id) {
					let (_, lx, ly) = self.game_pad.look.unwrap_or((touch.id, x, y));
					let (dx, dy) = (x - lx, y - ly);
					self.game_pad.look = Some((touch.id, x, y));
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_look(dx, dy);
					}
				}
			}
			TouchPhase::Ended | TouchPhase::Cancelled => {
				if stick_id == Some(touch.id) {
					self.game_pad.stick = None;
					self.game_pad.stick_vec = (0.0, 0.0);
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_stick = (0.0, 0.0);
					}
				}
				if look_id == Some(touch.id) {
					self.game_pad.look = None;
				}
				if self.game_pad.up == Some(touch.id) {
					self.game_pad.up = None;
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_lift = 0.0;
					}
				}
				if self.game_pad.down == Some(touch.id) {
					self.game_pad.down = None;
					if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
						ctrl.touch_lift = 0.0;
					}
				}
			}
		}
	}
}
