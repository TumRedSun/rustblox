//! Headless render of every screen/state into PNGs (debug/diagnostic tool).
//!
//! Usage: cargo run --example render_preview -- [output_dir]

use rustblox::app::UiCtx;
use rustblox::draw::FB;
use rustblox::font::Fonts;
use rustblox::login_ui::{LoginTab, LoginView, TwoStepPending};
use rustblox::model::{Friend, FriendRow, PlaceCard, Presence, PresenceKind, UserProfile};
use rustblox::theme;
use rustblox::ui::{FieldId, HitMap};
use std::sync::Arc;

struct Painter<'a> {
    fonts: &'a Fonts,
    hits: HitMap,
    avatars: std::collections::HashMap<u64, Arc<Vec<u8>>>,
    icons: std::collections::HashMap<u64, Arc<Vec<u8>>>,
}

impl<'a> Painter<'a> {
    fn new(fonts: &'a Fonts) -> Self {
        Painter { fonts, hits: HitMap::default(), avatars: Default::default(), icons: Default::default() }
    }

    fn ctx<'b>(&'b mut self, focus: Option<FieldId>) -> UiCtx<'b> {
        UiCtx {
            fonts: self.fonts,
            cursor: (-1.0, -1.0),
            hits: &mut self.hits,
            focus,
            scale: 1.0,
            now_ms: 0,
            caret_anim: false,
            avatars: &mut self.avatars,
            icons: &mut self.icons,
        }
    }
}

fn save(buf: &[u8], w: i32, h: i32, path: &str) {
    image::RgbaImage::from_raw(w as u32, h as u32, buf.to_vec())
        .expect("buffer")
        .save(path)
        .expect("save png");
    println!("saved {path}");
}

fn blank_canvas(w: i32, h: i32) -> Vec<u8> {
    vec![0u8; (w * h * 4) as usize]
}

fn fake_avatar(id: u64) -> (u64, Arc<Vec<u8>>) {
    // Deterministic gradient placeholder bitmap (48x48 RGBA).
    let mut v = Vec::with_capacity(48 * 48 * 4);
    let r = (id as u8).wrapping_mul(37);
    let g = (id as u8).wrapping_mul(91);
    for y in 0..48u8 {
        for x in 0..48u8 {
            v.push(r.saturating_add(x / 3));
            v.push(g.saturating_add(y / 3));
            v.push((id as u8).wrapping_mul(13).saturating_add(x / 4));
            v.push(255);
        }
    }
    (id, Arc::new(v))
}

fn fake_icon(place_id: u64) -> (u64, Arc<Vec<u8>>) {
    let mut v = Vec::with_capacity(theme::PLACE_ICON_PX as usize * theme::PLACE_ICON_PX as usize * 4);
    let base = (place_id as u8).wrapping_mul(53);
    for y in 0..theme::PLACE_ICON_PX as u8 {
        for x in 0..theme::PLACE_ICON_PX as u8 {
            v.push(base.saturating_add(x / 2));
            v.push(base.wrapping_mul(3).saturating_add(y / 2));
            v.push(base.wrapping_mul(7));
            v.push(255);
        }
    }
    (place_id, Arc::new(v))
}

fn sample_rows() -> Vec<FriendRow> {
    let mk = |id: u64, name: &str, dn: &str, kind: PresenceKind, game: Option<&str>, last: &str, lo: Option<i64>| FriendRow {
        friend: Friend { id, name: name.into(), display_name: dn.into(), is_banned: false },
        presence: Presence { kind, last_location: last.into(), ..Default::default() },
        game_name: game.map(|g| g.into()),
        last_online: lo,
    };
    vec![
        mk(1, "builderman", "Builderman", PresenceKind::InGame, Some("Natural Disaster Survival"), "Website", None),
        // Universe hidden by the games API -> lastLocation fallback.
        mk(2, "erik.cassel", "Erik", PresenceKind::InGame, None, "Playing: Blox Fruits", None),
        mk(3, "shedletsky", "Shedletsky", PresenceKind::Studio, None, "Creating RustBlox HQ", None),
        mk(4, "noob2007", "Noob", PresenceKind::Offline, None, "Website", Some(1_700_000_000)),
        mk(5, "guest1234", "Guest", PresenceKind::Invisible, None, "", None),
        mk(6, "telamon", "Telamon", PresenceKind::InGame, Some("Doomspire Brickbattle"), "Website", None),
        mk(7, "stickmasterluke", "Stickmasterluke", PresenceKind::Online, None, "Website", None),
        mk(8, "Asimo3089", "asimo3089", PresenceKind::Online, None, "Website", None),
        mk(9, "badcc", "badcc", PresenceKind::Offline, None, "Website", Some(1_702_000_000)),
        mk(10, "beeism", "Beeism", PresenceKind::InGame, Some("Adopt Me!"), "Website", None),
    ]
}

