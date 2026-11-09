//! Main menu, Roblox-style: left sidebar (rotated logo tile + wordmark,
//! profile chip, Home / Add Friends navigation, sign out), a vertical
//! friends list with an arrow scrollbar, live presence statuses, a reserved
//! "Places" strip for future place cards, and a status bar with the
//! auto-refresh countdown.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
use crate::model::{avatar_placeholder_color, FriendRow, PresenceKind, UserEntry};
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, FieldId, Hit, Rect, TextField};
use std::collections::HashMap;
use std::time::Instant;

/// Which main-area pane is open (driven by the sidebar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTab {
    Home,
    AddFriends,
}

pub struct ListMetrics {
    pub top: f32,
    pub h: f32,
    pub content_h: f32,
}

/// Clamp a scroll offset into `[0, content - view]`.
pub fn clamp_scroll(scroll: f32, content_h: f32, view_h: f32) -> f32 {
    scroll.max(0.0).min((content_h - view_h).max(0.0))
}

/// Delivery state of a friend request for a search-result row.
#[derive(Clone, Debug, PartialEq)]
pub enum RequestState {
    Idle,
    Pending,
    Sent,
    Failed(String),
}

/// State of the "Add friends" pane (search + request sending).
pub struct AddFriendsState {
    pub query: TextField,
    pub results: Vec<UserEntry>,
    pub searching: bool,
    pub searched_once: bool,
    pub error: Option<String>,
    pub pending: Vec<u64>,
    pub sent: Vec<u64>,
    pub failed: HashMap<u64, String>,
}

impl AddFriendsState {
    fn new() -> Self {
        AddFriendsState {
            query: TextField::new("Search users by username"),
            results: Vec::new(),
            searching: false,
            searched_once: false,
            error: None,
            pending: Vec::new(),
            sent: Vec::new(),
            failed: HashMap::new(),
        }
    }

    pub fn mark_pending(&mut self, id: u64) {
        if !self.pending.contains(&id) {
            self.pending.push(id);
        }
    }

    pub fn state_of(&self, id: u64) -> RequestState {
        if self.pending.contains(&id) {
            return RequestState::Pending;
        }
        if self.sent.contains(&id) {
            return RequestState::Sent;
        }
        match self.failed.get(&id) {
            Some(msg) => RequestState::Failed(msg.clone()),
            None => RequestState::Idle,
        }
    }
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
    pub tab: MenuTab,
    pub add: AddFriendsState,
    pub add_scroll: f32,
    pub add_metrics: Option<ListMetrics>,
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
            tab: MenuTab::Home,
            add: AddFriendsState::new(),
            add_scroll: 0.0,
            add_metrics: None,
        }
    }
}

