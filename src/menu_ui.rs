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
use crate::font::Fonts;
use crate::model::{
    avatar_placeholder_color, format_count, FriendRow, PlaceCard, PresenceKind, UserEntry,
};
use crate::theme::{self, Theme};
use crate::ui::{self, draw_button, BtnStyle, FieldId, Hit, Rect, TextField};
use std::collections::HashMap;
use std::time::Instant;

/// Which main-area pane is open (driven by the sidebar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTab {
    Home,
    Friends,
    Settings,
}

/// Sub-pane of the Friends tab: incoming requests first, search second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriendPane {
    Requests,
    Search,
}

/// Sub-tab of the Settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Appearance,
    Logs,
}

pub struct ListMetrics {
    /// Vertical lists: view top edge. Horizontal strip: view left edge.
    pub top: f32,
    /// Vertical lists: view height. Horizontal strip: view width.
    pub h: f32,
    pub content_h: f32,
}

impl Clone for ListMetrics {
    fn clone(&self) -> Self {
        ListMetrics { top: self.top, h: self.h, content_h: self.content_h }
    }
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

/// State of the "Find friends" pane (search + request sending).
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

/// State of the incoming friend requests pane.
pub struct RequestsState {
    pub items: Vec<UserEntry>,
    pub loading: bool,
    pub loaded_once: bool,
    pub error: Option<String>,
    /// Requests with an accept/decline call in flight.
    pub acting: Vec<u64>,
    pub failed: HashMap<u64, String>,
}

impl RequestsState {
    fn new() -> Self {
        RequestsState {
            items: Vec::new(),
            loading: false,
            loaded_once: false,
            error: None,
            acting: Vec::new(),
            failed: HashMap::new(),
        }
    }
}

/// Settings screen state (sub-tab selection; the values themselves live in
/// `app_settings` and are persisted immediately on every change).
#[derive(Clone, Copy, Debug)]
pub struct SettingsState {
    pub tab: SettingsTab,
    /// Logs pane: keep pinned to the newest line while the user is at the
    /// bottom (set false as soon as they scroll up).
    pub logs_follow: bool,
}

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
    let sb_w = ((if v.collapsed {
        theme::SIDEBAR_W_MINI
    } else {
        theme::SIDEBAR_W_FULL
    }) * s) as i32;

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

fn draw_sidebar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, w: i32, h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    let mini = v.collapsed;
    fb.rect(0, 0, w, h, t.surface);
    fb.rect(w - 1, 0, 1, h, [0x1E, 0x20, 0x22, 0xFF]);

    let pad = (12.0 * s) as i32;

    // Collapse/expand toggle (chevrons).
    let tg = (22.0 * s) as i32;
    let tgx = if mini { (w - tg) / 2 } else { w - pad - tg };
    let tgr = Rect { x: tgx, y: pad, w: tg, h: tg };
    let tghov = tgr.contains(ctx.cursor.0, ctx.cursor.1);
    if tghov {
        fb.rect_rounded(tgr.x, tgr.y, tgr.w, tgr.h, (6.0 * s) as i32, t.surface_2);
    }
    chevron(
        fb,
        tgr.x + tgr.w / 2,
        tgr.y + tgr.h / 2,
        if tghov { t.text } else { t.text_dim },
        mini,
    );
    ctx.hits.add(tgr, Hit::SidebarToggle);
    let mut y = tgr.y + tgr.h + (10.0 * s) as i32;

    // Logo: the slightly rotated red tile IS the "R" of the wordmark.
    // Clicking it refreshes and jumps to the top of the Home screen.
    let sq = (26.0 * s) as i32;
    let tile_cx = if mini { w / 2 } else { pad + sq / 2 + 1 };
    let tile_cy = y + sq / 2;
    let logo_rect = if mini {
        Rect { x: tile_cx - sq / 2 - 4, y: y - 4, w: sq + 8, h: sq + 8 }
    } else {
        Rect { x: pad, y: y - 4, w: w - 2 * pad, h: sq + 8 }
    };
    let logo_hov = logo_rect.contains(ctx.cursor.0, ctx.cursor.1);
    if logo_hov {
        fb.rect_rounded(logo_rect.x, logo_rect.y, logo_rect.w, logo_rect.h, (8.0 * s) as i32, t.surface_2);
    }
    fb.rect_rounded_rot(tile_cx, tile_cy, sq, sq, (6.0 * s) as i32, -0.16, t.red);
    let ir = 14.0 * s;
    let ir_w = font::measure(ctx.fonts, "R", ir, true);
    font::draw(
        ctx.fonts, fb, tile_cx as f32 - ir_w / 2.0, tile_cy as f32 + ir * 0.35, "R", ir,
        t.white, true, 0.0,
    );
    if !mini {
        let wx = (tile_cx + sq / 2 + (6.0 * s) as i32) as f32;
        font::draw(
            ctx.fonts, fb, wx, tile_cy as f32 + 14.0 * s * 0.35, "ustBlox", 14.0 * s,
            t.text, true, 0.0,
        );
    }
    ctx.hits.add(logo_rect, Hit::Logo);
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
            t.text, true, name_w,
        );
        font::draw_ellipsis(
            ctx.fonts, fb, nx, y as f32 + 27.0 * s, &format!("@{}", v.profile.name), 11.0 * s,
            t.text_dim, false, name_w,
        );
    }
    y += avatar_sz + (12.0 * s) as i32;

    // Navigation (no Home button: the logo does that; no Settings entry
    // either — the only one is the gear pinned to the bottom of the sidebar
    // below, in both mini and full layout, so there is exactly one).
    let item_h = (34.0 * s) as i32;
    let item_pad = if mini { (6.0 * s) as i32 } else { pad };
    let item_w = w - 2 * item_pad;
    nav_item(
        ctx, fb, item_pad, y, item_w, item_h, "Friends", icon_person_add,
        v.tab == MenuTab::Friends, true, Hit::NavFriends, None, mini,
    );
    y += item_h + (9.0 * s) as i32;

    if !mini {
        fb.hline(pad, y, w - 2 * pad, t.border);
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

    // Bottom: THE single Settings entry point — the gear, pinned here in
    // both sidebar layouts (account actions live in Settings). In full mode
    // the version label sits next to it, right-aligned.
    let bw = (34.0 * s) as i32;
    let bx = if mini { (w - bw) / 2 } else { pad };
    let by = h - bw - (12.0 * s) as i32;
    let hovered = Rect { x: bx, y: by, w: bw, h: bw }.contains(ctx.cursor.0, ctx.cursor.1);
    if hovered {
        fb.rect_rounded(bx, by, bw, bw, (8.0 * s) as i32, t.surface_2);
    }
    icon_gear(
        fb, bx + (bw - (16.0 * s) as i32) / 2, by + (bw - (16.0 * s) as i32) / 2,
        (16.0 * s) as i32, if hovered { t.text } else { t.text_dim },
    );
    ctx.hits.add(Rect { x: bx, y: by, w: bw, h: bw }, Hit::NavSettings);
    if !mini {
        let ver = format!("RustBlox v{}", env!("CARGO_PKG_VERSION"));
        let vw = font::measure(ctx.fonts, &ver, 10.5 * s, false);
        font::draw(
            ctx.fonts, fb, (w - pad) as f32 - vw, by as f32 + bw as f32 * 0.5 + 10.5 * s * 0.35,
            &ver, 10.5 * s, t.text_faint, false, 0.0,
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
    let t = theme::cur();
    let hovered = enabled
        && ctx.cursor.0 >= x as f32
        && ctx.cursor.0 < (x + w) as f32
        && ctx.cursor.1 >= y as f32
        && ctx.cursor.1 < (y + h) as f32;
    if active || hovered {
        fb.rect_rounded(x, y, w, h, (8.0 * s) as i32, t.surface_2);
    }
    if active {
        let bar_h = (16.0 * s) as i32;
        fb.rect(x + (3.0 * s) as i32, y + (h - bar_h) / 2, (3.0 * s).max(1.0) as i32, bar_h, t.accent);
    }

    let ic = (16.0 * s) as i32;
    let (icon_c, text_c) = if active || hovered {
        (t.text, t.text)
    } else if enabled {
        (t.text_dim, t.text_dim)
    } else {
        (t.text_faint, t.text_faint)
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
            font::draw(ctx.fonts, fb, bx, ty, b, 10.0 * s, t.text_faint, false, 0.0);
        }
    }
    if enabled {
        ctx.hits.add(Rect { x, y, w, h }, hit);
    }
}

// ---------------------------------------------------------------------------
// Icons (tiny vector glyphs built from primitives)
// ---------------------------------------------------------------------------

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

fn icon_gear(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
    // Proper gear: ring body + 8 teeth on the rim + open hub. Built from a
    // ring (no background-colored punching), so it renders correctly on any
    // surface including hover highlights.
    let cx = x + sz / 2;
    let cy = y + sz / 2;
    let ro = (sz / 2 - 1).max(3);
    let ri = (ro * 45 / 100).max(1);
    let tooth = ((sz / 8) as i32).max(2);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (sn, cs) = a.sin_cos();
        let tx = cx + (cs * ro as f32).round() as i32;
        let ty = cy + (sn * ro as f32).round() as i32;
        fb.rect(tx - tooth / 2, ty - tooth / 2, tooth, tooth, color);
    }
    fb.ring(cx, cy, ro, ri, color);
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
        fb.circle(dx, y + (sz - 5) / 2 + 1, d, crate::theme::cur().surface);
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
    // Apex on the LEFT, tall edge on the right (points outward).
    for i in 0..5 {
        fb.rect(cx + 4 - i, cy - (4 - i), 1, (4 - i) * 2 + 1, color);
    }
}

