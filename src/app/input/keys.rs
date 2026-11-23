//! Keyboard routing: shortcuts, focus cycling and text-field editing.

use crate::app::state::Screen;
use crate::app::App;
use crate::menu_ui::{FriendPane, MenuTab};
use crate::ui::{FieldId, Hit};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

impl App {
	fn focus_cycle(&mut self) {
		let order: &[FieldId] = match &self.screen {
			Screen::Login(v) => {
				if v.two_step.is_some() {
					&[FieldId::Code]
				} else {
					match v.tab {
						crate::login_ui::LoginTab::Password => {
							&[FieldId::Username, FieldId::Password]
						}
						crate::login_ui::LoginTab::Cookie => &[FieldId::Cookie],
					}
				}
			}
			Screen::Menu(v) => match (v.tab, v.friend_pane) {
				(MenuTab::Home, _) => &[FieldId::Search],
				(MenuTab::Friends, FriendPane::Search) => &[FieldId::AddSearch],
				_ => &[],
			},
			Screen::Game(_) => &[],
		};
		match self.focus {
			None => self.focus = order.first().copied(),
			Some(cur) => {
				let idx = order.iter().position(|f| *f == cur).unwrap_or(0);
				self.focus = order.get((idx + 1) % order.len()).copied();
			}
		}
		self.request_redraw();
	}

	pub(in crate::app) fn on_key(&mut self, event: KeyEvent) {
		let ctrl = self.mods.contains(ModifiersState::CONTROL);
		// Global shortcuts first.
		if ctrl && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("v")) {
			self.paste_clipboard();
			return;
		}
		if ctrl && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("a")) {
			if let Some(f) = self.focused_field_mut() {
				f.select_all();
				self.request_redraw();
			}
			return;
		}
		if ctrl && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("f")) {
			if matches!(self.screen, Screen::Menu(_)) {
				self.focus = Some(match &self.screen {
					Screen::Menu(v)
						if v.tab == MenuTab::Friends && v.friend_pane == FriendPane::Search =>
					{
						FieldId::AddSearch
					}
					Screen::Menu(v) if v.tab == MenuTab::Friends => {
						if let Screen::Menu(v2) = &mut self.screen {
							v2.friend_pane = FriendPane::Search;
						}
						FieldId::AddSearch
					}
					_ => FieldId::Search,
				});
				self.request_redraw();
			}
			return;
		}

		match &event.logical_key {
			Key::Named(NamedKey::Tab) => {
				self.focus_cycle();
				return;
			}
			Key::Named(NamedKey::Enter) => {
				self.primary_action();
				return;
			}
			Key::Named(NamedKey::Escape) => {
				match &mut self.screen {
					Screen::Login(v) => {
						if v.two_step.is_some() {
							v.two_step = None;
							v.status = None;
							v.busy = false;
							self.focus = Some(FieldId::Password);
						} else {
							self.focus = None;
						}
					}
					Screen::Menu(v) => {
						// Esc: back out one level (search pane -> requests ->
						// home), otherwise drop focus.
						if v.tab == MenuTab::Friends && v.friend_pane == FriendPane::Search {
							v.friend_pane = FriendPane::Requests;
							self.focus = None;
						} else if v.tab != MenuTab::Home {
							v.tab = MenuTab::Home;
							self.focus = None;
						} else {
							self.focus = None;
						}
					}
					Screen::Game(_) => {
						self.leave_game(None);
					}
				}
				self.request_redraw();
				return;
			}
			Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) => {
				// Keyboard scrolling of the active list.
				let dir = if matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp)) {
					Hit::ScrollUp
				} else {
					Hit::ScrollDown
				};
				if matches!(self.screen, Screen::Menu(_)) {
					self.scroll_step(&dir);
				}
				return;
			}
			_ => {}
		}

		// Field editing keys.
		let mods = self.mods;
		let mut edited = false;
		if let Some(f) = self.focused_field_mut() {
			match &event.logical_key {
				Key::Named(NamedKey::Backspace) => {
					f.backspace();
					edited = true;
				}
				Key::Named(NamedKey::Delete) => {
					f.delete();
					edited = true;
				}
				Key::Named(NamedKey::ArrowLeft) => {
					f.left();
					edited = true;
				}
				Key::Named(NamedKey::ArrowRight) => {
					f.right();
					edited = true;
				}
				Key::Named(NamedKey::Home) => {
					f.home();
					edited = true;
				}
				Key::Named(NamedKey::End) => {
					f.end();
					edited = true;
				}
				Key::Named(NamedKey::Space) => {
					if char_input_allowed(mods) {
						f.insert(" ");
						edited = true;
					}
				}
				Key::Character(text) => {
					// Plain text entry (letters, digits, punctuation,
					// Cyrillic…). Shortcut combos are filtered by
					// char_input_allowed; Ctrl+V/A were already handled above.
					if char_input_allowed(mods) {
						f.insert(text);
						edited = true;
					}
				}
				_ => {}
			}
		}
		if edited {
			self.request_redraw();
		}
	}
}

/// Whether a `Key::Character` press with these modifiers should insert text.
///
/// * No modifiers, or Shift only — insert.
/// * Ctrl or Alt alone — a shortcut, do not insert (Ctrl+V/A are handled
///   earlier in [`App::on_key`]).
/// * Ctrl+Alt together — that is AltGr on Windows / European layouts, which
///   produces printable characters (`\`, `@`, `#`, `€`…), so insert.
/// * Super (Cmd/Win) — system shortcut, do not insert.
pub fn char_input_allowed(mods: ModifiersState) -> bool {
	let ctrl = mods.contains(ModifiersState::CONTROL);
	let alt = mods.contains(ModifiersState::ALT);
	let sup = mods.contains(ModifiersState::SUPER);
	if sup {
		return false;
	}
	// Exactly one of ctrl/alt => shortcut. Both (or neither) => text.
	!(ctrl ^ alt)
}
