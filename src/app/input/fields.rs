//! Focused-field lookup and clipboard helpers.

use crate::app::state::Screen;
use crate::app::App;
use crate::menu_ui::{FriendPane, MenuTab};
use crate::ui::FieldId;

impl App {
	pub(in crate::app) fn focused_field_mut(&mut self) -> Option<&mut crate::ui::TextField> {
		let focus = self.focus?;
		match &mut self.screen {
			Screen::Login(v) => match focus {
				FieldId::Username => Some(&mut v.username),
				FieldId::Password => Some(&mut v.password),
				FieldId::Cookie => Some(&mut v.cookie),
				FieldId::Code => Some(&mut v.code),
				FieldId::Search | FieldId::AddSearch => None,
			},
			Screen::Menu(v) => match (focus, v.tab, v.friend_pane) {
				(FieldId::Search, MenuTab::Home, _) => Some(&mut v.search),
				(FieldId::AddSearch, MenuTab::Friends, FriendPane::Search) => {
					Some(&mut v.add.query)
				}
				_ => None,
			},
			Screen::Game(_) => None,
		}
	}

	pub(in crate::app) fn paste_clipboard(&mut self) {
		// Platform backend first (Android/iOS hooks); falls back to arboard
		// on desktop.
		let text = crate::platform::clipboard_get();
		#[cfg(not(any(target_os = "android", target_os = "ios")))]
		let text = text.or_else(|| {
			self.ensure_clipboard();
			self.clipboard.as_mut().and_then(|c| c.get_text().ok())
		});
		if let Some(text) = text.filter(|t| !t.is_empty()) {
			if let Some(f) = self.focused_field_mut() {
				f.insert(&text);
				self.needs_redraw = true;
			}
		}
	}

	pub(in crate::app) fn copy_text_to_clipboard(&mut self, text: &str) -> bool {
		if text.is_empty() {
			return false;
		}
		// Android/iOS hook (JNI UIPasteboard / ClipboardManager).
		if cfg!(any(target_os = "android", target_os = "ios")) {
			crate::platform::clipboard_set(text);
			return true;
		}
		#[cfg(not(any(target_os = "android", target_os = "ios")))]
		{
			self.ensure_clipboard();
			if let Some(c) = self.clipboard.as_mut() {
				return c.set_text(text).is_ok();
			}
		}
		false
	}

	#[cfg(not(any(target_os = "android", target_os = "ios")))]
	pub(in crate::app) fn ensure_clipboard(&mut self) {
		if self.clipboard.is_none() {
			self.clipboard = arboard::Clipboard::new().ok();
		}
	}
}
