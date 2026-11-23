//! Main menu, Roblox-style: a collapsible left sidebar (the rotated red logo
//! tile doubles as the "R" of the wordmark and clicking it refreshes + jumps
//! to the top of Home), a horizontal friends strip, a places search bar and
//! a sectioned card grid (Recently played + Recommended). The Friends tab
//! opens on incoming friend requests (accept / decline) with a search pane
//! next to it; the Settings tab hosts account switching, themes and the
//! appearance editor.

use crate::app::UiCtx;
use crate::config::Session;
use crate::draw::FB;
use crate::font;
use crate::model::{FriendRow, PlaceCard};
use crate::theme;
use crate::ui::{self, Rect, TextField};
use std::time::Instant;

mod common;
mod friends;
mod home;
mod settings;
mod sidebar;
mod state;

pub use common::*;
pub(crate) use friends::*;
pub(crate) use home::*;
pub(crate) use settings::*;
pub(crate) use sidebar::*;
pub use state::*;

pub struct MenuView {
	pub profile: crate::model::UserProfile,
	pub rows: Vec<FriendRow>,
	/// Sidebar collapsed to the icon-only mini mode.
	pub collapsed: bool,
	pub tab: MenuTab,
	/// Persisted app settings (theme, recommendations toggle, …).
	pub app_settings: crate::config::Settings,
	/// Saved accounts for the quick switcher (includes the active one).
	pub accounts: Vec<Session>,
	// -- Home: places search + sectioned card grid --
	/// Places search box (top of the Home pane).
	pub search: TextField,
	/// Search results (query non-empty) or the recommendation feed.
	pub places: Vec<PlaceCard>,
	pub places_searching: bool,
	pub places_error: Option<String>,
	/// The query the current `places` belong to ("" = recommendation feed).
	pub places_done_for: String,
	pub places_scroll: f32,
	pub places_metrics: Option<ListMetrics>,
	/// Recently played places (official "continue" row + local tracking).
	pub recent: Vec<PlaceCard>,
	// -- Home: horizontal friends strip --
	pub strip_scroll: f32,
	pub strip_metrics: Option<ListMetrics>,
	/// Physical-pixel rect of the strip (mouse-wheel routing).
	pub strip_rect: Option<Rect>,
	// -- Friends tab --
	pub friend_pane: FriendPane,
	pub requests: RequestsState,
	pub requests_scroll: f32,
	pub requests_metrics: Option<ListMetrics>,
	pub add: AddFriendsState,
	pub add_scroll: f32,
	pub add_metrics: Option<ListMetrics>,
	// -- Settings tab --
	pub settings: SettingsState,
	pub settings_scroll: f32,
	pub settings_metrics: Option<ListMetrics>,
	// -- shared --
	pub last_update: Option<Instant>,
	pub next_refresh: Option<Instant>,
	pub refreshing: bool,
	pub error: Option<String>,
	/// Transient status-bar note ("text", shown-until).
	pub notice: Option<(String, Instant)>,
}
impl MenuView {
	/// After a manual scroll change on the Settings screen: keep the Logs
	/// pane pinned to the newest line only while the user sits at the bottom.
	pub fn settings_sync_logs_follow(&mut self) {
		if self.settings.tab != SettingsTab::Logs {
			return;
		}
		let at_bottom = self
			.settings_metrics
			.as_ref()
			.map(|m| self.settings_scroll + m.h >= m.content_h - 1.0)
			.unwrap_or(false);
		self.settings.logs_follow = at_bottom;
	}

