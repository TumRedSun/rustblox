//! Login screen: password+2FA flow and session-cookie flow.

use crate::app::{StatusKind, UiCtx};
use crate::draw::FB;
use crate::font;
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, FieldId, Hit, Rect, TextField};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginTab {
    Password,
    Cookie,
}

pub struct TwoStepPending {
    pub login: String,
    pub ticket: String,
    pub media_type: String,
}

pub struct LoginView {
    pub tab: LoginTab,
    pub username: TextField,
    pub password: TextField,
    pub cookie: TextField,
    pub code: TextField,
    pub two_step: Option<TwoStepPending>,
    pub status: Option<(StatusKind, String)>,
    pub busy: bool,
    /// Remembered login name while the 2FA challenge is pending.
    pub pending_login: String,
}

impl LoginView {
    pub fn new() -> LoginView {
        let mut code = TextField::new("6-digit code").numeric();
        code.max_bytes = 12; // room for separators; server rejects wrong codes
        LoginView {
            tab: LoginTab::Password,
            username: TextField::new("Username or email"),
            password: TextField::new("Password").masked(),
            cookie: TextField::new(".ROBLOSECURITY token").masked(),
            code,
            two_step: None,
            status: None,
            busy: false,
            pending_login: String::new(),
        }
    }

    pub fn with_status(status: Option<(StatusKind, String)>) -> LoginView {
        let mut v = LoginView::new();
        v.status = status;
        v
    }
}

