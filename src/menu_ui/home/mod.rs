//! Home pane: the places search, the sectioned card grid and the
//! friends strip layout.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::model::{PlaceCard, PresenceKind};
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, FieldId, Hit, Rect};

mod cards;
pub(crate) use cards::*;

pub(crate) fn draw_home(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
	let s = ctx.scale;
	let t = theme::cur();
	let pad = (16.0 * s) as i32;
	let status_h = (theme::STATUSBAR_H * s) as i32;
	let mut y = (14.0 * s) as i32;

	// -- Places search row (fixed above the scroll area) --
	let field_h = (34.0 * s) as i32;
	let btn_w = (92.0 * s) as i32;
	let field_w =
		(w - 2 * pad - btn_w - (10.0 * s) as i32).max((120.0 * s) as i32).min((560.0 * s) as i32);
	v.search.draw(ctx, fb, x + pad, y, field_w, field_h, FieldId::Search, None);
	let btn_rect = draw_button(
		ctx,
		fb,
		Rect { x: x + pad + field_w + (10.0 * s) as i32, y, w: btn_w, h: field_h },
		if v.places_searching { "Searching\u{2026}" } else { "Search" },
		BtnStyle::Primary,
		v.places_searching,
	);
	ctx.hits.add(btn_rect, Hit::SearchPlacesBtn);
	if v.places_searching {
		ui::draw_spinner(
			fb,
			btn_rect.x as f32
				+ (btn_rect.w as f32
					- font::measure(ctx.fonts, "Searching\u{2026}", 14.0 * s, true))
					/ 2.0
				- 9.0 * s,
			btn_rect.y as f32 + btn_rect.h as f32 / 2.0,
			5.0 * s,
			ctx.now_ms,
			t.btn_text,
			s,
		);
	}
	y += field_h + (16.0 * s) as i32;

	// -- Everything below — the friends strip AND the place sections —
	// scrolls as one page: swiping the places list carries the friends
	// strip away with it (and back, when scrolled up again).
	draw_home_content(v, ctx, fb, x + pad, y, w - 2 * pad, h - status_h - (8.0 * s) as i32 - y);
}
/// One entry of the scrolling Home content: the friends strip, a section
/// header, an inline status message or a place card. Owned data (cards
/// cloned) so the layout pass can freely update the view's scroll metrics
/// while iterating.
pub(crate) enum HomeItem {
	/// "Friends" title + summary + horizontal strip (scrolls with the page).
	Friends,
	Header(String),
	/// Centered status message (loading / error / empty feed states).
	Message {
		text: String,
		spin: bool,
	},
	Card(PlaceCard),
}
pub(crate) fn draw_home_content(
	v: &mut MenuView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: i32,
	w: i32,
	h: i32,
) {
	let s = ctx.scale;
	let t = theme::cur();
	if h <= 0 {
		return;
	}

	let gap = (14.0 * s) as i32;
	let friends_h = (26.0 * s) as f32 + (theme::STRIP_H * s);

	// Build the item list: the friends section always leads; after it come
	// search results, or recently played + recommended, or a status message.
	let query = v.search.value.trim().to_string();
	let mut items: Vec<HomeItem> = vec![HomeItem::Friends];
	let mut have_sections = false;
	if query.is_empty() {
		if !v.recent.is_empty() {
			items.push(HomeItem::Header("Recently played".into()));
			for card in &v.recent {
				items.push(HomeItem::Card(card.clone()));
			}
			have_sections = true;
		}
		if v.app_settings.show_recommendations && !v.places.is_empty() {
			items.push(HomeItem::Header("Recommended for you".into()));
			for card in &v.places {
				items.push(HomeItem::Card(card.clone()));
			}
			have_sections = true;
		}
	} else if !v.places.is_empty() {
		items.push(HomeItem::Header("Results".into()));
		for card in &v.places {
			items.push(HomeItem::Card(card.clone()));
		}
		have_sections = true;
	}
	if !have_sections {
		let (msg, spin): (String, bool) = if v.places_searching {
			("Loading places\u{2026}".into(), true)
		} else if let Some(err) = &v.places_error {
			let mut m = format!("Could not load places: {}", err);
			m.truncate(140);
			(m, false)
		} else if !query.is_empty() {
			("No places found".into(), false)
		} else if !v.app_settings.show_recommendations {
			("Recommendations are off \u{2014} enable them in Settings".into(), false)
		} else {
			("No recommendations right now".into(), false)
		};
		items.push(HomeItem::Message { text: msg, spin });
	}

	let scroll_w = (theme::SCROLL_W * s) as i32;
	let grid_w = w - scroll_w - (4.0 * s) as i32;
	let card_w = ((theme::PLACE_CARD_W * s) as i32).min(grid_w);
	let cols = (((grid_w + gap) / (card_w + gap)).max(1)) as usize;
	let card_h = card_w + (88.0 * s) as i32;
	let header_h = (26.0 * s) as i32;
	let msg_h = (h as f32 - friends_h - gap as f32).max(120.0 * s);

	// Layout pass: y offset for every item + total content height.
	let mut offsets: Vec<f32> = Vec::with_capacity(items.len());
	let mut yy = 0.0f32;
	let mut col = 0usize;
	for it in &items {
		offsets.push(yy);
		match it {
			HomeItem::Friends => {
				yy += friends_h + gap as f32;
				col = 0;
			}
			HomeItem::Message { .. } => {
				yy += msg_h + gap as f32;
				col = 0;
			}
			HomeItem::Header(_) => {
				yy += (header_h + gap) as f32;
				col = 0;
			}
			HomeItem::Card(_) => {
				col += 1;
				if col == cols {
					yy += (card_h + gap) as f32;
					col = 0;
				}
			}
		}
	}
	let content_h = if col != 0 { yy + card_h as f32 } else { (yy - gap as f32).max(0.0) };

	v.places_scroll = clamp_scroll(v.places_scroll, content_h, h as f32);
	v.places_metrics = Some(ListMetrics { top: y as f32, h: h as f32, content_h });

	let clip = crate::draw::Clip { x0: x, y0: y, x1: x + w, y1: y + h };
	let old_clip = fb.clip();
	fb.set_clip(clip);
	for (i, it) in items.iter().enumerate() {
		let iy = y as f32 - v.places_scroll + offsets[i];
		if iy > y as f32 + h as f32 {
			break;
		}
		match it {
			HomeItem::Friends => {
				// Fully scrolled out of view: forget the strip rect so
				// wheel/drag routing never targets an off-screen strip.
				if iy + friends_h < y as f32 {
					v.strip_rect = None;
					continue;
				}
				font::draw(
					ctx.fonts,
					fb,
					x as f32,
					iy + 11.0 * s,
					"Friends",
					15.0 * s,
					t.text,
					true,
					0.0,
				);
				let online = v
					.rows
					.iter()
					.filter(|r| {
						r.presence.kind != PresenceKind::Offline
							&& r.presence.kind != PresenceKind::Invisible
					})
					.count();
				let ingame =
					v.rows.iter().filter(|r| r.presence.kind == PresenceKind::InGame).count();
				let summary = format!(
					"{} online \u{b7} {} in game \u{b7} {} friends",
					online,
					ingame,
					v.rows.len()
				);
				let sum_w = font::measure(ctx.fonts, &summary, 11.5 * s, false);
				font::draw(
					ctx.fonts,
					fb,
					(x + w) as f32 - sum_w,
					iy + 11.5 * s * 0.35 + 4.0 * s,
					&summary,
					11.5 * s,
					t.text_dim,
					false,
					0.0,
				);
				let strip_y = (iy + 26.0 * s) as i32;
				draw_friends_strip(v, ctx, fb, x, strip_y, w, (theme::STRIP_H * s) as i32);
			}
			HomeItem::Message { text, spin } => {
				if iy + msg_h < y as f32 {
					continue;
				}
				// Wrap long messages (hints, errors) instead of letting them
				// clip at the pane edge on narrow screens.
				let max_w = (w as f32 - 32.0 * s).max(80.0);
				let cx = x as f32 + w as f32 / 2.0;
				let my = iy + msg_h * 0.5;
				font::draw_wrapped_centered(
					ctx.fonts,
					fb,
					cx,
					my,
					max_w,
					text,
					14.0 * s,
					t.text_faint,
					false,
				);
				if *spin {
					let first_w = font::measure(ctx.fonts, text, 14.0 * s, false).min(max_w);
					ui::draw_spinner(
						fb,
						cx - first_w / 2.0 - 18.0 * s,
						my - 5.0 * s,
						7.0 * s,
						ctx.now_ms,
						t.accent,
						s,
					);
				}
			}
			HomeItem::Header(text) => {
				if iy + (header_h as f32) < y as f32 {
					continue;
				}
				font::draw(
					ctx.fonts,
					fb,
					x as f32,
					iy + 15.0 * s,
					text,
					15.0 * s,
					t.text,
					true,
					0.0,
				);
			}
			HomeItem::Card(card) => {
				if iy + (card_h as f32) < y as f32 {
					continue;
				}
				let cx = x + ((col_of(&items, i) as i32) % (cols as i32)) * (card_w + gap);
				draw_place_card(ctx, fb, card, cx, iy as i32, card_w, card_h, s);
			}
		}
	}
	fb.set_clip(old_clip);

	if content_h > h as f32 + 0.5 {
		draw_scrollbar(
			ctx,
			fb,
			x + w - scroll_w + scroll_w / 2,
			y,
			scroll_w,
			h,
			v.places_scroll,
			content_h,
			h as f32,
			s,
		);
	}
}
/// Number of cards since the last header (the column within the section;
/// take `% cols` when converting to an x offset).
pub(crate) fn col_of(items: &[HomeItem], i: usize) -> usize {
	let mut col = 0usize;
	for it in items.iter().take(i) {
		match it {
			HomeItem::Header(_) | HomeItem::Friends | HomeItem::Message { .. } => col = 0,
			HomeItem::Card(_) => col += 1,
		}
	}
	col
}