	pub fn new(profile: crate::model::UserProfile) -> MenuView {
		MenuView {
			profile,
			rows: Vec::new(),
			collapsed: false,
			tab: MenuTab::Home,
			app_settings: crate::config::load_settings(),
			accounts: crate::config::load_accounts(),
			search: TextField::new("Search places by name"),
			places: Vec::new(),
			places_searching: true,
			places_error: None,
			places_done_for: String::new(),
			places_scroll: 0.0,
			places_metrics: None,
			recent: Vec::new(),
			strip_scroll: 0.0,
			strip_metrics: None,
			strip_rect: None,
			friend_pane: FriendPane::Requests,
			requests: RequestsState::new(),
			requests_scroll: 0.0,
			requests_metrics: None,
			add: AddFriendsState::new(),
			add_scroll: 0.0,
			add_metrics: None,
			settings: SettingsState { tab: SettingsTab::General, logs_follow: true },
			settings_scroll: 0.0,
			settings_metrics: None,
			last_update: None,
			next_refresh: None,
			refreshing: true,
			error: None,
			notice: None,
		}
	}
}
pub fn draw(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB) {
	let s = ctx.scale;
	let w = fb.w;
	let h = fb.h;
	let sb_w =
		((if v.collapsed { theme::SIDEBAR_W_MINI } else { theme::SIDEBAR_W_FULL }) * s) as i32;

	draw_sidebar(v, ctx, fb, sb_w, h);

	// Main pane.
	let mx = sb_w;
	let mw = w - sb_w;
	let t = theme::cur();
	fb.rect(mx, 0, mw, h, t.bg);
	match v.tab {
		MenuTab::Home => draw_home(v, ctx, fb, mx, mw, h),
		MenuTab::Friends => draw_friends(v, ctx, fb, mx, mw, h),
		MenuTab::Settings => draw_settings(v, ctx, fb, mx, mw, h),
	}

	let sb_y = h - (theme::STATUSBAR_H * s) as i32;
	draw_status_bar(v, ctx, fb, mx, mw, sb_y, theme::STATUSBAR_H * s);
}

// ---------------------------------------------------------------------------
// Sidebar (collapsible: full labels <-> icon-only mini)
// ---------------------------------------------------------------------------
fn draw_status_bar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, y: i32, h: f32) {
	let s = ctx.scale;
	let t = theme::cur();
	fb.rect(x, y, w, h as i32, t.surface);
	fb.hline(x, y, w, t.border);
	let pad = (16.0 * s) as i32;

	// Transient notice wins over the "updated ago" label.
	if matches!(&v.notice, Some((_, until)) if Instant::now() >= *until) {
		v.notice = None;
	}
	let notice_active = matches!(&v.notice, Some((_, until)) if Instant::now() < *until);
	let left = if notice_active {
		v.notice.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
	} else {
		refresh_label(v)
	};
	let notice_color = if notice_active { t.green } else { t.text_faint };
	// Long notices (e.g. join refusals) get ellipsized: the status bar is a
	// single line; the full text lives in Settings → Logs, wrapped.
	let notice_max_w = w as f32 - 2.0 * pad as f32 - 250.0 * s;
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		(x + pad) as f32,
		y as f32 + h / 2.0 + 12.0 * s * 0.35,
		&left,
		12.0 * s,
		notice_color,
		false,
		notice_max_w.max(80.0),
	);
	if v.refreshing && !notice_active {
		ui::draw_spinner(
			fb,
			(x + pad) as f32 + font::measure(ctx.fonts, &left, 12.0 * s, false) + 14.0 * s,
			y as f32 + h / 2.0 - 1.0 * s,
			6.0 * s,
			ctx.now_ms,
			t.accent,
			s,
		);
	}
	if let Some(err) = &v.error {
		let ew = (w as f32 - font::measure(ctx.fonts, &left, 12.0 * s, false) - 60.0 * s).max(0.0);
		font::draw_ellipsis(
			ctx.fonts,
			fb,
			(x + w - pad) as f32 - ew.min(w as f32 - 220.0 * s),
			y as f32 + h / 2.0 + 12.0 * s * 0.35,
			&format!("Error: {}", err),
			12.0 * s,
			t.red,
			false,
			w as f32 - 220.0 * s,
		);
	}
}
fn refresh_label(v: &MenuView) -> String {
	if v.refreshing {
		return "Updating\u{2026}".to_string();
	}
	match v.last_update {
		Some(lu) => format!("Updated {}s ago", lu.elapsed().as_secs()),
		None => String::new(),
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
