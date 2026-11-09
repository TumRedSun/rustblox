//! Main menu: top bar with own profile, search, friends list with live
//! presence statuses, scrollbar and status bar.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
use crate::model::{avatar_placeholder_color, FriendRow, PresenceKind};
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, FieldId, Hit, Rect, TextField};
use std::time::Instant;

pub struct ListMetrics {
    pub list_top: f32,
    pub list_h: f32,
    pub content_h: f32,
}

pub struct MenuView {
    pub profile: crate::model::UserProfile,
    pub rows: Vec<FriendRow>,
    pub search: TextField,
    pub scroll: f32,
    pub last_update: Option<Instant>,
    pub next_refresh: Option<Instant>,
    pub refreshing: bool,
    pub error: Option<String>,
    pub view_metrics: Option<ListMetrics>,
}

impl MenuView {
    pub fn new(profile: crate::model::UserProfile) -> MenuView {
        MenuView {
            profile,
            rows: Vec::new(),
            search: TextField::new("Search friends (Ctrl+F)"),
            scroll: 0.0,
            last_update: None,
            next_refresh: None,
            refreshing: true,
            error: None,
            view_metrics: None,
        }
    }
}

pub fn draw(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB) {
    let s = ctx.scale;
    let w = fb.w;
    let h = fb.h;

    let topbar_h = (theme::TOPBAR_H * s) as i32;
    let search_h = (theme::SEARCHBAR_H * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;

    draw_topbar(v, ctx, fb, w, topbar_h);

    // --- Search row --------------------------------------------------------
    let search_y = topbar_h;
    fb.rect(0, search_y, w, search_h, theme::BG);
    let pad = (16.0 * s) as i32;
    let search_w = (320.0 * s) as i32;
    let field_h = (34.0 * s) as i32;
    let field_y = search_y + (search_h - field_h) / 2;
    v.search.draw(ctx, fb, pad, field_y, search_w, field_h, FieldId::Search, None);

    // Counters on the right.
    let online = v.rows.iter().filter(|r| r.presence.kind != PresenceKind::Offline && r.presence.kind != PresenceKind::Invisible).count();
    let ingame = v.rows.iter().filter(|r| r.presence.kind == PresenceKind::InGame).count();
    let summary = format!(
        "{} online \u{b7} {} in game \u{b7} {} friends",
        online,
        ingame,
        v.rows.len()
    );
    let sum_w = font::measure(ctx.fonts, &summary, 12.5 * s, false);
    font::draw(
        ctx.fonts, fb, (w - pad) as f32 - sum_w, search_y as f32 + search_h as f32 / 2.0 + 12.5 * s * 0.35,
        &summary, 12.5 * s, theme::TEXT_DIM, false, 0.0,
    );
    fb.hline(0, search_y + search_h, w, theme::BORDER);

    // --- Friends list -------------------------------------------------------
    let list_top = (search_y + search_h) as f32;
    let list_h = (h - status_h) as f32 - list_top;
    let query = v.search.value.trim().to_lowercase();
    let filtered: Vec<&FriendRow> = v.rows.iter().filter(|r| r.matches_filter(&query)).collect();

    let row_h = theme::ROW_H * s;
    let content_h = filtered.len() as f32 * row_h;
    let max_scroll = (content_h - list_h).max(0.0);
    v.scroll = v.scroll.min(max_scroll).max(0.0);
    v.view_metrics = Some(ListMetrics { list_top, list_h, content_h });

    let list_clip = crate::draw::Clip { x0: 0, y0: list_top as i32, x1: w, y1: (list_top + list_h) as i32 };
    let old_clip = fb.clip();
    fb.set_clip(list_clip);

    if filtered.is_empty() {
        fb.set_clip(old_clip);
        let msg = if v.refreshing && v.last_update.is_none() {
            "Loading friends\u{2026}"
        } else if !query.is_empty() {
            "No friends match your search"
        } else {
            "No friends yet"
        };
        let mw = font::measure(ctx.fonts, msg, 14.0 * s, false);
        font::draw(ctx.fonts, fb, w as f32 / 2.0 - mw / 2.0, list_top + list_h / 2.0, msg, 14.0 * s, theme::TEXT_FAINT, false, 0.0);
        if v.refreshing && v.last_update.is_none() {
            ui::draw_spinner(fb, w as f32 / 2.0 - mw / 2.0 - 18.0 * s, list_top + list_h / 2.0 - 5.0 * s, 7.0 * s, ctx.now_ms, theme::ACCENT, s);
        }
    } else {
        let first = (v.scroll / row_h) as usize;
        for (vi, row) in filtered.iter().enumerate().skip(first) {
            let y = list_top - v.scroll + vi as f32 * row_h;
            if y > list_top + list_h {
                break;
            }
            if y + row_h < list_top {
                continue;
            }
            draw_row(ctx, fb, row, 0.0, y, w as f32 - (12.0 * s), row_h, s);
        }
        // Scrollbar (on top of rows).
        if content_h > list_h {
            let track_w = (8.0 * s) as i32;
            let track_x = w - track_w;
            fb.rect(track_x, list_top as i32, track_w, list_h as i32, theme::SURFACE);
            let thumb_h = ((list_h / content_h) * list_h).max(24.0 * s);
            let scrollable = (content_h - list_h).max(0.001);
            let thumb_y = list_top + (v.scroll / scrollable) * (list_h - thumb_h);
            fb.rect_rounded(track_x + 1, thumb_y as i32, track_w - 2, thumb_h as i32, (3.0 * s) as i32, theme::BORDER);
            ctx.hits.add(
                Rect { x: track_x - (6.0 * s) as i32, y: list_top as i32, w: track_w + (12.0 * s) as i32, h: list_h as i32 },
                Hit::Scrollbar,
            );
        }
    }
    fb.set_clip(old_clip);

    // --- Status bar ----------------------------------------------------------
    let sb_y = h - status_h;
    fb.rect(0, sb_y, w, status_h, theme::SURFACE);
    fb.hline(0, sb_y, w, theme::BORDER);
    let left = if v.refreshing {
        "Updating\u{2026}".to_string()
    } else {
        match (v.last_update, v.next_refresh) {
            (Some(lu), Some(nr)) => {
                let secs = lu.elapsed().as_secs();
                let next = nr.saturating_duration_since(Instant::now()).as_secs();
                format!("Updated {}s ago \u{b7} next refresh in {}s", secs, next)
            }
            (Some(lu), None) => format!("Updated {}s ago", lu.elapsed().as_secs()),
            _ => String::new(),
        }
    };
    font::draw(ctx.fonts, fb, pad as f32, sb_y as f32 + status_h as f32 / 2.0 + 12.0 * s * 0.35, &left, 12.0 * s, theme::TEXT_FAINT, false, 0.0);
    if v.refreshing {
        ui::draw_spinner(fb, pad as f32 + font::measure(ctx.fonts, &left, 12.0 * s, false) + 14.0 * s, sb_y as f32 + status_h as f32 / 2.0 - 1.0 * s, 6.0 * s, ctx.now_ms, theme::ACCENT, s);
    }
    if let Some(err) = &v.error {
        let ew = (w as f32 - font::measure(ctx.fonts, &left, 12.0 * s, false) - 60.0 * s).max(0.0);
        font::draw_ellipsis(
            ctx.fonts, fb, (w - pad) as f32 - ew.min(w as f32 - 200.0 * s), sb_y as f32 + status_h as f32 / 2.0 + 12.0 * s * 0.35,
            &format!("Error: {}", err), 12.0 * s, theme::RED, false, w as f32 - 220.0 * s,
        );
    }
}

fn draw_topbar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, w: i32, topbar_h: i32) {
    let s = ctx.scale;
    fb.rect(0, 0, w, topbar_h, theme::SURFACE);

    let pad = (16.0 * s) as i32;

    // Logo.
    let sq = (20.0 * s) as i32;
    fb.rect_rounded(pad, (topbar_h - sq) / 2, sq, sq, (5.0 * s) as i32, theme::RED);
    let r_w = font::measure(ctx.fonts, "R", 12.0 * s, true);
    font::draw(
        ctx.fonts, fb, pad as f32 + (sq as f32 - r_w) * 0.5, (topbar_h as f32) / 2.0 + 12.0 * s * 0.35,
        "R", 12.0 * s, theme::WHITE, true, 0.0,
    );
    font::draw(
        ctx.fonts, fb, (pad + sq + (8.0 * s) as i32) as f32, topbar_h as f32 / 2.0 + 12.0 * s * 0.35,
        "RUSTBLOX", 12.0 * s, theme::TEXT, true, 1.0 * s,
    );

    // Own profile.
    let px = pad + sq + (8.0 * s) as i32 + (110.0 * s) as i32;
    let avatar_sz = (34.0 * s) as i32;
    let avatar_y = (topbar_h - avatar_sz) / 2;
    draw_avatar(ctx, fb, u64::MAX, &v.profile.display_name, px, avatar_y, avatar_sz);
    let name_x = px + avatar_sz + (10.0 * s) as i32;
    font::draw_ellipsis(
        ctx.fonts, fb, name_x as f32, topbar_h as f32 / 2.0 - 2.0 * s,
        &v.profile.display_name, 14.0 * s, theme::TEXT, true, 200.0 * s,
    );
    font::draw_ellipsis(
        ctx.fonts, fb, name_x as f32, topbar_h as f32 / 2.0 + 14.0 * s,
        &format!("@{}", v.profile.name), 12.0 * s, theme::TEXT_DIM, false, 200.0 * s,
    );

    // Buttons on the right.
    let btn_w = (110.0 * s) as i32;
    let btn_h = (32.0 * s) as i32;
    let by = (topbar_h - btn_h) / 2;
    let refresh_label = if v.refreshing { "Updating\u{2026}" } else { "Refresh" };
    let refresh = draw_button(
        ctx, fb,
        Rect { x: w - pad - btn_w * 2 - (10.0 * s) as i32, y: by, w: btn_w, h: btn_h },
        refresh_label, BtnStyle::Secondary, v.refreshing,
    );
    ctx.hits.add(refresh, Hit::Refresh);
    if v.refreshing {
        ui::draw_spinner(fb, refresh.x as f32 + 16.0 * s, refresh.y as f32 + refresh.h as f32 / 2.0, 6.0 * s, ctx.now_ms, theme::TEXT, s);
    }
    let logout = draw_button(
        ctx, fb,
        Rect { x: w - pad - btn_w, y: by, w: btn_w, h: btn_h },
        "Sign out", BtnStyle::Secondary, false,
    );
    ctx.hits.add(logout, Hit::Logout);
}