fn sample_places() -> Vec<PlaceCard> {
    let mk = |u: u64, p: u64, name: &str, creator: &str, players: u64, likes: Option<u8>| PlaceCard {
        universe_id: u,
        root_place_id: p,
        name: name.into(),
        creator: creator.into(),
        player_count: players,
        likes_pct: likes,
        sponsored: false,
    };
    vec![
        mk(920587237, 2574207693, "morning", "Fandom_Collecter", 1_626, Some(82)),
        mk(5688684783, 16518256559, "Mega Obby Fun", "ObbyMasters", 26_400, Some(87)),
        mk(142823291, 606849621, " Jailbreak", "asimo3089", 98_765, Some(93)),
        mk(1224212278, 2753915549, "Blox Fruits", "Gamer Robot Inc", 412_003, Some(89)),
        mk(3, 3, "[CONTENT DELETED]", "", 0, None),
        mk(185655149, 189707, "Natural Disaster Survival", "Stickmasterluke", 9_412, Some(95)),
    ]
}

fn blank_menu_view() -> rustblox::menu_ui::MenuView {
    let mut view = rustblox::menu_ui::MenuView::new(UserProfile {
        id: 42,
        name: "TumRedSun".into(),
        display_name: "TumRedSun".into(),
    });
    view.rows = sample_rows();
    view.places = sample_places();
    view.refreshing = false;
    view.places_searching = false;
    view.last_update = Some(std::time::Instant::now());
    view.next_refresh = Some(std::time::Instant::now() + std::time::Duration::from_secs(7));
    view
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp".into());
    let w: i32 = 1100;
    let h: i32 = 740;
    let fonts = Fonts::load();

    // 1. Login, password tab, username focused with typed text.
    {
        let mut p = Painter::new(&fonts);
        let mut view = LoginView::new();
        view.username.value = "TumRedSun".into();
        view.username.cursor = view.username.value.len();
        view.password.value = "hunter2secret".into();
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(Some(FieldId::Username));
        rustblox::login_ui::draw(&mut view, &mut ctx, &mut fb, false);
        save(&buf, w, h, &format!("{dir}/preview_login_filled.png"));
    }

    // 2. Login, cookie tab.
    {
        let mut p = Painter::new(&fonts);
        let mut view = LoginView::new();
        view.tab = LoginTab::Cookie;
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(Some(FieldId::Cookie));
        rustblox::login_ui::draw(&mut view, &mut ctx, &mut fb, false);
        save(&buf, w, h, &format!("{dir}/preview_login_cookie.png"));
    }

    // 3. Login, 2FA challenge.
    {
        let mut p = Painter::new(&fonts);
        let mut view = LoginView::new();
        view.pending_login = "TumRedSun".into();
        view.two_step = Some(TwoStepPending {
            login: "TumRedSun".into(),
            ticket: "A1B2C3".into(),
            media_type: "Email".into(),
        });
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(Some(FieldId::Code));
        rustblox::login_ui::draw(&mut view, &mut ctx, &mut fb, false);
        save(&buf, w, h, &format!("{dir}/preview_login_2fa.png"));
    }

    // 4. Menu: Home tab — places search, horizontal friends strip with
    //    arrows and the place-card grid.
    {
        let mut p = Painter::new(&fonts);
        let mut view = blank_menu_view();
        for (id, a) in [fake_avatar(42u64), fake_avatar(1), fake_avatar(2), fake_avatar(3), fake_avatar(6)] {
            p.avatars.insert(id, a);
        }
        for (id, ic) in [fake_icon(2574207693u64), fake_icon(16518256559), fake_icon(606849621), fake_icon(2753915549)] {
            p.icons.insert(id, ic);
        }
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(Some(FieldId::Search));
        rustblox::menu_ui::draw(&mut view, &mut ctx, &mut fb);
        save(&buf, w, h, &format!("{dir}/preview_menu.png"));
    }

    // 5. Menu: Home with the sidebar collapsed to the icon-only mini mode.
    {
        let mut p = Painter::new(&fonts);
        let mut view = blank_menu_view();
        view.collapsed = true;
        view.search.value = "obby".into();
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(None);
        rustblox::menu_ui::draw(&mut view, &mut ctx, &mut fb);
        save(&buf, w, h, &format!("{dir}/preview_menu_mini.png"));
    }

    // 6. Menu: Add Friends pane with search results.
    {
        let mut p = Painter::new(&fonts);
        let mut view = blank_menu_view();
        view.tab = rustblox::menu_ui::MenuTab::AddFriends;
        view.add.query.value = "builder".into();
        view.add.searched_once = true;
        view.add.results = vec![
            rustblox::model::UserEntry { id: 1, name: "builderman".into(), display_name: "Builderman".into() },
            rustblox::model::UserEntry { id: 156, name: "builderguy".into(), display_name: "Builderguy".into() },
            rustblox::model::UserEntry { id: 900, name: "blockbuilder99".into(), display_name: "BlockBuilder".into() },
        ];
        view.add.sent.push(156);
        view.add.failed.insert(900, "Friend request already pending".into());
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(Some(FieldId::AddSearch));
        rustblox::menu_ui::draw(&mut view, &mut ctx, &mut fb);
        save(&buf, w, h, &format!("{dir}/preview_menu_add.png"));
    }

    // 7. Menu: Home, places grid scrolled (scrollbar arrows + thumb state).
    {
        let mut p = Painter::new(&fonts);
        let mut view = blank_menu_view();
        view.places_scroll = 190.0;
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(None);
        rustblox::menu_ui::draw(&mut view, &mut ctx, &mut fb);
        save(&buf, w, h, &format!("{dir}/preview_menu_scrolled.png"));
    }
}
