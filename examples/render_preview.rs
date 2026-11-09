//! Headless render of every screen/state into PNGs (debug/diagnostic tool).
//!
//! Usage: cargo run --example render_preview -- [output_dir]

use rustblox::app::UiCtx;
use rustblox::draw::FB;
use rustblox::font::Fonts;
use rustblox::login_ui::{LoginTab, LoginView, TwoStepPending};
use rustblox::model::{Friend, FriendRow, Presence, PresenceKind, UserProfile};
use rustblox::theme;
use rustblox::ui::{FieldId, HitMap};

struct Painter<'a> {
    fonts: &'a Fonts,
    hits: HitMap,
    avatars: std::collections::HashMap<u64, std::sync::Arc<Vec<u8>>>,
}

impl<'a> Painter<'a> {
    fn new(fonts: &'a Fonts) -> Self {
        Painter { fonts, hits: HitMap::default(), avatars: Default::default() }
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

fn sample_rows() -> Vec<FriendRow> {
    let mk = |id: u64, name: &str, dn: &str, kind: PresenceKind, game: Option<&str>, last: Option<i64>| FriendRow {
        friend: Friend { id, name: name.into(), display_name: dn.into(), is_banned: false },
        presence: Presence { kind, ..Default::default() },
        game_name: game.map(|g| g.into()),
        last_online: last,
    };
    vec![
        mk(1, "builderman", "Builderman", PresenceKind::InGame, Some("Natural Disaster Survival"), None),
        mk(2, "erik.cassel", "Erik", PresenceKind::Online, None, None),
        mk(3, "shedletsky", "Shedletsky", PresenceKind::Studio, None, None),
        mk(4, "noob2007", "Noob", PresenceKind::Offline, None, Some(1_700_000_000)),
        mk(5, "guest1234", "Guest", PresenceKind::Invisible, None, None),
    ]
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

    // 4. Menu with sample friends.
    {
        let mut p = Painter::new(&fonts);
        let mut view = rustblox::menu_ui::MenuView::new(UserProfile {
            id: 42,
            name: "TumRedSun".into(),
            display_name: "TumRedSun".into(),
        });
        view.rows = sample_rows();
        view.refreshing = false;
        view.last_update = Some(std::time::Instant::now());
        view.next_refresh = Some(std::time::Instant::now() + std::time::Duration::from_secs(12));
        let mut buf = blank_canvas(w, h);
        let mut fb = FB::new(&mut buf, w, h);
        fb.fill(theme::BG);
        let mut ctx = p.ctx(None);
        rustblox::menu_ui::draw(&mut view, &mut ctx, &mut fb);
        save(&buf, w, h, &format!("{dir}/preview_menu.png"));
    }
}