/// Draw the whole login screen. `adding` = opened from Settings → Add
/// account: shows a hint and a Back button that returns to the running app
/// without touching the current session.
pub fn draw(v: &mut LoginView, ctx: &mut UiCtx, fb: &mut FB, restoring: bool, adding: bool) {
    let t = theme::cur();
    let w = fb.w;
    let h = fb.h;

    // Adaptive scale: every login widget shares one scale that always fits
    // the current window. On narrow phone screens the card shrinks
    // proportionally instead of being clipped at both edges; desktop is
    // unaffected (its scale already fits).
    let real_scale = ctx.scale;
    ctx.scale = ctx
        .scale
        .min(w as f32 / 500.0)
        .min(h as f32 / 560.0)
        .max(1.0);
    let s = ctx.scale;

    // Restoring banner: full-screen dim + centered card.
    if restoring {
        fb.fill(t.bg);
        let card_w = (420.0 * s) as i32;
        let card_h = (140.0 * s) as i32;
        let cx = (w - card_w) / 2;
        let cy = (h - card_h) / 2;
        fb.rect_rounded(cx, cy, card_w, card_h, (12.0 * s) as i32, t.surface);
        ui::draw_spinner(fb, w as f32 / 2.0 - 40.0 * s, cy as f32 + card_h as f32 / 2.0, 9.0 * s, ctx.now_ms, t.accent, s);
        font::draw(
            ctx.fonts, fb, w as f32 / 2.0 - 20.0 * s, cy as f32 + card_h as f32 / 2.0 + 5.0 * s,
            "Restoring saved session\u{2026}", 14.0 * s, t.text_dim, false, 0.0,
        );
        ctx.scale = real_scale;
        return;
    }

    // Card geometry.
    let card_w = (460.0 * s) as i32;
    let content_h = (380.0 * s) as i32;
    let card_x = (w - card_w) / 2;
    let card_y = ((h - content_h) / 2).max((40.0 * s) as i32);

    draw_logo(ctx, fb, w, (card_y - (96.0 * s) as i32).max((34.0 * s) as i32));

    if adding {
        // Back button between the logo and the card.
        let bw = (220.0 * s) as i32;
        let bh = (32.0 * s) as i32;
        let back = Rect { x: (w - bw) / 2, y: card_y - (58.0 * s) as i32, w: bw, h: bh };
        let r = draw_button(ctx, fb, back, "\u{2190} Back to RustBlox", BtnStyle::Ghost, false);
        ctx.hits.add(r, Hit::CancelAdd);
        font::draw(
            ctx.fonts, fb, w as f32 / 2.0 - font::measure(ctx.fonts, "Add another account", 12.5 * s, false) / 2.0,
            card_y as f32 - 14.0 * s, "Add another account", 12.5 * s, t.text_dim, false, 0.0,
        );
    }

    fb.rect_rounded(card_x, card_y, card_w, content_h, (12.0 * s) as i32, t.surface);

    let pad = (28.0 * s) as i32;
    let inner_w = card_w - 2 * pad;
    let mut y = card_y + pad;

    if v.two_step.is_some() {
        draw_two_step(v, ctx, fb, card_x + pad, &mut y, inner_w, card_y, content_h);
    } else {
        // Tabs.
        let tab_h = (38.0 * s) as i32;
        let gap = (8.0 * s) as i32;
        let tw = (inner_w - gap) / 2;
        draw_tab(ctx, fb, Rect { x: card_x + pad, y, w: tw, h: tab_h }, "Password login", v.tab == LoginTab::Password, Hit::TabPassword);
        draw_tab(ctx, fb, Rect { x: card_x + pad + tw + gap, y, w: tw, h: tab_h }, "Session cookie", v.tab == LoginTab::Cookie, Hit::TabCookie);
        y += tab_h + (24.0 * s) as i32;

        let field_h = (42.0 * s) as i32;
        let field_gap = (6.0 * s) as i32; // extra room for the floating label
        match v.tab {
            LoginTab::Password => {
                v.username.draw(ctx, fb, card_x + pad, y, inner_w, field_h, FieldId::Username, Some("Username or email"));
                y += field_h + field_gap + (10.0 * s) as i32;
                v.password.draw(ctx, fb, card_x + pad, y, inner_w, field_h, FieldId::Password, Some("Password"));
                y += field_h + field_gap + (18.0 * s) as i32;
                let btn = draw_button(
                    ctx, fb,
                    Rect { x: card_x + pad, y, w: inner_w, h: field_h },
                    if v.busy { "Signing in\u{2026}" } else { "Sign in" },
                    BtnStyle::Primary,
                    v.busy,
                );
                ctx.hits.add(btn, Hit::SignIn);
                if v.busy {
                    ui::draw_spinner(fb, btn.x as f32 + 18.0 * s, btn.y as f32 + btn.h as f32 / 2.0, 7.0 * s, ctx.now_ms, t.white, s);
                }
                y += field_h + (14.0 * s) as i32;
                font::draw_ellipsis(
                    ctx.fonts, fb, card_x as f32 + pad as f32, y as f32 + 11.0 * s,
                    "Having trouble? Use the Session cookie tab \u{2014} it never asks for a captcha.",
                    12.0 * s, t.text_faint, false, inner_w as f32,
                );
            }
            LoginTab::Cookie => {
                v.cookie.draw(ctx, fb, card_x + pad, y, inner_w, field_h, FieldId::Cookie, Some(".ROBLOSECURITY token"));
                y += field_h + field_gap + (18.0 * s) as i32;
                let btn = draw_button(
                    ctx, fb,
                    Rect { x: card_x + pad, y, w: inner_w, h: field_h },
                    if v.busy { "Signing in\u{2026}" } else { "Sign in with cookie" },
                    BtnStyle::Primary,
                    v.busy,
                );
                ctx.hits.add(btn, Hit::SignIn);
                if v.busy {
                    ui::draw_spinner(fb, btn.x as f32 + 18.0 * s, btn.y as f32 + btn.h as f32 / 2.0, 7.0 * s, ctx.now_ms, t.white, s);
                }
                y += field_h + (14.0 * s) as i32;
                let hint = "Website \u{2192} F12 \u{2192} Application \u{2192} Cookies \u{2192} https://www.roblox.com \u{2192} .ROBLOSECURITY";
                font::draw_ellipsis(
                    ctx.fonts, fb, card_x as f32 + pad as f32, y as f32 + 11.0 * s,
                    hint, 12.0 * s, t.text_faint, false, inner_w as f32,
                );
            }
        }
    }

    // Status line pinned near the card bottom.
    if let Some((kind, msg)) = &v.status {
        let color = match kind {
            StatusKind::Error => t.red,
            StatusKind::Info => t.text_dim,
            StatusKind::Success => t.green,
        };
        let sy = card_y + content_h - (34.0 * s) as i32;
        font::draw_ellipsis(
            ctx.fonts, fb, card_x as f32 + pad as f32, sy as f32,
            msg, 13.0 * s, color, false, inner_w as f32,
        );
    }

    draw_footer(ctx, fb, w, h);
    ctx.scale = real_scale;
}

