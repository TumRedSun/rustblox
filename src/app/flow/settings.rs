//! Settings/appearance/logs button actions, split out of `handle_button`.

use crate::app::state::Screen;
use crate::app::App;
use crate::config;
use crate::menu_ui::SettingsTab;
use crate::theme;
use crate::ui::Hit;
use std::time::{Duration, Instant};

impl App {
	pub(super) fn settings_hit(&mut self, hit: &Hit) {
		match hit {
			Hit::ThemePreset(idx) => {
				if let Screen::Menu(v) = &mut self.screen {
					let has_custom = v.app_settings.custom.is_some();
					let ids = ["dark", "light", "gruvbox", "nord", "custom"];
					if let Some(id) = ids.get(*idx as usize) {
						if *id == "custom" && !has_custom {
							return;
						}
						v.app_settings.theme = (*id).to_string();
						theme::apply(theme::by_id(id, v.app_settings.custom));
						config::save_settings(&v.app_settings);
					}
				}
				self.needs_redraw = true;
			}
			Hit::ToggleRecommendations => {
				if let Screen::Menu(v) = &mut self.screen {
					v.app_settings.show_recommendations = !v.app_settings.show_recommendations;
					config::save_settings(&v.app_settings);
				}
				self.needs_redraw = true;
			}
			Hit::SettingsTab(i) => {
				if let Screen::Menu(v) = &mut self.screen {
					v.settings.tab = match *i {
						1 => SettingsTab::Appearance,
						2 => SettingsTab::Logs,
						_ => SettingsTab::General,
					};
					// The Logs pane opens pinned to the newest line.
					v.settings.logs_follow = v.settings.tab == SettingsTab::Logs;
					v.settings_scroll = 0.0;
				}
				self.focus = None;
				self.needs_redraw = true;
			}
			Hit::RadiusUp | Hit::RadiusDown => {
				if let Screen::Menu(v) = &mut self.screen {
					if v.app_settings.custom.is_none() {
						v.app_settings.custom = Some(theme::cur());
						v.app_settings.theme = "custom".into();
					}
					let delta = if matches!(hit, Hit::RadiusUp) { 0.25 } else { -0.25 };
					if let Some(c) = v.app_settings.custom.as_mut() {
						c.radius_scale = (c.radius_scale + delta).clamp(0.0, 2.5);
						theme::apply(*c);
					}
					config::save_settings(&v.app_settings);
				}
				self.needs_redraw = true;
			}
			Hit::ColorPrev(i) | Hit::ColorNext(i) => {
				let dir = if matches!(hit, Hit::ColorNext(_)) { 1 } else { -1 };
				if let Screen::Menu(v) = &mut self.screen {
					if v.app_settings.custom.is_none() {
						v.app_settings.custom = Some(theme::cur());
						v.app_settings.theme = "custom".into();
					}
					if let Some(c) = v.app_settings.custom.as_mut() {
						let cur = crate::menu_ui::theme_field(*c, *i);
						let next = crate::menu_ui::cycle_color(cur, dir);
						*c = crate::menu_ui::set_theme_slot(*c, *i, next);
						theme::apply(*c);
					}
					config::save_settings(&v.app_settings);
				}
				self.needs_redraw = true;
			}
			Hit::ResetAppearance => {
				if let Screen::Menu(v) = &mut self.screen {
					v.app_settings.custom = Some(theme::DARK);
					v.app_settings.theme = "custom".into();
					theme::apply(theme::DARK);
					config::save_settings(&v.app_settings);
				}
				self.needs_redraw = true;
			}
			_ => {}
		}
	}

	pub(super) fn logs_hit(&mut self, hit: &Hit) {
		match hit {
			Hit::CopyLogs => {
				let text = crate::logs::text();
				let n = crate::logs::len();
				let ok = self.copy_text_to_clipboard(&text);
				if let Screen::Menu(v) = &mut self.screen {
					v.notice = Some((
						if ok {
							format!("Logs copied to the clipboard ({} lines)", n)
						} else {
							"Could not access the clipboard".into()
						},
						Instant::now() + Duration::from_secs(4),
					));
				}
				self.needs_redraw = true;
			}
			Hit::SaveLogs => {
				let saved = crate::logs::export_logs();
				if let Screen::Menu(v) = &mut self.screen {
					v.notice = Some((
						match &saved {
							Some(name) => format!("Log saved to Downloads as {name}"),
							None => "Could not save the log (no Downloads backend)".into(),
						},
						Instant::now() + Duration::from_secs(4),
					));
				}
				self.needs_redraw = true;
			}
			Hit::ClearLogs => {
				crate::logs::clear();
				if let Screen::Menu(v) = &mut self.screen {
					v.settings.logs_follow = true;
					v.settings_scroll = 0.0;
				}
				self.needs_redraw = true;
			}
			_ => {}
		}
	}
}