fn tri_right(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
    // Apex on the RIGHT, tall edge on the left (points outward).
    for i in 0..5 {
        fb.rect(cx - 4 + i, cy - (4 - i), 1, (4 - i) * 2 + 1, color);
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
// Home pane: search, friends strip, sectioned places content
// ---------------------------------------------------------------------------

fn draw_home(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    let pad = (16.0 * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;
    let mut y = (14.0 * s) as i32;

    // -- Places search row (fixed above the scroll area) --
    let field_h = (34.0 * s) as i32;
    let btn_w = (92.0 * s) as i32;
    let field_w = (w - 2 * pad - btn_w - (10.0 * s) as i32).max((120.0 * s) as i32).min((560.0 * s) as i32);
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
            btn_rect.y as f32 + btn_rect.h as f32 / 2.0, 5.0 * s, ctx.now_ms, t.btn_text, s,
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
enum HomeItem {
    /// "Friends" title + summary + horizontal strip (scrolls with the page).
    Friends,
    Header(String),
    /// Centered status message (loading / error / empty feed states).
    Message { text: String, spin: bool },
    Card(PlaceCard),
}

fn draw_home_content(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, y: i32, w: i32, h: i32) {
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
                font::draw(ctx.fonts, fb, x as f32, iy + 11.0 * s, "Friends", 15.0 * s, t.text, true, 0.0);
                let online = v.rows.iter().filter(|r| r.presence.kind != PresenceKind::Offline && r.presence.kind != PresenceKind::Invisible).count();
                let ingame = v.rows.iter().filter(|r| r.presence.kind == PresenceKind::InGame).count();
                let summary = format!("{} online \u{b7} {} in game \u{b7} {} friends", online, ingame, v.rows.len());
                let sum_w = font::measure(ctx.fonts, &summary, 11.5 * s, false);
                font::draw(
                    ctx.fonts, fb, (x + w) as f32 - sum_w, iy + 11.5 * s * 0.35 + 4.0 * s,
                    &summary, 11.5 * s, t.text_dim, false, 0.0,
                );
                let strip_y = (iy + 26.0 * s) as i32;
                draw_friends_strip(v, ctx, fb, x, strip_y, w, (theme::STRIP_H * s) as i32);
            }
            HomeItem::Message { text, spin } => {
                if iy + msg_h < y as f32 {
                    continue;
                }
                let mw = font::measure(ctx.fonts, text, 14.0 * s, false);
                let my = iy + msg_h * 0.5;
                font::draw(
                    ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, my,
                    text, 14.0 * s, t.text_faint, false, 0.0,
                );
                if *spin {
                    ui::draw_spinner(
                        fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s, my - 5.0 * s,
                        7.0 * s, ctx.now_ms, t.accent, s,
                    );
                }
            }
            HomeItem::Header(text) => {
                if iy + (header_h as f32) < y as f32 {
                    continue;
                }
                font::draw(
                    ctx.fonts, fb, x as f32, iy + 15.0 * s, text, 15.0 * s, t.text, true, 0.0,
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
            ctx, fb,
            x + w - scroll_w + scroll_w / 2, y, scroll_w, h,
            v.places_scroll, content_h, h as f32, s,
        );
    }
}

/// Number of cards since the last header (the column within the section;
/// take `% cols` when converting to an x offset).
fn col_of(items: &[HomeItem], i: usize) -> usize {
    let mut col = 0usize;
    for it in items.iter().take(i) {
        match it {
            HomeItem::Header(_) | HomeItem::Friends | HomeItem::Message { .. } => col = 0,
            HomeItem::Card(_) => col += 1,
        }
    }
    col
}

fn draw_friends_strip(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, y: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    fb.rect_rounded(x, y, w, h, (12.0 * s) as i32, t.surface);
    v.strip_rect = Some(Rect { x, y, w, h });

    if v.rows.is_empty() {
        let (msg, spin) = if v.refreshing && v.last_update.is_none() {
            ("Loading friends\u{2026}", true)
        } else {
            ("No friends yet \u{2014} open Friends to find people", false)
        };
        let mw = font::measure(ctx.fonts, msg, 13.0 * s, false);
        font::draw(
            ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, y as f32 + h as f32 / 2.0,
            msg, 13.0 * s, t.text_faint, false, 0.0,
        );
        if spin {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 16.0 * s, y as f32 + h as f32 / 2.0 - 4.0 * s,
                6.0 * s, ctx.now_ms, t.accent, s,
            );
        }
        v.strip_metrics = None;
        return;
    }

    let arrow_w = (26.0 * s) as i32;
    let inset = (6.0 * s) as i32;
    let view_w = (w - 2 * arrow_w - 2 * inset) as f32;
    // Card width scales with the theme but never overflows the strip view.
    let card_w = ((theme::STRIP_CARD_W * s) as i32).min(view_w as i32).max(1);
    let gap = (8.0 * s) as i32;
    let content_w = v.rows.len() as f32 * (card_w + gap) as f32;
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
    let t = theme::cur();
    let hovered = !dim && r.contains(ctx.cursor.0, ctx.cursor.1);
    if hovered {
        fb.rect_rounded(r.x + 1, r.y + 1, r.w - 2, r.h - 2, (8.0 * s) as i32, t.surface_2);
    }
    let color = if dim { t.text_faint } else if hovered { t.text } else { t.text_dim };
    if left {
        tri_left(fb, r.x + r.w / 2, r.y + r.h / 2, color);
    } else {
        tri_right(fb, r.x + r.w / 2, r.y + r.h / 2, color);
    }
    ctx.hits.add(r, hit);
}

fn draw_friend_card(ctx: &mut UiCtx, fb: &mut FB, row: &FriendRow, x: i32, y: i32, w: i32, h: i32, s: f32) {
    let s2 = s;
    let t = theme::cur();
    let hovered = Rect { x, y, w, h }.contains(ctx.cursor.0, ctx.cursor.1);
    if hovered {
        fb.rect_rounded(x + 2, y + 2, w - 4, h - 4, (10.0 * s2) as i32, t.surface_2);
    }

    let a = (56.0 * s2) as i32;
    let ax = x + (w - a) / 2;
    let ay = y + (8.0 * s2) as i32;
    draw_avatar(ctx, fb, row.friend.id, &row.friend.display_name, ax, ay, a);

    // Presence dot pinned to the avatar corner.
    let d = (12.0 * s2) as i32;
    let dx = ax + a - d + 1;
    let dy = ay + a - d + 1;
    fb.circle(dx + d / 2, dy + d / 2, d / 2 + (1.5 * s2) as i32, t.surface);
    fb.circle(dx + d / 2, dy + d / 2, d / 2, row.presence.kind.dot_color());

    let center = x as f32 + w as f32 / 2.0;
    let name_baseline = (ay + a) as f32 + (18.0 * s2);
    draw_centered(ctx, fb, center, name_baseline, &row.friend.display_name, 12.0 * s2, t.text, true, w as f32 - 10.0 * s2);
    let label = row.status_label();
    let label_color = match row.presence.kind {
        PresenceKind::Online => t.green,
        PresenceKind::InGame => t.blue,
        PresenceKind::Studio => t.orange,
        PresenceKind::Offline | PresenceKind::Invisible => t.text_dim,
    };
    draw_centered(ctx, fb, center, name_baseline + (15.0 * s2), &label, 10.5 * s2, label_color, false, w as f32 - 8.0 * s2);
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

fn draw_place_card(ctx: &mut UiCtx, fb: &mut FB, card: &PlaceCard, x: i32, y: i32, w: i32, h: i32, s: f32) {
    let t = theme::cur();
    let hovered = Rect { x, y, w, h }.contains(ctx.cursor.0, ctx.cursor.1);
    fb.rect_rounded(x, y, w, h, (10.0 * s) as i32, if hovered { t.surface_2 } else { t.surface });

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
            fb.rect_rounded(x + pad, y + pad, icon, icon, (8.0 * s) as i32, t.input_bg);
            let px0 = x + pad;
            let py0 = y + pad;
            let sun = (icon * 3 / 10).max(3);
            fb.circle(px0 + icon / 3, py0 + icon / 3, sun / 2, t.text_faint);
            let base = py0 + icon - icon / 4;
            for i in 0..(icon / 3).max(2) {
                let half = i * 2 / 2 + 1;
                fb.rect(px0 + icon / 2 - half, base - i, half * 2, 1, t.text_faint);
            }
        }
    }

    let text_x = (x + pad) as f32;
    let max_text = (w - 2 * pad) as f32;
    let mut ty = (y + pad + icon) as f32 + (16.0 * s);
    font::draw_ellipsis(ctx.fonts, fb, text_x, ty, &card.name, 12.5 * s, t.text, true, max_text);

    if !card.creator.is_empty() {
        ty += 14.0 * s;
        font::draw_ellipsis(
            ctx.fonts, fb, text_x, ty, &format!("By {}", card.creator), 11.0 * s,
            t.text_faint, false, max_text,
        );
    }

    // Stats: likes triangle + percent, then player count (locally tracked
    // cards have no live stats — skip the row entirely then).
    if card.likes_pct.is_some() || card.player_count > 0 {
        ty += 15.0 * s;
        let mut sx = text_x;
        if let Some(p) = card.likes_pct {
            tri_up_small(fb, (sx + (4.0 * s)) as i32, (ty - (4.0 * s)) as i32, t.green);
            sx += 11.0 * s;
            let pct = format!("{}%", p);
            font::draw(ctx.fonts, fb, sx, ty, &pct, 11.0 * s, t.text_dim, false, 0.0);
            sx += font::measure(ctx.fonts, &pct, 11.0 * s, false) + (6.0 * s);
            font::draw(ctx.fonts, fb, sx, ty, "\u{b7}", 11.0 * s, t.text_faint, false, 0.0);
            sx += font::measure(ctx.fonts, "\u{b7}", 11.0 * s, false) + (6.0 * s);
        }
        if card.player_count > 0 {
            let players = format!("{} playing", format_count(card.player_count));
            font::draw_ellipsis(ctx.fonts, fb, sx, ty, &players, 11.0 * s, t.text_faint, false, (x + w) as f32 - sx - pad as f32);
        }
    }

    // Join button (joining is not implemented yet — visual only).
    let btn_h = (28.0 * s) as i32;
    let btn = Rect { x: x + pad, y: y + h - btn_h - pad, w: w - 2 * pad, h: btn_h };
    let r = draw_button(ctx, fb, btn, "Join", BtnStyle::Success, false);
    ctx.hits.add(r, Hit::JoinPlace(card.root_place_id));
}

// ---------------------------------------------------------------------------
// Friends pane: incoming requests + find-friends search
// ---------------------------------------------------------------------------

fn draw_friends(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    let pad = (18.0 * s) as i32;
    let header_h = (54.0 * s) as i32;
    let tabs_h = (46.0 * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;

    // Header.
    font::draw(ctx.fonts, fb, x as f32 + pad as f32, header_h as f32 / 2.0 + 9.5 * s, "Friends", 19.0 * s, t.text, true, 0.0);
    let hint = match v.friend_pane {
        FriendPane::Requests => "People who want to add you",
        FriendPane::Search => "Find people by username and send a friend request",
    };
    let hw = font::measure(ctx.fonts, hint, 12.5 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - hw, header_h as f32 / 2.0 + 12.5 * s * 0.35,
        hint, 12.5 * s, t.text_dim, false, 0.0,
    );
    fb.hline(x, header_h, w, [0x1E, 0x20, 0x22, 0xFF]);

    // Pane tabs (share the row; never overflow narrow screens).
    let tab_h = (32.0 * s) as i32;
    let tw = (((w - 2 * pad) as f32 - (8.0 * s)) / 2.0).min(200.0 * s) as i32;
    draw_seg_tab(ctx, fb, Rect { x: x + pad, y: header_h + (tabs_h - tab_h) / 2, w: tw, h: tab_h }, "Requests", v.friend_pane == FriendPane::Requests, Hit::TabRequests);
    draw_seg_tab(ctx, fb, Rect { x: x + pad + tw + (8.0 * s) as i32, y: header_h + (tabs_h - tab_h) / 2, w: tw, h: tab_h }, "Find friends", v.friend_pane == FriendPane::Search, Hit::TabSearch);

    let list_top = (header_h + tabs_h) as f32;
    let list_h = (h - status_h) as f32 - list_top;

    match v.friend_pane {
        FriendPane::Requests => draw_requests_pane(v, ctx, fb, x, w, list_top, list_h),
        FriendPane::Search => draw_search_pane(v, ctx, fb, x, w, h, list_top, list_h),
    }
}

fn draw_seg_tab(ctx: &mut UiCtx, fb: &mut FB, r: Rect, label: &str, active: bool, hit: Hit) {
    let s = ctx.scale;
    let t = theme::cur();
    if active {
        fb.rect_rounded(r.x, r.y, r.w, r.h, (8.0 * s) as i32, t.surface_2);
        fb.rect(r.x + (10.0 * s) as i32, r.y + r.h - (3.0 * s).max(1.0) as i32, r.w - (20.0 * s) as i32, (3.0 * s).max(1.0) as i32, t.accent);
    }
    let tw = font::measure(ctx.fonts, label, 13.0 * s, true);
    font::draw(
        ctx.fonts, fb, r.x as f32 + (r.w as f32 - tw) * 0.5, r.y as f32 + r.h as f32 * 0.5 + 13.0 * s * 0.35,
        label, 13.0 * s, if active { t.text } else { t.text_dim }, true, 0.0,
    );
    ctx.hits.add(r, hit);
}

fn draw_requests_pane(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, top: f32, list_h: f32) {
    let s = ctx.scale;
    let t = theme::cur();
    let pad = (18.0 * s) as i32;
    let scroll_w = (theme::SCROLL_W * s) as i32;
    let rows_x = (x + pad) as f32;
    let rows_w = (w - 2 * pad - scroll_w) as f32;
    let row_h = (64.0 * s) as f32;
    let content_h = v.requests.items.len() as f32 * row_h;
    v.requests_scroll = clamp_scroll(v.requests_scroll, content_h, list_h);
    v.requests_metrics = Some(ListMetrics { top, h: list_h, content_h });

    let list_clip = crate::draw::Clip { x0: x, y0: top as i32, x1: x + w, y1: (top + list_h) as i32 };
    let old_clip = fb.clip();
    fb.set_clip(list_clip);

    if v.requests.items.is_empty() {
        fb.set_clip(old_clip);
        let (msg, spin): (String, bool) = if v.requests.loading {
            ("Loading requests\u{2026}".into(), true)
        } else if let Some(err) = &v.requests.error {
            let mut m = format!("Error: {}", err);
            m.truncate(120);
            (m, false)
        } else {
            ("No incoming friend requests".into(), false)
        };
        let mw = font::measure(ctx.fonts, &msg, 14.0 * s, false);
        font::draw(
            ctx.fonts, fb, x as f32 + w as f32 / 2.0 - mw / 2.0, top + list_h / 2.0,
            &msg, 14.0 * s, t.text_faint, false, 0.0,
        );
        if spin {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s, top + list_h / 2.0 - 5.0 * s,
                7.0 * s, ctx.now_ms, t.accent, s,
            );
        }
        return;
    }

    let first = (v.requests_scroll / row_h) as usize;
    for (vi, user) in v.requests.items.iter().enumerate().skip(first) {
        let y = top - v.requests_scroll + vi as f32 * row_h;
        if y > top + list_h {
            break;
        }
        if y + row_h < top {
            continue;
        }
        draw_request_row(&v.requests, ctx, fb, user, rows_x, y, rows_w, row_h, s);
    }
    fb.set_clip(old_clip);
    draw_scrollbar(
        ctx, fb,
        x + w - pad - scroll_w + scroll_w / 2, top as i32, scroll_w, list_h as i32,
        v.requests_scroll, content_h, list_h, s,
    );
}

fn draw_request_row(reqs: &RequestsState, ctx: &mut UiCtx, fb: &mut FB, user: &UserEntry, x: f32, y: f32, w: f32, h: f32, s: f32) {
    let t = theme::cur();
    let hovered = Rect { x: x as i32, y: y as i32, w: w as i32, h: h as i32 }.contains(ctx.cursor.0, ctx.cursor.1);
    if hovered {
        fb.rect(x as i32, y as i32, w as i32, h as i32, t.surface_2);
    }

    let pad = (12.0 * s) as i32;
    let avatar_sz = (42.0 * s) as i32;
    let avatar_y = y as i32 + ((h as i32) - avatar_sz) / 2;
    draw_avatar(ctx, fb, user.id, user.display_or_name(), x as i32 + pad, avatar_y, avatar_sz);

    let text_x = x + (pad + avatar_sz + (12.0 * s) as i32) as f32;
    let btn_w = (86.0 * s) as i32;
    let btn_h = (30.0 * s) as i32;
    let max_text_w = w - (text_x - x) - (btn_w * 2 + (8.0 * s) as i32) as f32 - (24.0 * s);

    font::draw_ellipsis(
        ctx.fonts, fb, text_x, y + 24.0 * s, user.display_or_name(), 14.0 * s, t.text, true, max_text_w,
    );
    let failed = reqs.failed.get(&user.id);
    let sub_color = if failed.is_some() { t.red } else { t.text_dim };
    let sub = match failed {
        Some(msg) => {
            let mut m = msg.clone();
            if m.len() > 80 {
                m.truncate(80);
            }
            m
        }
        None => format!("@{}", user.name),
    };
    font::draw_ellipsis(ctx.fonts, fb, text_x, y + 43.0 * s, &sub, 12.0 * s, sub_color, false, max_text_w);

    let busy = reqs.acting.contains(&user.id);
    let by = (y + (h - btn_h as f32) / 2.0) as i32;
    let accept_r = Rect { x: (x + w - pad as f32 - (btn_w * 2 + (8.0 * s) as i32) as f32) as i32, y: by, w: btn_w, h: btn_h };
    let decline_r = Rect { x: accept_r.x + btn_w + (8.0 * s) as i32, y: by, w: btn_w, h: btn_h };
    let ra = draw_button(ctx, fb, accept_r, "Accept", BtnStyle::Success, busy);
    let rd = draw_button(ctx, fb, decline_r, "Decline", BtnStyle::Secondary, busy);
    if !busy {
        ctx.hits.add(ra, Hit::RequestAccept(user.id));
        ctx.hits.add(rd, Hit::RequestDecline(user.id));
    }

    fb.hline(x as i32, (y + h) as i32, w as i32, [0x1E, 0x20, 0x22, 0xFF]);
}

fn draw_search_pane(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32, top: f32, _list_h: f32) {
    let s = ctx.scale;
    let t = theme::cur();
    let pad = (18.0 * s) as i32;

    // Search row: field + button.
    let field_h = (36.0 * s) as i32;
    let field_y = top as i32 + ((14.0 * s) as i32);
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
            5.5 * s, ctx.now_ms, t.btn_text, s,
        );
    }

    // Results list.
    let list_top = field_y as f32 + field_h as f32 + (16.0 * s);
    let list_h = (h as f32 - (theme::STATUSBAR_H * s)) - list_top;
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
        // Centered word-wrapped block (long errors must not run off the pane).
        let msg_px = 14.0 * s;
        let msg_lh = 19.0 * s;
        let segs = font::wrap_text(ctx.fonts, &msg, w as f32 - 48.0 * s, msg_px, false);
        let block_h = segs.len() as f32 * msg_lh;
        let mid_y = list_top + list_h / 2.0 - block_h / 2.0 + msg_px * 0.85;
        let mut mw = 0.0f32;
        for (i, seg) in segs.iter().enumerate() {
            let seg_w = font::measure(ctx.fonts, seg, msg_px, false);
            mw = mw.max(seg_w);
            font::draw(
                ctx.fonts, fb, x as f32 + w as f32 / 2.0 - seg_w / 2.0,
                mid_y + i as f32 * msg_lh,
                seg, msg_px, t.text_faint, false, 0.0,
            );
        }
        if spin {
            ui::draw_spinner(
                fb, x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s, list_top + list_h / 2.0 - 5.0 * s,
                7.0 * s, ctx.now_ms, t.accent, s,
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
    let t = theme::cur();
    let hovered = Rect { x: x as i32, y: y as i32, w: w as i32, h: h as i32 }.contains(ctx.cursor.0, ctx.cursor.1);
    if hovered {
        fb.rect(x as i32, y as i32, w as i32, h as i32, t.surface_2);
    }

    let pad = (12.0 * s) as i32;
    let avatar_sz = (42.0 * s) as i32;
    let avatar_y = y as i32 + ((h as i32) - avatar_sz) / 2;
    draw_avatar(ctx, fb, user.id, user.display_or_name(), x as i32 + pad, avatar_y, avatar_sz);

    let text_x = x + (pad + avatar_sz + (12.0 * s) as i32) as f32;
    let btn_w = (92.0 * s) as i32;
    let max_text_w = w - (text_x - x) - btn_w as f32 - (24.0 * s);

    let state = add.state_of(user.id);
    let name_c = t.text;
    font::draw_ellipsis(
        ctx.fonts, fb, text_x, y + 23.0 * s, user.display_or_name(), 14.0 * s, name_c, true, max_text_w,
    );
    let sub = format!("@{}", user.name);
    let sub_c = match &state {
        RequestState::Failed(_) => t.red,
        _ => t.text_dim,
    };
    let sub_text = match &state {
        RequestState::Failed(msg) => {
            let mut t2 = msg.clone();
            if t2.len() > 80 {
                t2.truncate(80);
            }
            t2
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
                "Request sent", 12.5 * s, t.green, false, 0.0,
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
// Settings pane: account, theme, appearance
// ---------------------------------------------------------------------------

fn draw_settings(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    let pad = (18.0 * s) as i32;
    let header_h = (54.0 * s) as i32;
    let tabs_h = (46.0 * s) as i32;
    let status_h = (theme::STATUSBAR_H * s) as i32;

    font::draw(ctx.fonts, fb, x as f32 + pad as f32, header_h as f32 / 2.0 + 9.5 * s, "Settings", 19.0 * s, t.text, true, 0.0);
    let hint = "Account, themes and diagnostics";
    let hw = font::measure(ctx.fonts, hint, 12.5 * s, false);
    font::draw(
        ctx.fonts, fb, (x + w - pad) as f32 - hw, header_h as f32 / 2.0 + 12.5 * s * 0.35,
        hint, 12.5 * s, t.text_dim, false, 0.0,
    );
    fb.hline(x, header_h, w, [0x1E, 0x20, 0x22, 0xFF]);

    let tab_h = (32.0 * s) as i32;
    let tw = (((w - 2 * pad) as f32 - 2.0 * (8.0 * s)) / 3.0).min(120.0 * s) as i32;
    let tabs_y = header_h + (tabs_h - tab_h) / 2;
    draw_seg_tab(ctx, fb, Rect { x: x + pad, y: tabs_y, w: tw, h: tab_h }, "General", v.settings.tab == SettingsTab::General, Hit::SettingsTab(0));
    draw_seg_tab(ctx, fb, Rect { x: x + pad + tw + (8.0 * s) as i32, y: tabs_y, w: tw, h: tab_h }, "Appearance", v.settings.tab == SettingsTab::Appearance, Hit::SettingsTab(1));
    draw_seg_tab(ctx, fb, Rect { x: x + pad + 2 * (tw + (8.0 * s) as i32), y: tabs_y, w: tw, h: tab_h }, "Logs", v.settings.tab == SettingsTab::Logs, Hit::SettingsTab(2));

    let top = (header_h + tabs_h) as f32;
    let view_h = (h - status_h) as f32 - top;
    // The Logs pane reserves a fixed footer row for the action buttons.
    let footer_h = if v.settings.tab == SettingsTab::Logs { 46.0 * s } else { 0.0 };
    let list_view = (view_h - footer_h).max(0.0);

    // Measure content first (any tab can exceed the viewport).
    let scroll_w = (theme::SCROLL_W * s) as i32;
    let inner_w = (w - 2 * pad - scroll_w) as f32;
    let content_h = match v.settings.tab {
        SettingsTab::General => measure_general(v, s),
        SettingsTab::Appearance => measure_appearance(s),
        SettingsTab::Logs => measure_logs(ctx.fonts, inner_w, s),
    };
    if v.settings.logs_follow && v.settings.tab == SettingsTab::Logs {
        v.settings_scroll = (content_h - list_view).max(0.0);
    }
    v.settings_scroll = clamp_scroll(v.settings_scroll, content_h, list_view);
    v.settings_metrics = Some(ListMetrics { top, h: list_view, content_h });

    let clip = crate::draw::Clip { x0: x, y0: top as i32, x1: x + w, y1: (top + list_view) as i32 };
    let old_clip = fb.clip();
    fb.set_clip(clip);
    let mut y = top - v.settings_scroll;
    match v.settings.tab {
        SettingsTab::General => draw_general(v, ctx, fb, x + pad, &mut y, inner_w, s),
        SettingsTab::Appearance => draw_appearance(ctx, fb, x + pad, &mut y, inner_w, s, &mut v.app_settings),
        SettingsTab::Logs => draw_logs(ctx, fb, x + pad, &mut y, inner_w, s),
    }
    fb.set_clip(old_clip);

    if content_h > list_view + 0.5 {
        draw_scrollbar(
            ctx, fb,
            x + w - pad - scroll_w + scroll_w / 2, top as i32, scroll_w, list_view as i32,
            v.settings_scroll, content_h, list_view, s,
        );
    }

    // Logs footer: action buttons pinned under the log view.
    if v.settings.tab == SettingsTab::Logs {
        let btn_h = (30.0 * s) as i32;
        let fy = (top + view_h - 46.0 * s) as i32;
        let copy_w = (120.0 * s) as i32;
        let r1 = Rect { x: x + pad, y: fy, w: copy_w, h: btn_h };
        let b1 = draw_button(ctx, fb, r1, "Copy logs", BtnStyle::Primary, false);
        ctx.hits.add(b1, Hit::CopyLogs);
        let save_w = (86.0 * s) as i32;
        let r3 = Rect { x: x + pad + copy_w + (10.0 * s) as i32, y: fy, w: save_w, h: btn_h };
        let b3 = draw_button(ctx, fb, r3, "Save", BtnStyle::Primary, false);
        ctx.hits.add(b3, Hit::SaveLogs);
        let clear_w = (90.0 * s) as i32;
        let r2 = Rect { x: x + pad + copy_w + save_w + (20.0 * s) as i32, y: fy, w: clear_w, h: btn_h };
        let b2 = draw_button(ctx, fb, r2, "Clear", BtnStyle::Secondary, false);
        ctx.hits.add(b2, Hit::ClearLogs);
        let count = crate::logs::len();
        let label = if count == 1 { "1 line".to_string() } else { format!("{} lines", count) };
        let lw = font::measure(ctx.fonts, &label, 11.5 * s, false);
        font::draw(
            ctx.fonts, fb,
            (x + w - pad) as f32 - lw,
            fy as f32 + btn_h as f32 / 2.0 + 11.5 * s * 0.35,
            &label, 11.5 * s, t.text_dim, false, 0.0,
        );
    }
}

/// Diagnostic log pane: every buffered line, greedy-wrapped to the pane
/// width. Newest lines sit at the bottom; the follow flag pins the view to
/// the tail until the user scrolls up.
const LOG_LINE_H: f32 = 17.0;

fn measure_logs(fonts: &Fonts, inner_w: f32, s: f32) -> f32 {
    let px = 11.5 * s;
    let line_h = LOG_LINE_H * s;
    let mut h = 8.0 * s;
    let lines = crate::logs::lines();
    if lines.is_empty() {
        // Keep in sync with draw_logs' empty state (the hint wraps too).
        let segs = font::wrap_text(
            fonts,
            "Try joining a place — every request, response and error lands here.",
            inner_w.max(32.0),
            px,
            false,
        );
        h += (segs.len() + 1) as f32 * line_h;
    } else {
        for l in &lines {
            h += log_layout(fonts, l, inner_w, px).2.len() as f32 * line_h;
        }
    }
    h + 12.0 * s
}

fn draw_logs(ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut f32, w: f32, s: f32) {
    let t = theme::cur();
    let px = 11.5 * s;
    let line_h = LOG_LINE_H * s;
    section_label(ctx, fb, x, y, "DIAGNOSTIC LOG", s);
    let lines = crate::logs::lines();
    if lines.is_empty() {
        font::draw(ctx.fonts, fb, x as f32, *y + 14.0 * s, "No events yet.", px, t.text_faint, false, 0.0);
        let hint_h = font::draw_wrapped(
            ctx.fonts, fb, x as f32, *y + 14.0 * s + line_h,
            "Try joining a place — every request, response and error lands here.",
            px, t.text_faint, false, w.max(32.0), line_h,
        );
        *y += line_h + hint_h;
        return;
    }
    for l in &lines {
        let (head, head_w, segs) = log_layout(ctx.fonts, l, w, px);
        for (k, seg) in segs.iter().enumerate() {
            let (sx, base) = if k == 0 {
                (x as f32 + head_w, *y)
            } else {
                (x as f32, *y + k as f32 * line_h)
            };
            if seg.is_empty() {
                continue;
            }
            let color = if k == 0 { t.text } else { t.text_dim };
            if k == 0 && !head.is_empty() {
                font::draw(ctx.fonts, fb, x as f32, *y + px * 0.85, &head, px, t.text_dim, false, 0.0);
            }
            font::draw(ctx.fonts, fb, sx, base + px * 0.85, seg, px, color, false, 0.0);
        }
        *y += segs.len() as f32 * line_h;
    }
}

/// Split a stored log line "[HH:MM:SS] [kind] message" into the dim header
/// (timestamp + kind tag) and the message, then greedy-wrap the message to
/// the pane width (long words are broken per character). The first visual
/// line starts after the header, the rest span the full width. Used for
/// BOTH measuring and drawing so the scroll height can never drift.
fn log_layout(fonts: &Fonts, line: &str, max_w: f32, px: f32) -> (String, f32, Vec<String>) {
    let space_w = font::measure(fonts, " ", px, false);
    // Header = up to the second "]" ("[21:53:21]" + "[net]").
    let first_br = line.find(']');
    let (head, msg) = match first_br {
        Some(fb1) => {
            let rest = &line[fb1 + 1..];
            match rest.find("] ") {
                Some(off) => {
                    let second = fb1 + 1 + off + 1;
                    (line[..second + 1].to_string(), &line[second + 2..])
                }
                None => (String::new(), line),
            }
        }
        None => (String::new(), line),
    };
    let head_w = if head.is_empty() {
        0.0
    } else {
        font::measure(fonts, &head, px, false) + space_w
    };
    let first_w = (max_w - head_w).max(32.0);
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0f32;
    let mut cap = first_w;
    let flush = |segs: &mut Vec<String>, cur: &mut String, cur_w: &mut f32| {
        segs.push(std::mem::take(cur));
        *cur_w = 0.0;
    };
    for word in msg.split(' ') {
        if word.is_empty() {
            continue;
        }
        let ww = font::measure(fonts, word, px, false);
        if !cur.is_empty() && cur_w + space_w + ww > cap {
            flush(&mut segs, &mut cur, &mut cur_w);
            cap = max_w;
        }
        if !cur.is_empty() {
            cur.push(' ');
            cur_w += space_w;
        }
        if ww > cap {
            // Hard-break a word wider than the whole line.
            for ch in word.chars() {
                let c = ch.to_string();
                let cw = font::measure(fonts, &c, px, false);
                if cur_w + cw > cap && !cur.is_empty() {
                    flush(&mut segs, &mut cur, &mut cur_w);
                    cap = max_w;
                }
                cur.push_str(&c);
                cur_w += cw;
            }
        } else {
            cur.push_str(word);
            cur_w += ww;
        }
    }
    let _ = cur_w;
    if !cur.is_empty() {
        segs.push(cur);
    }
    if segs.is_empty() {
        segs.push(String::new());
    }
    (head, head_w, segs)
}

/// Rough content height of the General tab (kept in sync with draw_general).
fn measure_general(v: &MenuView, s: f32) -> f32 {
    let others = v.accounts.iter().filter(|a| a.user_id != v.profile.id).count();
    let mut y = 6.0 * s;
    y += 24.0 * s; // ACCOUNT label
    y += 52.0 * s; // account row
    y += 42.0 * s; // add/sign-out buttons
    if others > 0 {
        y += 24.0 * s; // SWITCH ACCOUNT label
        y += others as f32 * (56.0 * s);
    }
    y += 14.0 * s + 24.0 * s; // THEME label
    y += 38.0 * s; // preset chips
    y += 14.0 * s + 24.0 * s; // HOME label
    y += 36.0 * s; // toggle row
    y += 22.0 * s; // version
    y + 16.0 * s
}

fn draw_general(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut f32, w: f32, s: f32) {
    let t = theme::cur();
    let pad = (12.0 * s) as i32;

    section_label(ctx, fb, x, y, "ACCOUNT", s);
    // Current account row.
    let avatar_sz = (40.0 * s) as i32;
    draw_avatar(ctx, fb, v.profile.id, &v.profile.display_name, x as i32, (*y + 4.0 * s) as i32, avatar_sz);
    let nx = x as f32 + (pad + avatar_sz + (12.0 * s) as i32) as f32;
    let name_w = w - (nx - x as f32) - 8.0 * s;
    font::draw_ellipsis(ctx.fonts, fb, nx, *y + 20.0 * s, &v.profile.display_name, 14.0 * s, t.text, true, name_w);
    font::draw_ellipsis(ctx.fonts, fb, nx, *y + 38.0 * s, &format!("@{}", v.profile.name), 12.0 * s, t.text_dim, false, name_w);
    *y += 52.0 * s;

    // Add account / Sign out.
    let btn_w = (150.0 * s) as i32;
    let btn_h = (32.0 * s) as i32;
    let b1 = draw_button(ctx, fb, Rect { x: x as i32, y: *y as i32, w: btn_w, h: btn_h }, "Add account", BtnStyle::Primary, false);
    ctx.hits.add(b1, Hit::AddAccount);
    let b2 = draw_button(ctx, fb, Rect { x: x as i32 + btn_w + (10.0 * s) as i32, y: *y as i32, w: btn_w, h: btn_h }, "Sign out", BtnStyle::Secondary, false);
    ctx.hits.add(b2, Hit::Logout);
    *y += 42.0 * s;

    // Other saved accounts (quick switch, Discord-style).
    let others: Vec<&Session> = v.accounts.iter().filter(|a| a.user_id != v.profile.id).collect();
    if !others.is_empty() {
        section_label(ctx, fb, x, y, "SWITCH ACCOUNT", s);
        for acc in others {
            let row_y = *y;
            let hovered = Rect { x: x as i32, y: row_y as i32, w: w as i32, h: (48.0 * s) as i32 }.contains(ctx.cursor.0, ctx.cursor.1);
            if hovered {
                fb.rect_rounded(x as i32, row_y as i32, w as i32, (48.0 * s) as i32, (8.0 * s) as i32, t.surface_2);
            }
            let avatar_sz = (34.0 * s) as i32;
            draw_avatar(ctx, fb, acc.user_id, &acc.display_name, x as i32 + pad, (row_y + 7.0 * s) as i32, avatar_sz);
            let nx = x as f32 + (pad + avatar_sz + (12.0 * s) as i32) as f32;
            let name_w = w - (nx - x as f32) - (110.0 * s);
            font::draw_ellipsis(ctx.fonts, fb, nx, row_y + 20.0 * s, &acc.display_name, 13.0 * s, t.text, true, name_w);
            font::draw_ellipsis(ctx.fonts, fb, nx, row_y + 37.0 * s, &format!("@{}", acc.username), 11.5 * s, t.text_dim, false, name_w);
            let sw = (86.0 * s) as i32;
            let sh = (28.0 * s) as i32;
            let r = Rect {
                x: (x as f32 + w - sw as f32 - 4.0 * s) as i32,
                y: (row_y + (48.0 * s - sh as f32) / 2.0) as i32,
                w: sw,
                h: sh,
            };
            let r = draw_button(ctx, fb, r, "Switch", BtnStyle::Secondary, false);
            ctx.hits.add(r, Hit::SwitchAccount(acc.user_id));
            *y += 56.0 * s;
        }
    }

    // Theme presets.
    section_label(ctx, fb, x, y, "THEME", s);
    let mut chips: Vec<(&str, &str)> = theme::PRESETS.iter().map(|(id, _)| (*id, pretty_theme_name(id))).collect();
    if v.app_settings.custom.is_some() {
        chips.push(("custom", "Custom"));
    }
    let chip_h = (30.0 * s) as i32;
    let mut cx = x as i32;
    for (i, (id, label)) in chips.iter().enumerate() {
        let cw = (font::measure(ctx.fonts, label, 12.5 * s, true) + 24.0 * s) as i32;
        let active = v.app_settings.theme == *id;
        let rect = Rect { x: cx, y: *y as i32, w: cw, h: chip_h };
        if active {
            fb.rect_rounded(rect.x, rect.y, rect.w, rect.h, (8.0 * s) as i32, t.surface_2);
            fb.rect(rect.x + (8.0 * s) as i32, rect.y + rect.h - (3.0 * s).max(1.0) as i32, rect.w - (16.0 * s) as i32, (3.0 * s).max(1.0) as i32, t.accent);
        } else {
            fb.rect_rounded(rect.x, rect.y, rect.w, rect.h, (8.0 * s) as i32, t.input_bg);
        }
        let lw = font::measure(ctx.fonts, label, 12.5 * s, true);
        font::draw(
            ctx.fonts, fb, cx as f32 + (cw as f32 - lw) * 0.5, *y + chip_h as f32 / 2.0 + 12.5 * s * 0.35,
            label, 12.5 * s, if active { t.text } else { t.text_dim }, true, 0.0,
        );
        ctx.hits.add(rect, Hit::ThemePreset(i as u8));
        cx += cw + (8.0 * s) as i32;
    }
    *y += 38.0 * s + 14.0 * s;

    // Home: recommendations toggle.
    section_label(ctx, fb, x, y, "HOME", s);
    let label = "Show recommended places";
    font::draw(ctx.fonts, fb, x as f32, *y + 17.0 * s, label, 13.5 * s, t.text, false, 0.0);
    let tr = draw_toggle(ctx, fb, (x as f32 + w - (52.0 * s)) as i32, *y as i32, v.app_settings.show_recommendations, Hit::ToggleRecommendations);
    let _ = tr;
    *y += 36.0 * s + 10.0 * s;

    // Version.
    let ver = format!("RustBlox v{} \u{b7} unofficial Roblox client", env!("CARGO_PKG_VERSION"));
    font::draw(ctx.fonts, fb, x as f32, *y + 12.0 * s, &ver, 11.0 * s, t.text_faint, false, 0.0);
    *y += 22.0 * s;
}

/// Rough content height of the Appearance tab.
fn measure_appearance(s: f32) -> f32 {
    let slots = color_slot_count();
    let mut y = 6.0 * s;
    y += 24.0 * s; // CORNER ROUNDING
    y += 40.0 * s; // radius row
    y += 14.0 * s + 24.0 * s; // COLORS
    y += slots as f32 * (36.0 * s);
    y += 16.0 * s + 40.0 * s; // reset button
    y + 16.0 * s
}

fn draw_appearance(ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut f32, w: f32, s: f32, app_settings: &mut crate::config::Settings) {
    let t = theme::cur();

    section_label(ctx, fb, x, y, "CORNER ROUNDING", s);
    let scale_pct = (t.radius_scale * 100.0).round();
    let btn = (30.0 * s) as i32;
    let b1 = Rect { x: x as i32, y: *y as i32, w: btn, h: btn };
    let b2 = Rect { x: x as i32 + btn + (10.0 * s) as i32, y: *y as i32, w: btn, h: btn };
    draw_button(ctx, fb, b1, "\u{2212}", BtnStyle::Secondary, false);
    draw_button(ctx, fb, b2, "+", BtnStyle::Secondary, false);
    ctx.hits.add(b1, Hit::RadiusDown);
    ctx.hits.add(b2, Hit::RadiusUp);
    let val = format!("{}%", scale_pct);
    font::draw(ctx.fonts, fb, x as f32 + (2.0 * btn as f32 + 22.0 * s), *y + btn as f32 / 2.0 + 12.0 * s * 0.35, &val, 13.5 * s, t.text, true, 0.0);
    *y += 40.0 * s + 14.0 * s;

    section_label(ctx, fb, x, y, "COLORS", s);
    for i in 0..color_slot_count() {
        let cur = theme_field(app_settings.custom.unwrap_or(t), i);
        let label = color_slot_label(i);
        let sw = (26.0 * s) as i32;
        let sh = (22.0 * s) as i32;
        let row_cy = *y + 17.0 * s;
        font::draw(ctx.fonts, fb, x as f32, row_cy + 4.0 * s, label, 12.5 * s, t.text_dim, false, 0.0);
        let chip_x = (x as f32 + w - (58.0 * s)) as i32;
        // Prev / next arrows around the swatch.
        let aw = (18.0 * s) as i32;
        let prev = Rect { x: chip_x - aw - (2.0 * s) as i32, y: (*y + (sh as f32 - aw as f32) / 2.0) as i32, w: aw, h: aw };
        let next = Rect { x: chip_x + sw + (2.0 * s) as i32, y: prev.y, w: aw, h: aw };
        draw_small_arrow(ctx, fb, prev, false);
        draw_small_arrow(ctx, fb, next, true);
        ctx.hits.add(prev, Hit::ColorPrev(i));
        ctx.hits.add(next, Hit::ColorNext(i));
        fb.rect_rounded(chip_x, *y as i32, sw, sh, (5.0 * s) as i32, cur);
        *y += 36.0 * s;
    }
    *y += 6.0 * s;

    let bw = (170.0 * s) as i32;
    let bh = (32.0 * s) as i32;
    let r = draw_button(ctx, fb, Rect { x: x as i32, y: *y as i32, w: bw, h: bh }, "Reset appearance", BtnStyle::Secondary, false);
    ctx.hits.add(r, Hit::ResetAppearance);
    *y += 40.0 * s;
}

fn draw_small_arrow(ctx: &mut UiCtx, fb: &mut FB, r: Rect, right: bool) {
    let t = theme::cur();
    let hov = r.contains(ctx.cursor.0, ctx.cursor.1);
    let color = if hov { t.text } else { t.text_faint };
    if right {
        tri_right(fb, r.x + r.w / 2, r.y + r.h / 2, color);
    } else {
        tri_left(fb, r.x + r.w / 2, r.y + r.h / 2, color);
    }
}

/// Pill-style on/off toggle (Settings → recommendations).
fn draw_toggle(ctx: &mut UiCtx, fb: &mut FB, x: i32, y: i32, on: bool, hit: Hit) -> Rect {
    let s = ctx.scale;
    let t = theme::cur();
    let w = (44.0 * s) as i32;
    let h = (24.0 * s) as i32;
    fb.rect_rounded(x, y, w, h, h / 2, if on { t.accent } else { t.border });
    let k = h - (6.0 * s) as i32;
    let kx = if on { x + w - k - (3.0 * s) as i32 } else { x + (3.0 * s) as i32 };
    fb.circle(kx + k / 2, y + h / 2, k / 2, t.white);
    let r = Rect { x, y, w, h };
    ctx.hits.add(r, hit);
    r
}

fn section_label(ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut f32, text: &str, s: f32) {
    let t = theme::cur();
    font::draw(ctx.fonts, fb, x as f32, *y + 12.0 * s, text, 11.0 * s, t.text_faint, true, 1.0 * s);
    *y += 24.0 * s;
}

fn pretty_theme_name(id: &str) -> &'static str {
    match id {
        "dark" => "Dark",
        "light" => "Light",
        "gruvbox" => "Gruvbox",
        "nord" => "Nord",
        "custom" => "Custom",
        _ => "Dark",
    }
}

// -- Appearance color slots --------------------------------------------------

const COLOR_LABELS: [&str; 17] = [
    "Background",
    "Surface",
    "Surface 2",
    "Input background",
    "Borders",
    "Text",
    "Text dim",
    "Text faint",
    "Accent",
    "Accent hover",
    "Disabled button",
    "Button text",
    "Green (online / Join)",
    "Blue (in game)",
    "Orange (Studio)",
    "Gray (offline)",
    "Red (errors / logo)",
];

fn color_slot_count() -> usize {
    COLOR_LABELS.len()
}

fn color_slot_label(i: usize) -> &'static str {
    COLOR_LABELS[i.min(COLOR_LABELS.len() - 1)]
}

/// Mutable access to color slot `i` of a theme.
fn theme_field_mut(t: &mut Theme, i: usize) -> &mut theme::Rgba {
    match i {
        0 => &mut t.bg,
        1 => &mut t.surface,
        2 => &mut t.surface_2,
        3 => &mut t.input_bg,
        4 => &mut t.border,
        5 => &mut t.text,
        6 => &mut t.text_dim,
        7 => &mut t.text_faint,
        8 => &mut t.accent,
        9 => &mut t.accent_hover,
        10 => &mut t.btn_disabled,
        11 => &mut t.btn_text,
        12 => &mut t.green,
        13 => &mut t.blue,
        14 => &mut t.orange,
        15 => &mut t.gray,
        _ => &mut t.red,
    }
}

/// Read a color slot (public: the app handler cycles slots).
pub fn theme_field(t: Theme, i: usize) -> theme::Rgba {
    match i {
        0 => t.bg,
        1 => t.surface,
        2 => t.surface_2,
        3 => t.input_bg,
        4 => t.border,
        5 => t.text,
        6 => t.text_dim,
        7 => t.text_faint,
        8 => t.accent,
        9 => t.accent_hover,
        10 => t.btn_disabled,
        11 => t.btn_text,
        12 => t.green,
        13 => t.blue,
        14 => t.orange,
        15 => t.gray,
        _ => t.red,
    }
}

/// Write a color slot (public: the app handler cycles slots).
pub fn set_theme_slot(mut t: Theme, i: usize, color: theme::Rgba) -> Theme {
    *theme_field_mut(&mut t, i) = color;
    t
}

/// Palette the color swatches cycle through (Settings → Appearance).
pub const COLOR_PALETTE: [theme::Rgba; 16] = [
    [0xFF, 0xFF, 0xFF, 0xFF],
    [0xC6, 0xCC, 0xD2, 0xFF],
    [0x8B, 0x91, 0x99, 0xFF],
    [0x5A, 0x60, 0x67, 0xFF],
    [0x2A, 0x2D, 0x2F, 0xFF],
    [0x17, 0x19, 0x1B, 0xFF],
    [0x0F, 0x11, 0x12, 0xFF],
    [0xE2, 0x23, 0x1A, 0xFF],
    [0xF2, 0x8A, 0x2E, 0xFF],
    [0xFA, 0xBD, 0x2F, 0xFF],
    [0x00, 0xB0, 0x6F, 0xFF],
    [0x00, 0xA2, 0xFF, 0xFF],
    [0x33, 0x5F, 0xFF, 0xFF],
    [0x8B, 0x5C, 0xF6, 0xFF],
    [0xE2, 0x56, 0x4E, 0xFF],
    [0x8C, 0x5A, 0x3C, 0xFF],
];

/// Next/prev palette entry for a color (unknown colors start at index 0).
pub fn cycle_color(cur: theme::Rgba, dir: i32) -> theme::Rgba {
    let idx = COLOR_PALETTE.iter().position(|c| *c == cur);
    let n = COLOR_PALETTE.len() as i32;
    match idx {
        Some(i) => {
            let next = (i as i32 + dir).rem_euclid(n);
            COLOR_PALETTE[next as usize]
        }
        None => {
            if dir < 0 {
                COLOR_PALETTE[n as usize - 1]
            } else {
                COLOR_PALETTE[0]
            }
        }
    }
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
    let t = theme::cur();
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
    fb.rect_rounded(x + w / 2 - 3, thumb_y as i32, 6, thumb_h, (3.0 * s) as i32, t.border);

    // Arrows.
    let cx = x + w / 2;
    tri_up(fb, cx, y + aw / 2, t.text_dim);
    tri_down(fb, cx, y + h - aw / 2, t.text_dim);
}

// ---------------------------------------------------------------------------
// Shared: status bar, avatar
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
        ctx.fonts, fb, (x + pad) as f32, y as f32 + h / 2.0 + 12.0 * s * 0.35,
        &left, 12.0 * s, notice_color, false, notice_max_w.max(80.0),
    );
    if v.refreshing && !notice_active {
        ui::draw_spinner(
            fb, (x + pad) as f32 + font::measure(ctx.fonts, &left, 12.0 * s, false) + 14.0 * s,
            y as f32 + h / 2.0 - 1.0 * s, 6.0 * s, ctx.now_ms, t.accent, s,
        );
    }
    if let Some(err) = &v.error {
        let ew = (w as f32 - font::measure(ctx.fonts, &left, 12.0 * s, false) - 60.0 * s).max(0.0);
        font::draw_ellipsis(
            ctx.fonts, fb, (x + w - pad) as f32 - ew.min(w as f32 - 220.0 * s), y as f32 + h / 2.0 + 12.0 * s * 0.35,
            &format!("Error: {}", err), 12.0 * s, t.red, false, w as f32 - 220.0 * s,
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

fn draw_avatar(ctx: &mut UiCtx, fb: &mut FB, id: u64, display_name: &str, x: i32, y: i32, size: i32) {
    let t = theme::cur();
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
                &initial, size as f32 * 0.42, t.white, true, 0.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_scroll, cycle_color, COLOR_PALETTE, format_count};

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

    #[test]
    fn color_cycling() {
        let a = COLOR_PALETTE[0];
        let b = COLOR_PALETTE[1];
        assert_eq!(cycle_color(a, 1), b);
        assert_eq!(cycle_color(b, -1), a);
        // Wraps both directions.
        let last = COLOR_PALETTE[COLOR_PALETTE.len() - 1];
        assert_eq!(cycle_color(last, 1), COLOR_PALETTE[0]);
        assert_eq!(cycle_color(COLOR_PALETTE[0], -1), last);
        // Unknown color enters the palette at the front (next) / back (prev).
        let unknown = [1u8, 2, 3, 4];
        assert_eq!(cycle_color(unknown, 1), COLOR_PALETTE[0]);
        assert_eq!(cycle_color(unknown, -1), last);
    }
}