fn draw_avatar(ctx: &mut UiCtx, fb: &mut FB, id: u64, display_name: &str, x: i32, y: i32, size: i32) {
    let radius = (size as f32 * 0.18) as i32;
    match ctx.avatars.get(&id) {
        Some(rgba) => {
            fb.image(x, y, rgba, size, size, radius);
        }
        None => {
            let color = avatar_placeholder_color(if id == u64::MAX { 0 } else { id });
            fb.rect_rounded(x, y, size, size, radius, color);
            let initial = display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
            let iw = font::measure(ctx.fonts, &initial, size as f32 * 0.42, true);
            font::draw(
                ctx.fonts, fb, x as f32 + (size as f32 - iw) / 2.0,
                y as f32 + size as f32 / 2.0 + size as f32 * 0.15,
                &initial, size as f32 * 0.42, theme::WHITE, true, 0.0,
            );
        }
    }
}

fn draw_row(ctx: &mut UiCtx, fb: &mut FB, row: &FriendRow, x: f32, y: f32, w: f32, h: f32, s: f32) {
    // Hover highlight.
    let (cx, cy) = ctx.cursor;
    let hovered = cx >= x && cx < x + w && cy >= y && cy < y + h;
    if hovered {
        fb.rect(x as i32, y as i32, w as i32, h as i32, theme::SURFACE_2);
    }

    let pad = (14.0 * s) as i32;
    let avatar_sz = (theme::AVATAR_SIZE as f32 * s) as i32;
    let avatar_y = y as i32 + ((h as i32) - avatar_sz) / 2;

    draw_avatar(ctx, fb, row.friend.id, &row.friend.display_name, pad, avatar_y, avatar_sz);

    let text_x = (pad + avatar_sz + (12.0 * s) as i32) as f32;
    let max_text_w = w - (text_x - x) - (20.0 * s);

    // Line 1: display name + @username.
    let name_w = font::measure(ctx.fonts, &row.friend.display_name, 14.5 * s, true);
    font::draw(ctx.fonts, fb, text_x, y + 26.0 * s, &row.friend.display_name, 14.5 * s, theme::TEXT, true, 0.0);
    if name_w + 8.0 * s < max_text_w {
        font::draw_ellipsis(
            ctx.fonts, fb, text_x + name_w + 8.0 * s, y + 26.0 * s,
            &format!("@{}{}", row.friend.name, if row.friend.is_banned { " (banned)" } else { "" }),
            12.5 * s, if row.friend.is_banned { theme::RED } else { theme::TEXT_DIM }, false,
            max_text_w - name_w - 8.0 * s,
        );
    } else {
        font::draw_ellipsis(ctx.fonts, fb, text_x, y + 26.0 * s, &row.friend.display_name, 14.5 * s, theme::TEXT, true, max_text_w);
    }

    // Line 2: status dot + label.
    let dot_d = (9.0 * s) as i32;
    let dot_y = (y + 44.0 * s) as i32;
    let kind = row.presence.kind;
    ui::draw_dot(fb, text_x as i32, dot_y, dot_d, kind.dot_color());
    let label = row.status_label();
    let label_color = match kind {
        PresenceKind::Online => theme::GREEN,
        PresenceKind::InGame => theme::BLUE,
        PresenceKind::Studio => theme::ORANGE,
        PresenceKind::Offline | PresenceKind::Invisible => theme::TEXT_DIM,
    };
    font::draw_ellipsis(
        ctx.fonts, fb, text_x + dot_d as f32 + 7.0 * s, y + 48.0 * s,
        &label, 12.5 * s, label_color, false,
        max_text_w - dot_d as f32 - 7.0 * s,
    );

    // Row separator.
    fb.hline(x as i32, (y + h) as i32, w as i32, [0x1E, 0x20, 0x22, 0xFF]);
}
