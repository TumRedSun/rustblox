//! Main menu, Roblox-style: a collapsible left sidebar (the rotated red logo
//! tile doubles as the "R" of the wordmark; mini mode shows icons only),
//! a horizontal friends strip with left/right scroll arrows, a places search
//! bar on top and a place-card grid (icon, name, rating, Join) that fills the
//! rest of the screen.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
use crate::model::{
    avatar_placeholder_color, format_count, FriendRow, PlaceCard, PresenceKind, UserEntry,
};
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
    /// Vertical lists: view top edge. Horizontal strip: view left edge.
    pub top: f32,
    /// Vertical lists: view height. Horizontal strip: view width.
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
    /// Sidebar collapsed to the icon-only mini mode.
    pub collapsed: bool,
    pub tab: MenuTab,
    // -- Home: places search + grid --
    /// Places search box (top of the Home pane).
    pub search: TextField,
    pub places: Vec<PlaceCard>,
    pub places_searching: bool,
    pub places_error: Option<String>,
    /// The query the current `places` belong to ("" = recommendation feed).
    pub places_done_for: String,
    pub places_scroll: f32,
    pub places_metrics: Option<ListMetrics>,
    // -- Home: horizontal friends strip --
    pub strip_scroll: f32,
    pub strip_metrics: Option<ListMetrics>,
    /// Physical-pixel rect of the strip (mouse-wheel routing).
    pub strip_rect: Option<Rect>,
    // -- Add Friends pane --
    pub add: AddFriendsState,
    pub add_scroll: f32,
    pub add_metrics: Option<ListMetrics>,
    // -- shared --
    pub last_update: Option<Instant>,
    pub next_refresh: Option<Instant>,
    pub refreshing: bool,
    pub error: Option<String>,
    /// Transient status-bar note ("text", shown-until).
    pub notice: Option<(String, Instant)>,
}