fn draw_two_step(v: &mut LoginView, ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut i32, inner_w: i32, card_y: i32, content_h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    font::draw(ctx.fonts, fb, x as f32, *y as f32 + 14.0 * s, "2-Step Verification", 17.0 * s, t.text, true, 0.0);
    *y += (26.0 * s) as i32;
    let media = v.two_step.as_ref().map(|t| t.media_type.clone()).unwrap_or_default();
    let via = if media.eq_ignore_ascii_case("Email") { "your email" } else { "your authenticator app" };
    font::draw_ellipsis(
        ctx.fonts, fb, x as f32, *y as f32 + 12.0 * s,
        &format!("Enter the code from {}.", via), 13.0 * s, t.text_dim, false, inner_w as f32,
    );
    // Extra room so the field label below never collides with the subtitle.
    *y += (34.0 * s) as i32;

    let field_h = (42.0 * s) as i32;
    v.code.draw(ctx, fb, x, *y, inner_w, field_h, FieldId::Code, Some("Verification code"));
    *y += field_h + (18.0 * s) as i32;

    let gap = (10.0 * s) as i32;
    let bw = (inner_w - gap) / 2;
    let verify = draw_button(ctx, fb, Rect { x, y: *y, w: bw, h: field_h }, if v.busy { "Verifying\u{2026}" } else { "Verify" }, BtnStyle::Primary, v.busy);
    ctx.hits.add(verify, Hit::Verify);
    let resend = draw_button(ctx, fb, Rect { x: x + bw + gap, y: *y, w: bw, h: field_h }, "Resend code", BtnStyle::Secondary, v.busy);
    ctx.hits.add(resend, Hit::Resend);
    *y += field_h + gap;
    let back = draw_button(ctx, fb, Rect { x, y: *y, w: inner_w, h: field_h }, "Back to login", BtnStyle::Ghost, false);
    ctx.hits.add(back, Hit::Back);

    // Keep the status area consistent with the card bottom.
    let _ = (card_y, content_h);
}

fn draw_logo(ctx: &mut UiCtx, fb: &mut FB, w: i32, top: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    let sq = (26.0 * s) as i32;
    let cx = w / 2 - (110.0 * s) as i32 / 2;
    fb.rect_rounded(cx, top, sq, sq, (6.0 * s) as i32, t.red);
    let r_w = font::measure(ctx.fonts, "R", 16.0 * s, true);
    font::draw(
        ctx.fonts, fb, cx as f32 + (sq as f32 - r_w) * 0.5, top as f32 + sq as f32 * 0.5 + 16.0 * s * 0.35,
        "R", 16.0 * s, t.white, true, 0.0,
    );
    font::draw(
        ctx.fonts, fb, (cx + sq + (10.0 * s) as i32) as f32, top as f32 + sq as f32 * 0.5 + 16.0 * s * 0.35,
        "RUSTBLOX", 16.0 * s, t.text, true, 1.2 * s,
    );
    let sub = "Unofficial open-source Roblox client";
    let sub_w = font::measure(ctx.fonts, sub, 12.5 * s, false);
    font::draw(
        ctx.fonts, fb, w as f32 / 2.0 - sub_w / 2.0, top as f32 + sq as f32 + 20.0 * s,
        sub, 12.5 * s, t.text_faint, false, 0.0,
    );
}

fn draw_tab(ctx: &mut UiCtx, fb: &mut FB, r: Rect, label: &str, active: bool, hit: Hit) {
    let s = ctx.scale;
    let t = theme::cur();
    // Frameless tabs: active = lighter fill + accent underline, inactive =
    // plain dim text on the card background.
    if active {
        fb.rect_rounded(r.x, r.y, r.w, r.h, (8.0 * s) as i32, t.surface_2);
        fb.rect(r.x + (10.0 * s) as i32, r.y + r.h - (3.0 * s).max(1.0) as i32, r.w - (20.0 * s) as i32, (3.0 * s).max(1.0) as i32, t.accent);
    }
    let tw = font::measure(ctx.fonts, label, 13.5 * s, true);
    font::draw(
        ctx.fonts, fb, r.x as f32 + (r.w as f32 - tw) * 0.5, r.y as f32 + r.h as f32 * 0.5 + 13.5 * s * 0.35,
        label, 13.5 * s, if active { t.text } else { t.text_dim }, true, 0.0,
    );
    ctx.hits.add(r, hit);
}

fn draw_footer(ctx: &mut UiCtx, fb: &mut FB, w: i32, h: i32) {
    let s = ctx.scale;
    let t = theme::cur();
    let text = "GPL-3.0 open-source \u{b7} not affiliated with Roblox Corporation";
    let tw = font::measure(ctx.fonts, text, 11.5 * s, false);
    font::draw(ctx.fonts, fb, w as f32 / 2.0 - tw / 2.0, h as f32 - 14.0 * s, text, 11.5 * s, t.text_faint, false, 0.0);
}