pub fn draw(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB) {
    let s = ctx.scale;
    let w = fb.w;
    let h = fb.h;
    let sb_w = (theme::SIDEBAR_W * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;

    draw_sidebar(v, ctx, fb, sb_w, h);

    // Main pane.
    let mx = sb_w;
    let mw = w - sb_w;
    fb.rect(mx, 0, mw, h, theme::BG);
    match v.tab {
        MenuTab::Home => draw_home(v, ctx, fb, mx, mw, h),
        MenuTab::AddFriends => draw_add_friends(v, ctx, fb, mx, mw, h),
    }

    let sb_y = h - status_h;
    draw_status_bar(v, ctx, fb, mx, mw, sb_y, status_h);
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

fn draw_sidebar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, w: i32, h: i32) {
    let s = ctx.scale;
    fb.rect(0, 0, w, h, theme::SURFACE);
    fb.rect(w - 1, 0, 1, h, [0x1E, 0x20, 0x22, 0xFF]);

    let pad = (16.0 * s) as i32;

    // Logo: slightly rotated red tile with a white R + "RustBlox" wordmark
    // whose leading R is red.
    let sq = (30.0 * s) as i32;
    let cx = pad + sq / 2 + 1;
    let cy = (14.0 * s) as i32 + sq / 2;
    fb.rect_rounded_rot(cx, cy, sq, sq, (7.0 * s) as i32, -0.16, theme::RED);
    let ir = 15.0 * s;
    let ir_w = font::measure(ctx.fonts, "R", ir, true);
    font::draw(ctx.fonts, fb, cx as f32 - ir_w / 2.0, cy as f32 + ir * 0.35, "R", ir, theme::WHITE, true, 0.0);
    let wx = (pad + sq + (9.0 * s) as i32) as f32;
    let wbase = cy as f32 + 16.0 * s * 0.35;
    let rw = font::measure(ctx.fonts, "R", 16.5 * s, true);
    font::draw(ctx.fonts, fb, wx, wbase, "R", 16.5 * s, theme::RED, true, 0.0);
    font::draw(ctx.fonts, fb, wx + rw, wbase, "ustBlox", 16.5 * s, theme::TEXT, true, 0.0);

    // Profile chip.
    let chip_y = cy + sq / 2 + (14.0 * s) as i32;
    let avatar_sz = (38.0 * s) as i32;
    draw_avatar(ctx, fb, v.profile.id, &v.profile.display_name, pad, chip_y, avatar_sz);
    let nx = (pad + avatar_sz + (10.0 * s) as i32) as f32;
    let name_w = (w - pad) as f32 - nx - (12.0 * s);
    font::draw_ellipsis(
        ctx.fonts, fb, nx, chip_y as f32 + 14.0 * s,
        &v.profile.display_name, 13.5 * s, theme::TEXT, true, name_w,
    );
    font::draw_ellipsis(
        ctx.fonts, fb, nx, chip_y as f32 + 30.0 * s,
        &format!("@{}", v.profile.name), 11.5 * s, theme::TEXT_DIM, false, name_w,
    );

    // Navigation.
    let item_h = (38.0 * s) as i32;
    let item_w = w - 2 * pad;
    let mut ny = chip_y + avatar_sz + (16.0 * s) as i32;
    nav_item(
        ctx, fb, pad, ny, item_w, item_h,
        "Home", icon_home, v.tab == MenuTab::Home, true, Hit::NavHome, None,
    );
    ny += item_h + (6.0 * s) as i32;
    nav_item(
        ctx, fb, pad, ny, item_w, item_h,
        "Add Friends", icon_person_add, v.tab == MenuTab::AddFriends, true, Hit::NavAdd, None,
    );
    ny += item_h + (12.0 * s) as i32;
    fb.hline(pad, ny, item_w, theme::BORDER);
    ny += (12.0 * s) as i32;

    // Future sections (disabled placeholders).
    for (label, icon) in [
        ("Charts", icon_charts as fn(&mut FB, i32, i32, i32, theme::Rgba)),
        ("Avatar", icon_person),
        ("Chat", icon_chat),
    ] {
        nav_item(ctx, fb, pad, ny, item_w, item_h, label, icon, false, false, Hit::None, Some("Soon"));
        ny += item_h + (6.0 * s) as i32;
    }

    // Bottom: sign out + version.
    let btn_h = (32.0 * s) as i32;
    let by = h - btn_h - (14.0 * s) as i32 - (16.0 * s) as i32;
    let rect = draw_button(
        ctx, fb,
        Rect { x: pad, y: by, w: item_w, h: btn_h },
        "Sign out", BtnStyle::Secondary, false,
    );
    ctx.hits.add(rect, Hit::Logout);
    let ver = format!("RustBlox v{}", env!("CARGO_PKG_VERSION"));
    let vw = font::measure(ctx.fonts, &ver, 11.0 * s, false);
    font::draw(
        ctx.fonts, fb, (w - pad) as f32 - vw, (by + btn_h) as f32 + 12.0 * s + 11.0 * s * 0.35,
        &ver, 11.0 * s, theme::TEXT_FAINT, false, 0.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn nav_item(
    ctx: &mut UiCtx,
    fb: &mut FB,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    label: &str,
    icon: fn(&mut FB, i32, i32, i32, theme::Rgba),
    active: bool,
    enabled: bool,
    hit: Hit,
    badge: Option<&str>,
) {
    let s = ctx.scale;
    let hovered = enabled
        && ctx.cursor.0 >= x as f32
        && ctx.cursor.0 < (x + w) as f32
        && ctx.cursor.1 >= y as f32
        && ctx.cursor.1 < (y + h) as f32;
    if active {
        fb.rect_rounded(x, y, w, h, (9.0 * s) as i32, theme::SURFACE_2);
        let bar_h = (18.0 * s) as i32;
        fb.rect(x + (4.0 * s) as i32, y + (h - bar_h) / 2, (3.0 * s).max(1.0) as i32, bar_h, theme::ACCENT);
    } else if hovered {
        fb.rect_rounded(x, y, w, h, (9.0 * s) as i32, theme::SURFACE_2);
    }

    let ic = (18.0 * s) as i32;
    let (icon_c, text_c) = if active || hovered {
        (theme::TEXT, theme::TEXT)
    } else if enabled {
        (theme::TEXT_DIM, theme::TEXT_DIM)
    } else {
        (theme::TEXT_FAINT, theme::TEXT_FAINT)
    };
    icon(fb, x + (15.0 * s) as i32, y + (h - ic) / 2, ic, icon_c);
    let tx = (x + (15.0 * s) as i32 + ic + (11.0 * s) as i32) as f32;
    let ty = y as f32 + h as f32 / 2.0 + 13.5 * s * 0.35;
    font::draw_ellipsis(ctx.fonts, fb, tx, ty, label, 13.5 * s, text_c, active, {
        let mut avail = (x + w) as f32 - tx - (12.0 * s);
        if badge.is_some() {
            avail -= 34.0 * s;
        }
        avail.max(20.0)
    });
    if let Some(b) = badge {
        let bw = font::measure(ctx.fonts, b, 10.5 * s, false);
        let bx = (x + w - (12.0 * s) as i32) as f32 - bw;
        font::draw(ctx.fonts, fb, bx, ty, b, 10.5 * s, theme::TEXT_FAINT, false, 0.0);
    }
    if enabled {
        ctx.hits.add(Rect { x, y, w, h }, hit);
    }
}

// ---------------------------------------------------------------------------
// Icons (tiny vector-ish glyphs built from primitives)
// ---------------------------------------------------------------------------

fn icon_home(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    let roof = (sz * 5) / 11;
    for i in 0..roof {
        let half = (i * (sz / 2)).max(i) / roof + i / 3 + 1;
        fb.rect(x + sz / 2 - half, y + i, half * 2 + 1, 1, color);
    }
    fb.rect(x + 2, y + roof, sz - 4, sz - roof, color);
}

fn icon_person_add(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    let hr = sz / 5;
    fb.circle(x + sz / 2, y + hr + 1, hr, color);
    fb.rect_rounded(x + 1, y + sz / 2 + 1, sz - 2, sz / 2, (sz / 4) as i32, color);
    // Plus badge, top-right.
    let px = x + sz - 1;
    let py = y + 1;
    fb.rect(px - 3, py, 7, 2, color);
    fb.rect(px, py - 3, 2, 7, color);
}

fn icon_person(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    let hr = sz / 5;
    fb.circle(x + sz / 2, y + hr + 1, hr, color);
    fb.rect_rounded(x + 1, y + sz / 2 + 1, sz - 2, sz / 2, (sz / 4) as i32, color);
}

fn icon_charts(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    let bw = (sz / 4).max(2);
    for (i, hgt) in [(0usize, 5i32), (1, 9), (2, 13)] {
        let bx = x + (i as i32) * (bw + 2);
        fb.rect(bx, y + sz - hgt, bw, hgt, color);
    }
}

fn icon_chat(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    fb.rect_rounded(x, y + 1, sz, sz - 5, (sz / 4) as i32, color);
    // Speech tail.
    for i in 0..3 {
        fb.rect(x + 3 + i, y + sz - 5 + i, 3, 1, color);
    }
    // Three dots punched out with the sidebar background.
    let d = 1.max(sz / 9);
    for i in 0..3 {
        let dx = x + sz / 2 - sz / 4 + i * (sz / 4);
        fb.circle(dx, y + (sz - 5) / 2 + 1, d, theme::SURFACE);
    }
}

// ---------------------------------------------------------------------------
// Home pane: friends list
// ---------------------------------------------------------------------------

fn draw_home(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let pad = (18.0 * s) as i32;
    let header_h = (54.0 * s) as i32;
    let search_h = (52.0 * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;
    let places_h = (theme::PLACES_H * s) as i32;

    // Header: title + counters.
    font::draw(ctx.fonts, fb, x as f32 + pad as f32, header_h as f32 / 2.0 + 9.5 * s, "Friends", 19.0 * s, theme::TEXT, true, 0.0);
    let online = v.rows.iter().filter(|r| r.presence.kind != PresenceKind::Offline && r.presence.kind != PresenceKind::Invisible).count();
    let ingame = v.rows.iter().filter(|r| r.presence.kind == PresenceKind::InGame).count();
    let summary = format!(
        "{} online \u{b7} {} in game \u{b7} {} friends",
        online, ingame, v.rows.len()
    );
    let sum_w = font::measure(ctx.fonts, &summary, 12.5 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - sum_w, header_h as f32 / 2.0 + 12.5 * s * 0.35,
        &summary, 12.5 * s, theme::TEXT_DIM, false, 0.0,
    );
    fb.hline(x, header_h, w, [0x1E, 0x20, 0x22, 0xFF]);

    // Filter row.
    let field_h = (34.0 * s) as i32;
    let field_y = header_h + (search_h - field_h) / 2;
    let field_w = (340.0 * s) as i32;
    v.search.draw(ctx, fb, x + pad, field_y, field_w.min(w - 2 * pad), field_h, FieldId::Search, None);

    // Friends list (vertical, Roblox-style, arrow scrollbar on the right).
    let scroll_w = (theme::SCROLL_W * s) as i32;
    let list_top = (header_h + search_h) as f32;
    let places_y = h - status_h - places_h;
    let list_h = places_y as f32 - list_top;
    let query = v.search.value.trim().to_lowercase();
    let filtered: Vec<&FriendRow> = v.rows.iter().filter(|r| r.matches_filter(&query)).collect();

    let row_h = theme::ROW_H * s;
    let rows_x = (x + pad) as f32;
    let rows_w = (w - 2 * pad - scroll_w) as f32;
    let content_h = filtered.len() as f32 * row_h;
    v.scroll = clamp_scroll(v.scroll, content_h, list_h);
    v.view_metrics = Some(ListMetrics { top: list_top, h: list_h, content_h });

    let list_clip = crate::draw::Clip { x0: x, y0: list_top as i32, x1: x + w, y1: places_y };
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
        font::draw(
            ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, list_top + list_h / 2.0,
            msg, 14.0 * s, theme::TEXT_FAINT, false, 0.0,
        );
        if v.refreshing && v.last_update.is_none() {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s, list_top + list_h / 2.0 - 5.0 * s,
                7.0 * s, ctx.now_ms, theme::ACCENT, s,
            );
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
            draw_row(ctx, fb, row, rows_x, y, rows_w, row_h, s);
        }
        fb.set_clip(old_clip);
        // Scrollbar with up/down arrows (drawn on top, outside the clip so
        // the thumb never slides under the places strip).
        draw_scrollbar(
            ctx, fb,
            x + w - pad - scroll_w + scroll_w / 2, list_top as i32, scroll_w, list_h as i32,
            v.scroll, content_h, list_h, s,
        );
    }

    // Reserved strip for future place cards.
    draw_places(ctx, fb, x, w, places_y, places_h);
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

    draw_avatar(ctx, fb, row.friend.id, &row.friend.display_name, x as i32 + pad, avatar_y, avatar_sz);

    let text_x = x + (pad + avatar_sz + (12.0 * s) as i32) as f32;
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

// ---------------------------------------------------------------------------
// Scrollbar with arrows
// ---------------------------------------------------------------------------

/// Roblox-style vertical scrollbar: up/down arrow buttons plus a draggable
/// thumb. `x` is the track's left edge; hits are registered for both arrows
/// and the whole track (thumb drag).
fn draw_scrollbar(
    ctx: &mut UiCtx,
    fb: &mut FB,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scroll: f32,
    content_h: f32,
    view_h: f32,
    s: f32,
) {
    if content_h <= view_h + 0.5 {
        return;
    }
    let aw = w.max((13.0 * s) as i32); // arrow zone height (square-ish)
    let track_y = y + aw;
    let track_h = (h - 2 * aw).max(1);

    // Whole-track drag hit first, so the arrows (added later) win on top.
    ctx.hits.add(
        Rect { x: x - (6.0 * s) as i32, y, w: w + (12.0 * s) as i32, h },
        Hit::Scrollbar,
    );
    ctx.hits.add(Rect { x, y, w, h: aw }, Hit::ScrollUp);
    ctx.hits.add(Rect { x, y: y + h - aw, w, h: aw }, Hit::ScrollDown);

    // Thumb.
    let thumb_h = ((view_h / content_h) * track_h as f32).max(22.0 * s) as i32;
    let scrollable = (content_h - view_h).max(0.001);
    let thumb_y = track_y as f32 + (scroll / scrollable) * (track_h as f32 - thumb_h as f32);
    fb.rect_rounded(x + w / 2 - 3, thumb_y as i32, 6, thumb_h, (3.0 * s) as i32, theme::BORDER);

    // Arrows.
    let cx = x + w / 2;
    tri_up(fb, cx, y + aw / 2, theme::TEXT_DIM);
    tri_down(fb, cx, y + h - aw / 2, theme::TEXT_DIM);
}

fn tri_up(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
    for i in 0..5 {
        fb.rect(cx - i, cy - 4 + i, i * 2 + 1, 1, color);
    }
}

fn tri_down(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
    for i in 0..5 {
        fb.rect(cx - (4 - i), cy - 4 + i, (4 - i) * 2 + 1, 1, color);
    }
}

// ---------------------------------------------------------------------------
// Places strip (reserved for future place cards)
// ---------------------------------------------------------------------------

fn draw_places(ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, y: i32, h: i32) {
    let s = ctx.scale;
    fb.rect(x, y, w, h, theme::SURFACE);
    fb.hline(x, y, w, theme::BORDER);

    let pad = (18.0 * s) as i32;
    font::draw(ctx.fonts, fb, (x + pad) as f32, y as f32 + (24.0 * s), "Places", 14.0 * s, theme::TEXT, true, 0.0);
    let note = "Coming soon \u{b7} place cards will appear here";
    let nw = font::measure(ctx.fonts, note, 12.0 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - nw, y as f32 + (24.0 * s),
        note, 12.0 * s, theme::TEXT_FAINT, false, 0.0,
    );

    // Placeholder cards.
    let card_w = (148.0 * s) as i32;
    let card_h = h - (56.0 * s) as i32;
    let gap = (14.0 * s) as i32;
    let card_y = y + (36.0 * s) as i32;
    let mut cx = x + pad;
    let mut drawn = 0;
    while cx + card_w <= x + w - pad && drawn < 4 {
        fb.rect_rounded(cx, card_y, card_w, card_h, (10.0 * s) as i32, theme::SURFACE_2);
        let t = "Coming soon";
        let tw = font::measure(ctx.fonts, t, 11.5 * s, false);
        font::draw(
            ctx.fonts, fb, cx as f32 + (card_w as f32 - tw) / 2.0, card_y as f32 + card_h as f32 / 2.0 + 11.5 * s * 0.35,
            t, 11.5 * s, theme::TEXT_FAINT, false, 0.0,
        );
        cx += card_w + gap;
        drawn += 1;
    }
}

// ---------------------------------------------------------------------------
// Add Friends pane
// ---------------------------------------------------------------------------

fn draw_add_friends(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let pad = (18.0 * s) as i32;
    let header_h = (54.0 * s) as i32;
    let search_h = (56.0 * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;

    // Header.
    font::draw(ctx.fonts, fb, x as f32 + pad as f32, header_h as f32 / 2.0 + 9.5 * s, "Add Friends", 19.0 * s, theme::TEXT, true, 0.0);
    let hint = "Find people by username and send a friend request";
    let hw = font::measure(ctx.fonts, hint, 12.5 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - hw, header_h as f32 / 2.0 + 12.5 * s * 0.35,
        hint, 12.5 * s, theme::TEXT_DIM, false, 0.0,
    );
    fb.hline(x, header_h, w, [0x1E, 0x20, 0x22, 0xFF]);

    // Search row: field + button.
    let field_h = (36.0 * s) as i32;
    let field_y = header_h + (search_h - field_h) / 2;
    let btn_w = (96.0 * s) as i32;
    let field_w = (w - 2 * pad - btn_w - (10.0 * s) as i32).min(460 * s as i32);
    v.add.query.draw(ctx, fb, x + pad, field_y, field_w, field_h, FieldId::AddSearch, None);
    let btn_rect = draw_button(
        ctx, fb,
        Rect { x: x + pad + field_w + (10.0 * s) as i32, y: field_y, w: btn_w, h: field_h },
        if v.add.searching { "Searching\u{2026}" } else { "Search" },
        BtnStyle::Primary,
        v.add.searching,
    );
    ctx.hits.add(btn_rect, Hit::AddSearchBtn);
    if v.add.searching {
        ui::draw_spinner(
            fb, btn_rect.x as f32 + 14.0 * s, btn_rect.y as f32 + btn_rect.h as f32 / 2.0,
            5.5 * s, ctx.now_ms, theme::BTN_TEXT, s,
        );
    }

    // Results list.
    let list_top = (header_h + search_h) as f32;
    let list_h = (h - status_h) as f32 - list_top;
    let scroll_w = (theme::SCROLL_W * s) as i32;
    let rows_x = (x + pad) as f32;
    let rows_w = (w - 2 * pad - scroll_w) as f32;
    let row_h = (58.0 * s) as f32;
    let content_h = v.add.results.len() as f32 * row_h;
    v.add_scroll = clamp_scroll(v.add_scroll, content_h, list_h);
    v.add_metrics = Some(ListMetrics { top: list_top, h: list_h, content_h });

    let list_clip = crate::draw::Clip { x0: x, y0: list_top as i32, x1: x + w, y1: (list_top + list_h) as i32 };
    let old_clip = fb.clip();
    fb.set_clip(list_clip);

    if v.add.results.is_empty() {
        fb.set_clip(old_clip);
        let (msg, spin): (String, bool) = if v.add.searching {
            ("Searching\u{2026}".into(), true)
        } else if let Some(err) = &v.add.error {
            let mut m = format!("Error: {}", err);
            m.truncate(120);
            (m, false)
        } else if v.add.searched_once {
            ("No users found".into(), false)
        } else {
            ("Type at least 2 characters of a username and press Enter".into(), false)
        };
        let mw = font::measure(ctx.fonts, &msg, 14.0 * s, false);
        font::draw(
            ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, list_top + list_h / 2.0,
            &msg, 14.0 * s, theme::TEXT_FAINT, false, 0.0,
        );
        if spin {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s, list_top + list_h / 2.0 - 5.0 * s,
                7.0 * s, ctx.now_ms, theme::ACCENT, s,
            );
        }
    } else {
        let first = (v.add_scroll / row_h) as usize;
        for (vi, user) in v.add.results.iter().enumerate().skip(first) {
            let y = list_top - v.add_scroll + vi as f32 * row_h;
            if y > list_top + list_h {
                break;
            }
            if y + row_h < list_top {
                continue;
            }
            draw_result_row(&v.add, ctx, fb, user, rows_x, y, rows_w, row_h, s);
        }
        fb.set_clip(old_clip);
        draw_scrollbar(
            ctx, fb,
            x + w - pad - scroll_w + scroll_w / 2, list_top as i32, scroll_w, list_h as i32,
            v.add_scroll, content_h, list_h, s,
        );
    }
}

fn draw_result_row(add: &AddFriendsState, ctx: &mut UiCtx, fb: &mut FB, user: &UserEntry, x: f32, y: f32, w: f32, h: f32, s: f32) {
    let (cx, cy) = ctx.cursor;
    let hovered = cx >= x && cx < x + w && cy >= y && cy < y + h;
    if hovered {
        fb.rect(x as i32, y as i32, w as i32, h as i32, theme::SURFACE_2);
    }

    let pad = (12.0 * s) as i32;
    let avatar_sz = (42.0 * s) as i32;
    let avatar_y = y as i32 + ((h as i32) - avatar_sz) / 2;
    draw_avatar(ctx, fb, user.id, user.display_or_name(), x as i32 + pad, avatar_y, avatar_sz);

    let text_x = x + (pad + avatar_sz + (12.0 * s) as i32) as f32;
    let btn_w = (92.0 * s) as i32;
    let max_text_w = w - (text_x - x) - btn_w as f32 - (24.0 * s);

    let state = add.state_of(user.id);
    let name_c = theme::TEXT;
    font::draw_ellipsis(
        ctx.fonts, fb, text_x, y + 23.0 * s, user.display_or_name(), 14.0 * s, name_c, true, max_text_w,
    );
    let sub = format!("@{}", user.name);
    let sub_c = match &state {
        RequestState::Failed(_) => theme::RED,
        _ => theme::TEXT_DIM,
    };
    let sub_text = match &state {
        RequestState::Failed(msg) => {
            let mut t = msg.clone();
            if t.len() > 80 {
                t.truncate(80);
            }
            t
        }
        _ => sub,
    };
    font::draw_ellipsis(ctx.fonts, fb, text_x, y + 42.0 * s, &sub_text, 12.0 * s, sub_c, false, max_text_w);

    let btn_rect = Rect {
        x: (x + w - pad as f32 - btn_w as f32) as i32,
        y: (y + (h - 30.0 * s) / 2.0) as i32,
        w: btn_w,
        h: (30.0 * s) as i32,
    };
    match state {
        RequestState::Idle => {
            let r = draw_button(ctx, fb, btn_rect, "Add", BtnStyle::Primary, false);
            ctx.hits.add(r, Hit::AddUser(user.id));
        }
        RequestState::Pending => {
            draw_button(ctx, fb, btn_rect, "Sending\u{2026}", BtnStyle::Secondary, true);
        }
        RequestState::Sent => {
            let tw = font::measure(ctx.fonts, "Request sent", 12.5 * s, false);
            font::draw(
                ctx.fonts, fb, btn_rect.x as f32 + (btn_rect.w as f32 - tw) / 2.0,
                btn_rect.y as f32 + btn_rect.h as f32 / 2.0 + 12.5 * s * 0.35,
                "Request sent", 12.5 * s, theme::GREEN, false, 0.0,
            );
        }
        RequestState::Failed(_) => {
            let r = draw_button(ctx, fb, btn_rect, "Retry", BtnStyle::Secondary, false);
            ctx.hits.add(r, Hit::AddUser(user.id));
        }
    }

    fb.hline(x as i32, (y + h) as i32, w as i32, [0x1E, 0x20, 0x22, 0xFF]);
}

// ---------------------------------------------------------------------------
// Shared: status bar, avatar
// ---------------------------------------------------------------------------

fn draw_status_bar(v: &MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, y: i32, h: i32) {
    let s = ctx.scale;
    fb.rect(x, y, w, h, theme::SURFACE);
    fb.hline(x, y, w, theme::BORDER);
    let pad = (16.0 * s) as i32;
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
    font::draw(
        ctx.fonts, fb, (x + pad) as f32, y as f32 + h as f32 / 2.0 + 12.0 * s * 0.35,
        &left, 12.0 * s, theme::TEXT_FAINT, false, 0.0,
    );
    if v.refreshing {
        ui::draw_spinner(
            fb, (x + pad) as f32 + font::measure(ctx.fonts, &left, 12.0 * s, false) + 14.0 * s,
            y as f32 + h as f32 / 2.0 - 1.0 * s, 6.0 * s, ctx.now_ms, theme::ACCENT, s,
        );
    }
    if let Some(err) = &v.error {
        let ew = (w as f32 - font::measure(ctx.fonts, &left, 12.0 * s, false) - 60.0 * s).max(0.0);
        font::draw_ellipsis(
            ctx.fonts, fb, (x + w - pad) as f32 - ew.min(w as f32 - 220.0 * s), y as f32 + h as f32 / 2.0 + 12.0 * s * 0.35,
            &format!("Error: {}", err), 12.0 * s, theme::RED, false, w as f32 - 220.0 * s,
        );
    }
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

#[cfg(test)]
mod tests {
    use super::clamp_scroll;

    #[test]
    fn clamp_scroll_basics() {
        // Nothing to scroll.
        assert_eq!(clamp_scroll(10.0, 100.0, 200.0), 0.0);
        // Within range stays.
        assert_eq!(clamp_scroll(30.0, 300.0, 200.0), 30.0);
        // Negative and overflow clamp.
        assert_eq!(clamp_scroll(-5.0, 300.0, 200.0), 0.0);
        assert_eq!(clamp_scroll(500.0, 300.0, 200.0), 100.0);
    }
}