impl MenuView {
    pub fn new(profile: crate::model::UserProfile) -> MenuView {
        MenuView {
            profile,
            rows: Vec::new(),
            collapsed: false,
            tab: MenuTab::Home,
            search: TextField::new("Search places by name"),
            places: Vec::new(),
            places_searching: true,
            places_error: None,
            places_done_for: String::new(),
            places_scroll: 0.0,
            places_metrics: None,
            strip_scroll: 0.0,
            strip_metrics: None,
            strip_rect: None,
            add: AddFriendsState::new(),
            add_scroll: 0.0,
            add_metrics: None,
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
    let sb_w = ((if v.collapsed {
        theme::SIDEBAR_W_MINI
    } else {
        theme::SIDEBAR_W_FULL
    }) * s) as i32;

    draw_sidebar(v, ctx, fb, sb_w, h);

    // Main pane.
    let mx = sb_w;
    let mw = w - sb_w;
    fb.rect(mx, 0, mw, h, theme::BG);
    match v.tab {
        MenuTab::Home => draw_home(v, ctx, fb, mx, mw, h),
        MenuTab::AddFriends => draw_add_friends(v, ctx, fb, mx, mw, h),
    }

    let sb_y = h - (theme::STATUSBAR_H * s) as i32;
    draw_status_bar(v, ctx, fb, mx, mw, sb_y, theme::STATUSBAR_H * s);
}

// ---------------------------------------------------------------------------
// Sidebar (collapsible: full labels <-> icon-only mini)
// ---------------------------------------------------------------------------

fn draw_sidebar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, w: i32, h: i32) {
    let s = ctx.scale;
    let mini = v.collapsed;
    fb.rect(0, 0, w, h, theme::SURFACE);
    fb.rect(w - 1, 0, 1, h, [0x1E, 0x20, 0x22, 0xFF]);

    let pad = (12.0 * s) as i32;

    // Collapse/expand toggle (chevrons).
    let tg = (22.0 * s) as i32;
    let tgx = if mini { (w - tg) / 2 } else { w - pad - tg };
    let tgr = Rect { x: tgx, y: pad, w: tg, h: tg };
    let tghov = ctx.cursor.0 >= tgr.x as f32
        && ctx.cursor.0 < (tgr.x + tgr.w) as f32
        && ctx.cursor.1 >= tgr.y as f32
        && ctx.cursor.1 < (tgr.y + tgr.h) as f32;
    if tghov {
        fb.rect_rounded(tgr.x, tgr.y, tgr.w, tgr.h, (6.0 * s) as i32, theme::SURFACE_2);
    }
    chevron(
        fb,
        tgr.x + tgr.w / 2,
        tgr.y + tgr.h / 2,
        if tghov { theme::TEXT } else { theme::TEXT_DIM },
        mini,
    );
    ctx.hits.add(tgr, Hit::SidebarToggle);
    let mut y = tgr.y + tgr.h + (10.0 * s) as i32;

    // Logo: the slightly rotated red tile IS the "R" of the wordmark.
    let sq = (26.0 * s) as i32;
    let tile_cx = if mini { w / 2 } else { pad + sq / 2 + 1 };
    let tile_cy = y + sq / 2;
    fb.rect_rounded_rot(tile_cx, tile_cy, sq, sq, (6.0 * s) as i32, -0.16, theme::RED);
    let ir = 14.0 * s;
    let ir_w = font::measure(ctx.fonts, "R", ir, true);
    font::draw(
        ctx.fonts, fb, tile_cx as f32 - ir_w / 2.0, tile_cy as f32 + ir * 0.35, "R", ir,
        theme::WHITE, true, 0.0,
    );
    if !mini {
        let wx = (tile_cx + sq / 2 + (6.0 * s) as i32) as f32;
        font::draw(
            ctx.fonts, fb, wx, tile_cy as f32 + 14.0 * s * 0.35, "ustBlox", 14.0 * s,
            theme::TEXT, true, 0.0,
        );
    }
    y = tile_cy + sq / 2 + (14.0 * s) as i32;

    // Profile chip.
    let avatar_sz = ((if mini { 32.0 } else { 34.0 }) * s) as i32;
    let ax = if mini { (w - avatar_sz) / 2 } else { pad };
    draw_avatar(ctx, fb, v.profile.id, &v.profile.display_name, ax, y, avatar_sz);
    if !mini {
        let nx = (ax + avatar_sz + (9.0 * s) as i32) as f32;
        let name_w = (w - pad) as f32 - nx - (6.0 * s);
        font::draw_ellipsis(
            ctx.fonts, fb, nx, y as f32 + 13.0 * s, &v.profile.display_name, 12.5 * s,
            theme::TEXT, true, name_w,
        );
        font::draw_ellipsis(
            ctx.fonts, fb, nx, y as f32 + 27.0 * s, &format!("@{}", v.profile.name), 11.0 * s,
            theme::TEXT_DIM, false, name_w,
        );
    }
    y += avatar_sz + (12.0 * s) as i32;

    // Navigation.
    let item_h = (34.0 * s) as i32;
    let item_pad = if mini { (6.0 * s) as i32 } else { pad };
    let item_w = w - 2 * item_pad;
    nav_item(
        ctx, fb, item_pad, y, item_w, item_h, "Home", icon_home,
        v.tab == MenuTab::Home, true, Hit::NavHome, None, mini,
    );
    y += item_h + (5.0 * s) as i32;
    nav_item(
        ctx, fb, item_pad, y, item_w, item_h, "Add Friends", icon_person_add,
        v.tab == MenuTab::AddFriends, true, Hit::NavAdd, None, mini,
    );
    y += item_h + (9.0 * s) as i32;

    if !mini {
        fb.hline(pad, y, w - 2 * pad, theme::BORDER);
        y += (9.0 * s) as i32;
    }

    // Future sections (disabled placeholders).
    for (label, icon) in [
        ("Charts", icon_charts as fn(&mut FB, i32, i32, i32, theme::Rgba)),
        ("Avatar", icon_person),
        ("Chat", icon_chat),
    ] {
        nav_item(ctx, fb, item_pad, y, item_w, item_h, label, icon, false, false, Hit::None, None, mini);
        y += item_h + (5.0 * s) as i32;
    }

    // Bottom: sign out (+ version in the expanded mode).
    if mini {
        let bw = (34.0 * s) as i32;
        let bx = (w - bw) / 2;
        let by = h - bw - (12.0 * s) as i32;
        let hovered = ctx.cursor.0 >= bx as f32
            && ctx.cursor.0 < (bx + bw) as f32
            && ctx.cursor.1 >= by as f32
            && ctx.cursor.1 < (by + bw) as f32;
        if hovered {
            fb.rect_rounded(bx, by, bw, bw, (8.0 * s) as i32, theme::SURFACE_2);
        }
        icon_signout(
            fb, bx + (bw - (16.0 * s) as i32) / 2, by + (bw - (16.0 * s) as i32) / 2,
            (16.0 * s) as i32, if hovered { theme::TEXT } else { theme::TEXT_DIM },
        );
        ctx.hits.add(Rect { x: bx, y: by, w: bw, h: bw }, Hit::Logout);
    } else {
        let btn_h = (30.0 * s) as i32;
        let by = h - btn_h - (20.0 * s) as i32;
        let rect = draw_button(
            ctx, fb,
            Rect { x: pad, y: by, w: item_w, h: btn_h },
            "Sign out", BtnStyle::Secondary, false,
        );
        ctx.hits.add(rect, Hit::Logout);
        let ver = format!("RustBlox v{}", env!("CARGO_PKG_VERSION"));
        let vw = font::measure(ctx.fonts, &ver, 10.5 * s, false);
        font::draw(
            ctx.fonts, fb, (w - pad) as f32 - vw, (by + btn_h) as f32 + 14.0 * s,
            &ver, 10.5 * s, theme::TEXT_FAINT, false, 0.0,
        );
    }
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
    mini: bool,
) {
    let s = ctx.scale;
    let hovered = enabled
        && ctx.cursor.0 >= x as f32
        && ctx.cursor.0 < (x + w) as f32
        && ctx.cursor.1 >= y as f32
        && ctx.cursor.1 < (y + h) as f32;
    if active || hovered {
        fb.rect_rounded(x, y, w, h, (8.0 * s) as i32, theme::SURFACE_2);
    }
    if active {
        let bar_h = (16.0 * s) as i32;
        fb.rect(x + (3.0 * s) as i32, y + (h - bar_h) / 2, (3.0 * s).max(1.0) as i32, bar_h, theme::ACCENT);
    }

    let ic = (16.0 * s) as i32;
    let (icon_c, text_c) = if active || hovered {
        (theme::TEXT, theme::TEXT)
    } else if enabled {
        (theme::TEXT_DIM, theme::TEXT_DIM)
    } else {
        (theme::TEXT_FAINT, theme::TEXT_FAINT)
    };
    let ix = if mini { x + (w - ic) / 2 } else { x + (12.0 * s) as i32 };
    icon(fb, ix, y + (h - ic) / 2, ic, icon_c);

    if !mini {
        let tx = (ix + ic + (10.0 * s) as i32) as f32;
        let ty = y as f32 + h as f32 / 2.0 + 12.5 * s * 0.35;
        font::draw_ellipsis(ctx.fonts, fb, tx, ty, label, 12.5 * s, text_c, active, {
            let mut avail = (x + w) as f32 - tx - (10.0 * s);
            if badge.is_some() {
                avail -= 32.0 * s;
            }
            avail.max(20.0)
        });
        if let Some(b) = badge {
            let bw = font::measure(ctx.fonts, b, 10.0 * s, false);
            let bx = (x + w - (10.0 * s) as i32) as f32 - bw;
            font::draw(ctx.fonts, fb, bx, ty, b, 10.0 * s, theme::TEXT_FAINT, false, 0.0);
        }
    }
    if enabled {
        ctx.hits.add(Rect { x, y, w, h }, hit);
    }
}

// ---------------------------------------------------------------------------
// Icons (tiny vector glyphs built from primitives)
// ---------------------------------------------------------------------------

fn icon_home(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    // Roof.
    let roof = (sz * 2) / 5;
    for i in 0..roof {
        let half = ((i + 1) * (sz / 2)) / roof + 1;
        fb.rect(x + sz / 2 - half, y + i, half * 2 + 1, 1, color);
    }
    // Body (outline) + door.
    let body_y = y + roof - 1;
    let body_h = sz - 1 - body_y;
    fb.outline(x + 2, body_y, sz - 4, body_h, color);
    let dw = (sz / 5).max(2);
    fb.rect(x + sz / 2 - dw / 2, y + sz - 1 - body_h / 2, dw, body_h / 2, color);
}

fn icon_person_add(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    let hr = (sz * 3) / 11;
    let cx = x + sz / 2 - 1;
    fb.circle(cx, y + hr + 2, hr, color);
    let sy = y + sz / 2 + 3;
    let rows = (y + sz - 1 - sy).max(1);
    for i in 0..rows {
        let half = (((i + 1) * (sz / 2 - 2)) / rows + 2).min(sz / 2);
        fb.rect(cx - half, sy + i, half * 2, 1, color);
    }
    // Plus badge, top-right.
    let px = x + sz - 2;
    let py = y + 2;
    fb.rect(px - 4, py, 9, 2, color);
    fb.rect(px - 1, py - 3, 2, 9, color);
}

fn icon_person(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    let hr = sz / 4;
    fb.circle(x + sz / 2, y + hr + 1, hr, color);
    let sy = y + sz / 2 + 2;
    let rows = (sz - 1 - sy).max(1);
    for i in 0..rows {
        let half = (((i + 1) * (sz / 2 - 2)) / rows + 2).min(sz / 2);
        fb.rect(x + sz / 2 - half, sy + i, half * 2, 1, color);
    }
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

fn icon_signout(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    // Door frame on the right.
    let dw = sz / 2 + 2;
    let dx = x + sz - dw;
    fb.outline(dx, y, dw, sz, color);
    // Arrow pointing through the door.
    let ay = y + sz / 2;
    let tip = dx + 1;
    fb.rect(x + 1, ay, tip - x - 1, 1, color);
    for i in 0..3 {
        fb.rect(tip - 1 - i, ay - i, 1, i * 2 + 1, color);
    }
}

/// Small collapse chevron: `right` = point right (expand), else left.
fn chevron(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba, right: bool) {
    for i in 0..4 {
        let px = if right { cx - 3 + i } else { cx + 3 - i };
        fb.rect(px, cy - 3 + i, 1, 1, color);
        fb.rect(px, cy + 3 - i, 1, 1, color);
    }
}

fn tri_left(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
    for i in 0..5 {
        fb.rect(cx - 4 + i, cy - (4 - i), 1, (4 - i) * 2 + 1, color);
    }
}

fn tri_right(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
    for i in 0..5 {
        fb.rect(cx + 4 - i, cy - (4 - i), 1, (4 - i) * 2 + 1, color);
    }
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

/// Small green "likes" triangle drawn before the rating percentage.
fn tri_up_small(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
    for i in 0..4 {
        fb.rect(cx - i, cy - 3 + i, i * 2 + 1, 1, color);
    }
}

// ---------------------------------------------------------------------------
// Home pane: search, friends strip, places grid
// ---------------------------------------------------------------------------

fn draw_home(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let pad = (16.0 * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;
    let mut y = (14.0 * s) as i32;

    // -- Places search row (field + button) --
    let field_h = (34.0 * s) as i32;
    let btn_w = (92.0 * s) as i32;
    let field_w = (w - 2 * pad - btn_w - (10.0 * s) as i32).min((560.0 * s) as i32);
    v.search.draw(ctx, fb, x + pad, y, field_w, field_h, FieldId::Search, None);
    let btn_rect = draw_button(
        ctx, fb,
        Rect { x: x + pad + field_w + (10.0 * s) as i32, y, w: btn_w, h: field_h },
        if v.places_searching { "Searching\u{2026}" } else { "Search" },
        BtnStyle::Primary,
        v.places_searching,
    );
    ctx.hits.add(btn_rect, Hit::SearchPlacesBtn);
    if v.places_searching {
        ui::draw_spinner(
            fb, btn_rect.x as f32 + (btn_rect.w as f32 - font::measure(ctx.fonts, "Searching\u{2026}", 14.0 * s, true)) / 2.0 - 9.0 * s,
            btn_rect.y as f32 + btn_rect.h as f32 / 2.0, 5.0 * s, ctx.now_ms, theme::BTN_TEXT, s,
        );
    }
    y += field_h + (16.0 * s) as i32;

    // -- Friends section --
    font::draw(ctx.fonts, fb, x as f32 + pad as f32, y as f32 + 11.0 * s, "Friends", 15.0 * s, theme::TEXT, true, 0.0);
    let online = v.rows.iter().filter(|r| r.presence.kind != PresenceKind::Offline && r.presence.kind != PresenceKind::Invisible).count();
    let ingame = v.rows.iter().filter(|r| r.presence.kind == PresenceKind::InGame).count();
    let summary = format!("{} online \u{b7} {} in game \u{b7} {} friends", online, ingame, v.rows.len());
    let sum_w = font::measure(ctx.fonts, &summary, 11.5 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - sum_w, y as f32 + 11.5 * s * 0.35 + 4.0 * s,
        &summary, 11.5 * s, theme::TEXT_DIM, false, 0.0,
    );
    y += (26.0 * s) as i32;

    let strip_h = (theme::STRIP_H * s) as i32;
    draw_friends_strip(v, ctx, fb, x + pad, y, w - 2 * pad, strip_h);
    y += strip_h + (16.0 * s) as i32;

    // -- Places section (fills the rest of the screen) --
    font::draw(ctx.fonts, fb, x as f32 + pad as f32, y as f32 + 11.0 * s, "Places", 15.0 * s, theme::TEXT, true, 0.0);
    let subtitle = if v.search.value.trim().is_empty() {
        "Recommended for you".to_string()
    } else {
        format!("Results for \u{201c}{}\u{201d}", v.search.value.trim())
    };
    let sub_w = font::measure(ctx.fonts, &subtitle, 11.5 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - sub_w, y as f32 + 11.5 * s * 0.35 + 4.0 * s,
        &subtitle, 11.5 * s, theme::TEXT_DIM, false, 0.0,
    );
    y += (26.0 * s) as i32;

    draw_places_grid(v, ctx, fb, x + pad, y, w - 2 * pad, h - status_h - (8.0 * s) as i32 - y);
}

fn draw_friends_strip(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, y: i32, w: i32, h: i32) {
    let s = ctx.scale;
    fb.rect_rounded(x, y, w, h, (12.0 * s) as i32, theme::SURFACE);
    v.strip_rect = Some(Rect { x, y, w, h });

    if v.rows.is_empty() {
        let (msg, spin) = if v.refreshing && v.last_update.is_none() {
            ("Loading friends\u{2026}", true)
        } else {
            ("No friends yet \u{2014} open Add Friends", false)
        };
        let mw = font::measure(ctx.fonts, msg, 13.0 * s, false);
        font::draw(
            ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, y as f32 + h as f32 / 2.0,
            msg, 13.0 * s, theme::TEXT_FAINT, false, 0.0,
        );
        if spin {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 16.0 * s, y as f32 + h as f32 / 2.0 - 4.0 * s,
                6.0 * s, ctx.now_ms, theme::ACCENT, s,
            );
        }
        v.strip_metrics = None;
        return;
    }

    let card_w = (theme::STRIP_CARD_W * s) as i32;
    let gap = (8.0 * s) as i32;
    let arrow_w = (26.0 * s) as i32;
    let inset = (6.0 * s) as i32;
    let content_w = v.rows.len() as f32 * (card_w + gap) as f32;
    let view_w = (w - 2 * arrow_w - 2 * inset) as f32;
    v.strip_scroll = clamp_scroll(v.strip_scroll, content_w, view_w);
    v.strip_metrics = Some(ListMetrics { top: x as f32, h: view_w, content_h: content_w });
    let scrollable = content_w > view_w + 0.5;

    // Cards (clipped to the strip viewport).
    let clip = crate::draw::Clip { x0: x + arrow_w, y0: y + 1, x1: x + w - arrow_w, y1: y + h - 1 };
    let old_clip = fb.clip();
    fb.set_clip(clip);
    let mut cx = x + arrow_w + inset - v.strip_scroll as i32;
    for row in &v.rows {
        if cx + card_w >= clip.x0 && cx <= clip.x1 {
            draw_friend_card(ctx, fb, row, cx, y, card_w, h, s);
        }
        cx += card_w + gap;
    }
    fb.set_clip(old_clip);

    // Left/right arrow buttons (dimmed when the strip end is reached).
    if scrollable {
        let at_start = v.strip_scroll <= 0.5;
        let at_end = v.strip_scroll >= content_w - view_w - 0.5;
        draw_strip_arrow(ctx, fb, Rect { x, y, w: arrow_w, h }, true, at_start, Hit::StripLeft);
        draw_strip_arrow(ctx, fb, Rect { x: x + w - arrow_w, y, w: arrow_w, h }, false, at_end, Hit::StripRight);
    }
}

fn draw_strip_arrow(ctx: &mut UiCtx, fb: &mut FB, r: Rect, left: bool, dim: bool, hit: Hit) {
    let s = ctx.scale;
    let hovered = !dim
        && ctx.cursor.0 >= r.x as f32
        && ctx.cursor.0 < (r.x + r.w) as f32
        && ctx.cursor.1 >= r.y as f32
        && ctx.cursor.1 < (r.y + r.h) as f32;
    if hovered {
        fb.rect_rounded(r.x + 1, r.y + 1, r.w - 2, r.h - 2, (8.0 * s) as i32, theme::SURFACE_2);
    }
    let color = if dim { theme::TEXT_FAINT } else if hovered { theme::TEXT } else { theme::TEXT_DIM };
    if left {
        tri_left(fb, r.x + r.w / 2, r.y + r.h / 2, color);
    } else {
        tri_right(fb, r.x + r.w / 2, r.y + r.h / 2, color);
    }
    ctx.hits.add(r, hit);
}

fn draw_friend_card(ctx: &mut UiCtx, fb: &mut FB, row: &FriendRow, x: i32, y: i32, w: i32, h: i32, s: f32) {
    let (cx, cy) = ctx.cursor;
    let hovered = cx >= x as f32 && cx < (x + w) as f32 && cy >= y as f32 && cy < (y + h) as f32;
    if hovered {
        fb.rect_rounded(x + 2, y + 2, w - 4, h - 4, (10.0 * s) as i32, theme::SURFACE_2);
    }

    let a = (56.0 * s) as i32;
    let ax = x + (w - a) / 2;
    let ay = y + (8.0 * s) as i32;
    draw_avatar(ctx, fb, row.friend.id, &row.friend.display_name, ax, ay, a);

    // Presence dot pinned to the avatar corner.
    let d = (12.0 * s) as i32;
    let dx = ax + a - d + 1;
    let dy = ay + a - d + 1;
    fb.circle(dx + d / 2, dy + d / 2, d / 2 + (1.5 * s) as i32, theme::SURFACE);
    fb.circle(dx + d / 2, dy + d / 2, d / 2, row.presence.kind.dot_color());

    let center = x as f32 + w as f32 / 2.0;
    let name_baseline = (ay + a) as f32 + (18.0 * s);
    draw_centered(ctx, fb, center, name_baseline, &row.friend.display_name, 12.0 * s, theme::TEXT, true, w as f32 - 10.0 * s);
    let label = row.status_label();
    let label_color = match row.presence.kind {
        PresenceKind::Online => theme::GREEN,
        PresenceKind::InGame => theme::BLUE,
        PresenceKind::Studio => theme::ORANGE,
        PresenceKind::Offline | PresenceKind::Invisible => theme::TEXT_DIM,
    };
    draw_centered(ctx, fb, center, name_baseline + (15.0 * s), &label, 10.5 * s, label_color, false, w as f32 - 8.0 * s);
}

/// Draw a one-line text horizontally centered at `cx`, truncated if needed.
#[allow(clippy::too_many_arguments)]
fn draw_centered(
    ctx: &UiCtx,
    fb: &mut FB,
    cx: f32,
    baseline: f32,
    text: &str,
    px: f32,
    color: theme::Rgba,
    bold: bool,
    max_w: f32,
) {
    let full = font::measure(ctx.fonts, text, px, bold);
    if full <= max_w {
        font::draw(ctx.fonts, fb, cx - full / 2.0, baseline, text, px, color, bold, 0.0);
    } else {
        font::draw_ellipsis(ctx.fonts, fb, cx - max_w / 2.0, baseline, text, px, color, bold, max_w);
    }
}

fn draw_places_grid(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, y: i32, w: i32, h: i32) {
    let s = ctx.scale;
    if h <= 0 {
        return;
    }

    let scroll_w = (theme::SCROLL_W * s) as i32;
    let grid_w = w - scroll_w - (4.0 * s) as i32;
    let card_w = (theme::PLACE_CARD_W * s) as i32;
    let gap = (14.0 * s) as i32;
    let cols = (((grid_w + gap) / (card_w + gap)).max(1)) as usize;
    let card_h = card_w + (88.0 * s) as i32;

    // Empty / loading / error states.
    if v.places.is_empty() {
        let (msg, spin): (String, bool) = if v.places_searching {
            ("Loading places\u{2026}".into(), true)
        } else if let Some(err) = &v.places_error {
            let mut m = format!("Could not load places: {}", err);
            m.truncate(140);
            (m, false)
        } else if v.search.value.trim().is_empty() {
            ("Type a name above to search places".into(), false)
        } else {
            ("No places found".into(), false)
        };
        let mw = font::measure(ctx.fonts, &msg, 14.0 * s, false);
        font::draw(
            ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, y as f32 + h as f32 / 2.0,
            &msg, 14.0 * s, theme::TEXT_FAINT, false, 0.0,
        );
        if spin {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s, y as f32 + h as f32 / 2.0 - 5.0 * s,
                7.0 * s, ctx.now_ms, theme::ACCENT, s,
            );
        }
        v.places_metrics = None;
        return;
    }

    let rows_n = (v.places.len() + cols - 1) / cols;
    let content_h = rows_n as f32 * (card_h + gap) as f32 - gap as f32;
    v.places_scroll = clamp_scroll(v.places_scroll, content_h, h as f32);
    v.places_metrics = Some(ListMetrics { top: y as f32, h: h as f32, content_h: content_h });

    let clip = crate::draw::Clip { x0: x, y0: y, x1: x + grid_w, y1: y + h };
    let old_clip = fb.clip();
    fb.set_clip(clip);
    for (i, card) in v.places.iter().enumerate() {
        let r = i / cols;
        let c = i % cols;
        let cy = y as f32 - v.places_scroll + r as f32 * (card_h + gap) as f32;
        if cy > y as f32 + h as f32 {
            break;
        }
        if cy + (card_h as f32) < y as f32 {
            continue;
        }
        let cx = x + (c as i32) * (card_w + gap);
        draw_place_card(ctx, fb, card, cx, cy as i32, card_w, card_h, s);
    }
    fb.set_clip(old_clip);

    if content_h > h as f32 + 0.5 {
        draw_scrollbar(
            ctx, fb,
            x + w - scroll_w + scroll_w / 2, y, scroll_w, h,
            v.places_scroll, content_h, h as f32, s,
        );
    }
}

fn draw_place_card(ctx: &mut UiCtx, fb: &mut FB, card: &PlaceCard, x: i32, y: i32, w: i32, h: i32, s: f32) {
    let (mx, my) = ctx.cursor;
    let hovered = mx >= x as f32 && mx < (x + w) as f32 && my >= y as f32 && my < (y + h) as f32;
    fb.rect_rounded(x, y, w, h, (10.0 * s) as i32, if hovered { theme::SURFACE_2 } else { theme::SURFACE });

    let pad = (8.0 * s) as i32;
    let icon = w - 2 * pad;
    match ctx.icons.get(&card.root_place_id) {
        Some(rgba) => {
            fb.image(
                x + pad, y + pad, rgba,
                theme::PLACE_ICON_PX as i32, theme::PLACE_ICON_PX as i32,
                icon, icon, (8.0 * s) as i32,
            );
        }
        None => {
            // Placeholder: dark tile with a tiny "photo" glyph (sun + hills).
            fb.rect_rounded(x + pad, y + pad, icon, icon, (8.0 * s) as i32, theme::INPUT_BG);
            let px0 = x + pad;
            let py0 = y + pad;
            let sun = (icon * 3 / 10).max(3);
            fb.circle(px0 + icon / 3, py0 + icon / 3, sun / 2, theme::TEXT_FAINT);
            let base = py0 + icon - icon / 4;
            for i in 0..(icon / 3).max(2) {
                let half = i * 2 / 2 + 1;
                fb.rect(px0 + icon / 2 - half, base - i, half * 2, 1, theme::TEXT_FAINT);
            }
        }
    }

    let text_x = (x + pad) as f32;
    let max_text = (w - 2 * pad) as f32;
    let mut ty = (y + pad + icon) as f32 + (16.0 * s);
    font::draw_ellipsis(ctx.fonts, fb, text_x, ty, &card.name, 12.5 * s, theme::TEXT, true, max_text);

    if !card.creator.is_empty() {
        ty += 14.0 * s;
        font::draw_ellipsis(
            ctx.fonts, fb, text_x, ty, &format!("By {}", card.creator), 11.0 * s,
            theme::TEXT_FAINT, false, max_text,
        );
    }

    // Stats: likes triangle + percent, then player count.
    ty += 15.0 * s;
    let mut sx = text_x;
    if let Some(p) = card.likes_pct {
        tri_up_small(fb, (sx + (4.0 * s)) as i32, (ty - (4.0 * s)) as i32, theme::GREEN);
        sx += 11.0 * s;
        let pct = format!("{}%", p);
        font::draw(ctx.fonts, fb, sx, ty, &pct, 11.0 * s, theme::TEXT_DIM, false, 0.0);
        sx += font::measure(ctx.fonts, &pct, 11.0 * s, false) + (6.0 * s);
        font::draw(ctx.fonts, fb, sx, ty, "\u{b7}", 11.0 * s, theme::TEXT_FAINT, false, 0.0);
        sx += font::measure(ctx.fonts, "\u{b7}", 11.0 * s, false) + (6.0 * s);
    }
    let players = format!("{} playing", format_count(card.player_count));
    font::draw_ellipsis(ctx.fonts, fb, sx, ty, &players, 11.0 * s, theme::TEXT_FAINT, false, (x + w) as f32 - sx - pad as f32);

    // Join button (joining is not implemented yet — visual only).
    let btn_h = (28.0 * s) as i32;
    let btn = Rect { x: x + pad, y: y + h - btn_h - pad, w: w - 2 * pad, h: btn_h };
    let r = draw_button(ctx, fb, btn, "Join", BtnStyle::Success, false);
    ctx.hits.add(r, Hit::JoinPlace(card.root_place_id));
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
// Scrollbar with arrows
// ---------------------------------------------------------------------------

/// Roblox-style vertical scrollbar: up/down arrow buttons plus a draggable
/// thumb. `x` is the track's left edge; hits are registered for both arrows
/// and the whole track (thumb drag).
#[allow(clippy::too_many_arguments)]
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

// ---------------------------------------------------------------------------
// Shared: status bar, avatar
// ---------------------------------------------------------------------------

fn draw_status_bar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, y: i32, h: f32) {
    let s = ctx.scale;
    fb.rect(x, y, w, h as i32, theme::SURFACE);
    fb.hline(x, y, w, theme::BORDER);
    let pad = (16.0 * s) as i32;

    // Transient notice wins over the refresh countdown.
    if matches!(&v.notice, Some((_, until)) if Instant::now() >= *until) {
        v.notice = None;
    }
    let notice_active = matches!(&v.notice, Some((_, until)) if Instant::now() < *until);
    let left = if notice_active {
        v.notice.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
    } else {
        refresh_label(v)
    };
    let notice_color = if notice_active { theme::GREEN } else { theme::TEXT_FAINT };
    font::draw(
        ctx.fonts, fb, (x + pad) as f32, y as f32 + h / 2.0 + 12.0 * s * 0.35,
        &left, 12.0 * s, notice_color, false, 0.0,
    );
    if v.refreshing && !notice_active {
        ui::draw_spinner(
            fb, (x + pad) as f32 + font::measure(ctx.fonts, &left, 12.0 * s, false) + 14.0 * s,
            y as f32 + h / 2.0 - 1.0 * s, 6.0 * s, ctx.now_ms, theme::ACCENT, s,
        );
    }
    if let Some(err) = &v.error {
        let ew = (w as f32 - font::measure(ctx.fonts, &left, 12.0 * s, false) - 60.0 * s).max(0.0);
        font::draw_ellipsis(
            ctx.fonts, fb, (x + w - pad) as f32 - ew.min(w as f32 - 220.0 * s), y as f32 + h / 2.0 + 12.0 * s * 0.35,
            &format!("Error: {}", err), 12.0 * s, theme::RED, false, w as f32 - 220.0 * s,
        );
    }
}

fn refresh_label(v: &MenuView) -> String {
    if v.refreshing {
        return "Updating\u{2026}".to_string();
    }
    match (v.last_update, v.next_refresh) {
        (Some(lu), Some(nr)) => {
            let secs = lu.elapsed().as_secs();
            let next = nr.saturating_duration_since(Instant::now()).as_secs();
            format!("Updated {}s ago \u{b7} next refresh in {}s", secs, next)
        }
        (Some(lu), None) => format!("Updated {}s ago", lu.elapsed().as_secs()),
        _ => String::new(),
    }
}

fn draw_avatar(ctx: &mut UiCtx, fb: &mut FB, id: u64, display_name: &str, x: i32, y: i32, size: i32) {
    let radius = (size as f32 * 0.18) as i32;
    match ctx.avatars.get(&id) {
        Some(rgba) => {
            fb.image(
                x, y, rgba,
                theme::AVATAR_SIZE as i32, theme::AVATAR_SIZE as i32,
                size, size, radius,
            );
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
    use super::{clamp_scroll, format_count};

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

    #[test]
    fn count_formatting() {
        assert_eq!(format_count(98), "98");
        assert_eq!(format_count(1_626), "1.6K");
        assert_eq!(format_count(12_345), "12K");
        assert_eq!(format_count(1_234_567), "1.2M");
    }
}
