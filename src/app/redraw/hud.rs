//! Game-screen HUD overlay (joining spinner / live diagnostics).

use crate::app::state::{game_pad_layout, GamePad, GameView};
use crate::app::App;
use crate::draw::FB;
use crate::font::Fonts;
use crate::theme;

impl App {
	/// HUD overlay for the game screen (joining spinner / live diagnostics).
	/// Explicit field args (not &self) so it can run inside the
	/// `&mut self.screen` match without aliasing.
	pub(super) fn draw_game_hud(
		fonts: &Fonts,
		scale: f32,
		screen_w: i32,
		screen_h: i32,
		gv: &GameView,
		fb: &mut FB,
		now_ms: u64,
		pad_state: &GamePad,
	) {
		let t = theme::cur();
		let s = scale;
		let w = screen_w;
		let h = screen_h;
		match gv {
			GameView::Joining { place_id, status, .. } => {
				let box_w = 460.0 * s;
				// Wrap long pipeline/refusal messages to the box width and
				// grow the box so nothing runs off the edge.
				let status_lines =
					crate::font::wrap_text(fonts, status, box_w - 48.0 * s, 14.0 * s, false);
				let box_h = (104.0 + status_lines.len() as f32 * 19.0 + 28.0).max(150.0) * s;
				let (bx, by) = ((w as f32 - box_w) / 2.0, (h as f32 - box_h) / 2.0);
				fb.rect_rounded(bx as i32, by as i32, box_w as i32, box_h as i32, 14, t.surface);
				crate::font::draw(
					fonts,
					fb,
					bx + 24.0 * s,
					by + 44.0 * s,
					&format!("Joining place {}", place_id),
					20.0 * s,
					t.text,
					true,
					0.0,
				);
				for (i, line) in status_lines.iter().enumerate() {
					crate::font::draw(
						fonts,
						fb,
						bx + 24.0 * s,
						by + 76.0 * s + i as f32 * 19.0 * s,
						line,
						14.0 * s,
						t.text_dim,
						false,
						0.0,
					);
				}
				// Spinner.
				let ang = (now_ms % 900) as f32 / 900.0 * std::f32::consts::TAU;
				let cx = (bx + box_w - 40.0 * s) as i32;
				let cy = (by + 44.0 * s) as i32;
				fb.ring(cx, cy, (7.0 * s) as i32, (5.0 * s) as i32, t.text_faint);
				fb.circle(
					cx + (ang.cos() * 6.0 * s) as i32,
					cy + (ang.sin() * 6.0 * s) as i32,
					(3.0 * s) as i32,
					t.accent,
				);
				crate::font::draw(
					fonts,
					fb,
					bx + 24.0 * s,
					by + box_h - 20.0 * s,
					"Esc — cancel",
					12.0 * s,
					t.text_dim,
					false,
					0.0,
				);
			}
			GameView::Active { session, ctrl, place_name, fps, .. } => {
				let st = session.state.lock().unwrap_or_else(|p| p.into_inner());
				let pad = 14.0 * s;
				let lines: Vec<(String, f32)> = vec![
					(place_name.clone(), 17.0 * s),
					(format!("state: {:?}", st.phase), 13.0 * s),
					(
						format!(
							"server: {}  job: {}…",
							st.server,
							st.job_id.chars().take(8).collect::<String>()
						),
						13.0 * s,
					),
					(
						format!(
							"replication: {} packets  zstd: {}  names: {}  ({})",
							st.replication,
							st.inflated,
							st.names,
							if st.repl_heads.is_empty() { "waiting…" } else { &st.repl_heads }
						),
						13.0 * s,
					),
					(
						format!(
							"ping: {}   fps: {:.0}   inst: {}   pkts: {} (ok {} / skip {})",
							st.ping_ms.map(|p| format!("{:.0} ms", p)).unwrap_or("-".into()),
							fps,
							st.instances,
							st.packets,
							st.decoded,
							st.skipped
						),
						13.0 * s,
					),
					(
						format!(
							"mode: {}   pos: {:.0}, {:.0}, {:.0}",
							match ctrl.mode {
								crate::game::ControlMode::Walk => "walk",
								crate::game::ControlMode::Fly => "fly (debug)",
							},
							ctrl.pos[0],
							ctrl.pos[1],
							ctrl.pos[2]
						),
						13.0 * s,
					),
				];
				let box_w = 430.0 * s;
				// Word-wrap every HUD line to the box; count the visual rows
				// so the panel height follows the content.
				let max_line_w = box_w - 24.0 * s;
				let wrapped: Vec<Vec<String>> = lines
					.iter()
					.map(|(l, size)| crate::font::wrap_text(fonts, l, max_line_w, *size, false))
					.collect();
				let box_h = 20.0 * s
					+ wrapped
						.iter()
						.zip(lines.iter())
						.map(|(segs, (_, size))| segs.len() as f32 * (*size + 8.0 * s))
						.sum::<f32>();
				fb.rect_rounded(pad as i32, pad as i32, box_w as i32, box_h as i32, 12, t.surface);
				let mut y = pad + 26.0 * s;
				for (i, (segs, (_, size))) in wrapped.iter().zip(lines.iter()).enumerate() {
					let color = if i == 0 { t.text } else { t.text_dim };
					for seg in segs {
						crate::font::draw(
							fonts,
							fb,
							pad + 12.0 * s,
							y,
							seg,
							*size,
							color,
							i == 0,
							0.0,
						);
						y += size + 8.0 * s;
					}
				}
				if cfg!(any(target_os = "android", target_os = "ios")) {
					// On-screen controls: joystick base + knob, +/− lift
					// buttons, "X" leave. Highlight the ones being touched.
					let (sc, sr, up_r, down_r, leave_r) = game_pad_layout(w as f32, h as f32, s);
					fb.rect_rounded(
						(sc.0 - sr) as i32,
						(sc.1 - sr) as i32,
						(sr * 2.0) as i32,
						(sr * 2.0) as i32,
						sr as i32,
						t.surface,
					);
					let kr = 40.0 * s;
					let kx = sc.0 + pad_state.stick_vec.0 * sr * 0.62;
					let ky = sc.1 - pad_state.stick_vec.1 * sr * 0.62;
					fb.rect_rounded(
						(kx - kr) as i32,
						(ky - kr) as i32,
						(kr * 2.0) as i32,
						(kr * 2.0) as i32,
						kr as i32,
						t.text_dim,
					);
					let held_up = pad_state.up.is_some();
					let held_down = pad_state.down.is_some();
					fb.rect_rounded(
						up_r.0,
						up_r.1,
						up_r.2,
						up_r.3,
						14,
						if held_up { t.text_dim } else { t.surface },
					);
					fb.rect_rounded(
						down_r.0,
						down_r.1,
						down_r.2,
						down_r.3,
						14,
						if held_down { t.text_dim } else { t.surface },
					);
					crate::font::draw(
						fonts,
						fb,
						up_r.0 as f32 + up_r.2 as f32 / 2.0 - 10.0 * s,
						up_r.1 as f32 + up_r.3 as f32 / 2.0 - 12.0 * s,
						"+",
						22.0 * s,
						if held_up { t.surface } else { t.text },
						true,
						0.0,
					);
					crate::font::draw(
						fonts,
						fb,
						down_r.0 as f32 + down_r.2 as f32 / 2.0 - 8.0 * s,
						down_r.1 as f32 + down_r.3 as f32 / 2.0 - 12.0 * s,
						"-",
						22.0 * s,
						if held_down { t.surface } else { t.text },
						true,
						0.0,
					);
					fb.rect_rounded(leave_r.0, leave_r.1, leave_r.2, leave_r.3, 12, t.surface);
					crate::font::draw(
						fonts,
						fb,
						leave_r.0 as f32 + leave_r.2 as f32 / 2.0 - 9.0 * s,
						leave_r.1 as f32 + leave_r.3 as f32 / 2.0 - 14.0 * s,
						"X",
						20.0 * s,
						t.text,
						true,
						0.0,
					);
					crate::font::draw(
						fonts,
						fb,
						pad + 12.0 * s,
						h as f32 - pad - 8.0 * s,
						"joystick — move   drag — look   + — jump   X — leave",
						12.0 * s,
						t.text_dim,
						false,
						0.0,
					);
				} else {
					crate::font::draw(
						fonts,
						fb,
						pad + 12.0 * s,
						h as f32 - pad - 8.0 * s,
						"Esc — release / leave   LMB — capture mouse   WASD — walk   Space — jump   F — fly",
						12.0 * s,
						t.text_dim,
						false,
						0.0,
					);
				}
			}
		}
	}
}
