//! Application core: window/event-loop glue (winit 0.30 `ApplicationHandler`),
//! screen state machine, input routing and worker event handling.

use crate::config::{self, Session};
use crate::draw::FB;
use crate::font::Fonts;
use crate::login_ui::LoginView;
use crate::menu_ui::{MenuTab, MenuView};
use crate::model::{build_rows, UserProfile};
use crate::net::{Command, ImageKind, UserEvent, WorkerEvent};
use crate::roblox::auth::AuthOutcome;
use crate::roblox::errors::ApiError;
use crate::theme;
use crate::ui::{FieldId, Hit, HitMap};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowLevel};

pub const WINDOW_W: f32 = 1100.0;
pub const WINDOW_H: f32 = 740.0;
/// Presence auto-refresh interval (no manual refresh button — automatic only).
pub const REFRESH_SECS: u64 = 10;
pub const BACKOFF_SECS: u64 = 30;
/// Delay before the first auto-repeat tick of a held scroll arrow.
pub const ARROW_REPEAT_DELAY_MS: u64 = 250;
/// Auto-repeat interval of a held scroll arrow.
pub const ARROW_REPEAT_MS: u64 = 110;

pub enum Screen {
    Login(LoginView),
    Menu(MenuView),
}

#[derive(Clone, Copy, PartialEq)]
pub enum StatusKind {
    Info,
    Error,
    Success,
}

/// Draw/input context handed to screen modules each frame.
pub struct UiCtx<'a> {
    pub fonts: &'a Fonts,
    pub cursor: (f32, f32),
    pub hits: &'a mut HitMap,
    pub focus: Option<FieldId>,
    pub scale: f32,
    pub now_ms: u64,
    /// Set by a text field when a caret was visible (schedules blink redraw).
    pub caret_anim: bool,
    pub avatars: &'a mut HashMap<u64, Arc<Vec<u8>>>,
    /// Decoded square game icons keyed by root place id.
    pub icons: &'a mut HashMap<u64, Arc<Vec<u8>>>,
}

pub struct App {
    pub window: Option<Arc<Window>>,
    pub pixels: Option<pixels::Pixels<'static>>,
    pub fonts: Fonts,
    pub hits: HitMap,
    pub cursor: (f32, f32),
    pub mouse_down: Option<Hit>,
    pub drag_scroll: Option<(f32, f32)>, // (start cursor y, start scroll)
    /// Held scroll arrow (Hit::ScrollUp/ScrollDown) with the next repeat tick.
    pub arrow_hold: Option<(Hit, Instant)>,
    pub mods: ModifiersState,
    pub focus: Option<FieldId>,
    pub screen: Screen,
    pub tx: Sender<Command>,
    pub fbw: i32,
    pub fbh: i32,
    pub scale: f32,
    pub needs_redraw: bool,
    pub next_anim: Option<Instant>,
    pub avatars: HashMap<u64, Arc<Vec<u8>>>,
    pub place_icons: HashMap<u64, Arc<Vec<u8>>>,
    pub clipboard: Option<arboard::Clipboard>,
    pub start_instant: Instant,
    /// True while validating a saved session at startup.
    pub restoring: bool,
}

impl App {
    pub fn new(tx: Sender<Command>, has_saved_session: bool) -> App {
        App {
            window: None,
            pixels: None,
            fonts: Fonts::load(),
            hits: HitMap::default(),
            cursor: (-1.0, -1.0),
            mouse_down: None,
            drag_scroll: None,
            arrow_hold: None,
            mods: ModifiersState::empty(),
            focus: None,
            screen: Screen::Login(LoginView::new()),
            tx,
            fbw: 1,
            fbh: 1,
            scale: 1.0,
            needs_redraw: true,
            next_anim: None,
            avatars: HashMap::new(),
            place_icons: HashMap::new(),
            clipboard: arboard::Clipboard::new().ok(),
            start_instant: Instant::now(),
            restoring: has_saved_session,
        }
    }

    pub fn send_command(&mut self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    // -----------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------

    fn draw_frame(&mut self) {
        let Some(mut px) = self.pixels.take() else { return };
        {
            let frame = px.frame_mut();
            let mut fb = FB::new(frame, self.fbw, self.fbh);
            fb.fill(theme::BG);
            let now_ms = self.start_instant.elapsed().as_millis() as u64;
            match &mut self.screen {
                Screen::Login(view) => {
                    let mut ctx = UiCtx {
                        fonts: &self.fonts,
                        cursor: self.cursor,
                        hits: &mut self.hits,
                        focus: self.focus,
                        scale: self.scale,
                        now_ms,
                        caret_anim: false,
                        avatars: &mut self.avatars,
                        icons: &mut self.place_icons,
                    };
                    crate::login_ui::draw(view, &mut ctx, &mut fb, self.restoring);
                }
                Screen::Menu(view) => {
                    let mut ctx = UiCtx {
                        fonts: &self.fonts,
                        cursor: self.cursor,
                        hits: &mut self.hits,
                        focus: self.focus,
                        scale: self.scale,
                        now_ms,
                        caret_anim: false,
                        avatars: &mut self.avatars,
                        icons: &mut self.place_icons,
                    };
                    crate::menu_ui::draw(view, &mut ctx, &mut fb);
                }
            }
        }
        let _ = px.render();
        self.pixels = Some(px);
    }

    /// Earliest future redraw needed for animations (spinner, caret, countdown).
    fn schedule_anim(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut wakes: Vec<Instant> = Vec::with_capacity(3);

        let spinner = match &self.screen {
            Screen::Login(v) => v.busy,
            Screen::Menu(v) => v.refreshing || v.add.searching || v.places_searching,
        };
        if spinner {
            wakes.push(now + Duration::from_millis(90));
        }
        if self.focus.is_some() {
            wakes.push(now + Duration::from_millis(530));
        }
        if matches!(self.screen, Screen::Menu(_)) {
            wakes.push(now + Duration::from_millis(1000));
        }
        wakes.into_iter().min()
    }

    // -----------------------------------------------------------------
    // Input helpers
    // -----------------------------------------------------------------

    fn focus_cycle(&mut self) {
        let order: &[FieldId] = match &self.screen {
            Screen::Login(v) => {
                if v.two_step.is_some() {
                    &[FieldId::Code]
                } else {
                    match v.tab {
                        crate::login_ui::LoginTab::Password => &[FieldId::Username, FieldId::Password],
                        crate::login_ui::LoginTab::Cookie => &[FieldId::Cookie],
                    }
                }
            }
            Screen::Menu(v) => match v.tab {
                MenuTab::Home => &[FieldId::Search],
                MenuTab::AddFriends => &[FieldId::AddSearch],
            },
        };
        match self.focus {
            None => self.focus = order.first().copied(),
            Some(cur) => {
                let idx = order.iter().position(|f| *f == cur).unwrap_or(0);
                self.focus = order.get((idx + 1) % order.len()).copied();
            }
        }
        self.request_redraw();
    }

    fn focused_field_mut(&mut self) -> Option<&mut crate::ui::TextField> {
        let focus = self.focus?;
        match &mut self.screen {
            Screen::Login(v) => match focus {
                FieldId::Username => Some(&mut v.username),
                FieldId::Password => Some(&mut v.password),
                FieldId::Cookie => Some(&mut v.cookie),
                FieldId::Code => Some(&mut v.code),
                FieldId::Search | FieldId::AddSearch => None,
            },
            Screen::Menu(v) => match focus {
                FieldId::Search => Some(&mut v.search),
                FieldId::AddSearch => Some(&mut v.add.query),
                _ => None,
            },
        }
    }

    fn paste_clipboard(&mut self) {
        if let Some(text) = self
            .clipboard
            .as_mut()
            .and_then(|c| c.get_text().ok())
            .filter(|t| !t.is_empty())
        {
            if let Some(f) = self.focused_field_mut() {
                f.insert(&text);
                self.needs_redraw = true;
            }
        }
    }

    fn primary_action(&mut self) {
        let tx = self.tx.clone();
        match &mut self.screen {
            Screen::Login(v) => {
                if v.busy {
                    return;
                }
                if let Some(ts) = &v.two_step {
                    let code = v.code.value.trim().to_string();
                    if code.is_empty() {
                        v.status = Some((StatusKind::Error, "Enter the verification code.".into()));
                        return;
                    }
                    let (login, ticket) = (ts.login.clone(), ts.ticket.clone());
                    v.busy = true;
                    v.status = Some((StatusKind::Info, "Verifying\u{2026}".into()));
                    let _ = tx.send(Command::Verify2FA { login, ticket, code, remember: false });
                } else {
                    match v.tab {
                        crate::login_ui::LoginTab::Password => {
                            let login = v.username.value.trim().to_string();
                            let password = v.password.value.clone();
                            if login.is_empty() || password.is_empty() {
                                v.status = Some((StatusKind::Error, "Enter your username and password.".into()));
                                return;
                            }
                            v.pending_login = login.clone();
                            v.busy = true;
                            v.status = Some((StatusKind::Info, "Signing in\u{2026}".into()));
                            let _ = tx.send(Command::LoginPassword { login, password });
                        }
                        crate::login_ui::LoginTab::Cookie => {
                            let token = crate::roblox::client::normalize_cookie_token(&v.cookie.value);
                            if token.is_empty() {
                                v.status = Some((StatusKind::Error, "Paste your .ROBLOSECURITY token first.".into()));
                                return;
                            }
                            v.busy = true;
                            v.status = Some((StatusKind::Info, "Signing in with cookie\u{2026}".into()));
                            let _ = tx.send(Command::LoginCookie { token });
                        }
                    }
                }
                self.needs_redraw = true;
            }
            Screen::Menu(v) => {
                match v.tab {
                    MenuTab::AddFriends => self.run_user_search(),
                    MenuTab::Home => self.run_place_search(),
                }
            }
        }
    }

    /// Kick off a user search from the Add Friends pane.
    fn run_user_search(&mut self) {
        let query = match &self.screen {
            Screen::Menu(v) => v.add.query.value.trim().to_string(),
            _ => return,
        };
        if let Screen::Menu(v) = &mut self.screen {
            if query.chars().count() < 2 {
                v.add.error = Some("Enter at least 2 characters.".into());
                self.needs_redraw = true;
                return;
            }
            v.add.searching = true;
            v.add.error = None;
        }
        self.send_command(Command::SearchUsers { query });
        self.needs_redraw = true;
    }

    /// Kick off a place search from the Home pane (empty query = the default
    /// recommendation feed).
    fn run_place_search(&mut self) {
        let query = match &self.screen {
            Screen::Menu(v) => v.search.value.trim().to_string(),
            _ => return,
        };
        if let Screen::Menu(v) = &mut self.screen {
            v.places_searching = true;
            v.places_error = None;
        }
        self.send_command(Command::SearchPlaces { query });
        self.needs_redraw = true;
    }

    // -----------------------------------------------------------------
    // Screen transitions
    // -----------------------------------------------------------------

    fn enter_menu(&mut self, profile: UserProfile) {
        self.screen = Screen::Menu(MenuView::new(profile));
        self.focus = None;
        self.restoring = false;
        // Kick off the first refresh and the default places feed immediately.
        if let Screen::Menu(v) = &mut self.screen {
            v.refreshing = true;
            v.next_refresh = Some(Instant::now() + Duration::from_secs(REFRESH_SECS));
        }
        self.send_command(Command::StartRefresh);
        self.send_command(Command::SearchPlaces { query: String::new() });
        self.request_redraw();
    }

    fn enter_login(&mut self, status: Option<(StatusKind, String)>) {
        self.screen = Screen::Login(LoginView::with_status(status));
        self.focus = None;
        self.restoring = false;
        self.request_redraw();
    }

    // -----------------------------------------------------------------
    // Worker events
    // -----------------------------------------------------------------

    fn on_worker_event(&mut self, ev: WorkerEvent) {
        match ev {
            WorkerEvent::Login(Ok(outcome)) => match outcome {
                AuthOutcome::Session { security, profile } => {
                    let sess = Session {
                        security_token: security,
                        user_id: profile.id,
                        username: profile.name.clone(),
                        display_name: profile.display_name.clone(),
                    };
                    if let Err(e) = config::save_session(&sess) {
                        eprintln!("warn: could not persist session: {e}");
                    }
                    self.enter_menu(profile);
                }
                AuthOutcome::TwoStep { ticket, media_type } => {
                    if let Screen::Login(v) = &mut self.screen {
                        v.busy = false;
                        v.two_step = Some(crate::login_ui::TwoStepPending {
                            login: v.pending_login.clone(),
                            ticket,
                            media_type,
                        });
                        v.code.clear();
                        v.status = Some((StatusKind::Info, "Enter the verification code.".into()));
                        self.focus = Some(FieldId::Code);
                    }
                    self.request_redraw();
                }
            },
            WorkerEvent::Login(Err(e)) => {
                let msg = e.to_string();
                if self.restoring {
                    config::clear_session();
                    self.enter_login(Some((StatusKind::Error, format!(
                        "Saved session expired. {msg}"
                    ))));
                } else if let Screen::Login(v) = &mut self.screen {
                    v.busy = false;
                    if matches!(e, ApiError::TwoStepRequired { .. }) {
                        // Should not normally happen; handled via AuthOutcome.
                    }
                    v.status = Some((StatusKind::Error, msg));
                }
                self.request_redraw();
            }
            WorkerEvent::TwoStepSent { ticket, media_type } => {
                if let Screen::Login(v) = &mut self.screen {
                    if ticket.is_empty() {
                        v.status = Some((StatusKind::Error, "Could not resend the code.".into()));
                    } else {
                        if let Some(ts) = &mut v.two_step {
                            ts.ticket = ticket;
                            if !media_type.is_empty() {
                                ts.media_type = media_type;
                            }
                        }
                        v.status = Some((StatusKind::Info, "A new code was sent.".into()));
                    }
                }
                self.request_redraw();
            }
            WorkerEvent::RefreshDone(Ok(snap)) => {
                if let Screen::Menu(v) = &mut self.screen {
                    v.rows = build_rows(&snap);
                    v.last_update = Some(Instant::now());
                    v.next_refresh = Some(Instant::now() + Duration::from_secs(REFRESH_SECS));
                    v.refreshing = false;
                    v.error = None;
                    self.needs_redraw = true;
                }
            }
            WorkerEvent::RefreshDone(Err(e)) => {
                if let Screen::Menu(v) = &mut self.screen {
                    v.refreshing = false;
                    v.next_refresh = Some(Instant::now() + Duration::from_secs(BACKOFF_SECS));
                    let session_dead = matches!(
                        e,
                        ApiError::InvalidCookie(_) | ApiError::TwoStepRequired { .. }
                    ) || matches!(&e, ApiError::Status { code: 401, .. });
                    if session_dead {
                        config::clear_session();
                        self.enter_login(Some((StatusKind::Error, format!(
                            "Session expired. {e}"
                        ))));
                        return;
                    }
                    v.error = Some(e.to_string());
                    self.needs_redraw = true;
                }
            }
            WorkerEvent::SearchDone { query, result } => {
                if let Screen::Menu(v) = &mut self.screen {
                    // Ignore stale responses for an edited query.
                    if v.add.query.value.trim() == query {
                        v.add.searching = false;
                        v.add.searched_once = true;
                        match result {
                            Ok(users) => {
                                v.add.results = users;
                                v.add.error = None;
                                v.add_scroll = 0.0;
                            }
                            Err(e) => v.add.error = Some(e.to_string()),
                        }
                        self.needs_redraw = true;
                    }
                }
            }
            WorkerEvent::FriendRequestDone { user_id, result } => {
                if let Screen::Menu(v) = &mut self.screen {
                    v.add.pending.retain(|id| *id != user_id);
                    match result {
                        Ok(()) => {
                            v.add.sent.push(user_id);
                            v.add.failed.remove(&user_id);
                            v.notice = Some(("Friend request sent".into(), Instant::now() + Duration::from_secs(4)));
                        }
                        Err(e) => {
                            v.add.failed.insert(user_id, e.to_string());
                        }
                    }
                    self.needs_redraw = true;
                }
            }
            WorkerEvent::PlacesDone { query, result } => {
                if let Screen::Menu(v) = &mut self.screen {
                    // Ignore stale responses for an edited query.
                    if v.search.value.trim() == query {
                        v.places_searching = false;
                        v.places_done_for = query;
                        match result {
                            Ok(cards) => {
                                v.places = cards;
                                v.places_error = None;
                                v.places_scroll = 0.0;
                            }
                            Err(e) => v.places_error = Some(e.to_string()),
                        }
                        self.needs_redraw = true;
                    }
                }
            }
            WorkerEvent::LoggedOut => {
                config::clear_session();
                self.enter_login(None);
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("RustBlox \u{2014} Unofficial Roblox Client")
            .with_inner_size(winit::dpi::LogicalSize::new(WINDOW_W, WINDOW_H))
            .with_min_inner_size(winit::dpi::LogicalSize::new(880.0, 600.0))
            .with_window_level(WindowLevel::Normal);
        let window = Arc::new(elwt.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        self.scale = window.scale_factor() as f32;
        self.fbw = size.width.max(1) as i32;
        self.fbh = size.height.max(1) as i32;
        let surface = pixels::SurfaceTexture::new(size.width.max(1), size.height.max(1), window.clone());
        self.pixels = Some(pixels::Pixels::new(self.fbw as u32, self.fbh as u32, surface)
            .expect("create pixel surface"));
        self.window = Some(window);
        self.request_redraw();
    }

    fn window_event(&mut self, elwt: &ActiveEventLoop, _window_id: winit::window::WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.send_command(Command::Shutdown);
                elwt.exit();
            }
            WindowEvent::Resized(size) => {
                self.fbw = size.width.max(1) as i32;
                self.fbh = size.height.max(1) as i32;
                if let (Some(px), Some(w)) = (self.pixels.as_mut(), self.window.as_ref()) {
                    let _ = px.resize_buffer(size.width.max(1), size.height.max(1));
                    let _ = px.resize_surface(size.width.max(1), size.height.max(1));
                    self.scale = w.scale_factor() as f32;
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                // Scrollbar drag (whichever list is active).
                if let (Some((start_y, start_scroll)), true) = (
                    self.drag_scroll,
                    self.mouse_down.as_ref().map(|h| matches!(h, Hit::Scrollbar)).unwrap_or(false),
                ) {
                    if let Screen::Menu(v) = &mut self.screen {
                        let (scroll, k) = match v.tab {
                            MenuTab::Home => (
                                &mut v.places_scroll,
                                v.places_metrics.as_ref().map(|m| (m.content_h / m.h.max(1.0)).max(1.0)).unwrap_or(1.0),
                            ),
                            MenuTab::AddFriends => (
                                &mut v.add_scroll,
                                v.add_metrics.as_ref().map(|m| (m.content_h / m.h.max(1.0)).max(1.0)).unwrap_or(1.0),
                            ),
                        };
                        *scroll = (start_scroll + (position.y as f32 - start_y) * k).max(0.0);
                    }
                }
                self.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = (-1.0, -1.0);
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => {
                    let hit = self.hits.at(self.cursor.0, self.cursor.1);
                    if let Hit::Field(id) = hit {
                        self.focus = Some(id);
                    }
                    if let Hit::Scrollbar = hit {
                        if let Screen::Menu(v) = &mut self.screen {
                            let start = match v.tab {
                                MenuTab::Home => v.places_scroll,
                                MenuTab::AddFriends => v.add_scroll,
                            };
                            self.drag_scroll = Some((self.cursor.1, start));
                        }
                    }
                    if matches!(hit, Hit::ScrollUp | Hit::ScrollDown) {
                        // First step immediately, then auto-repeat while held.
                        self.scroll_step(&hit);
                        self.arrow_hold = Some((hit.clone(), Instant::now() + Duration::from_millis(ARROW_REPEAT_DELAY_MS)));
                    }
                    self.mouse_down = Some(hit);
                    self.needs_redraw = true;
                }
                ElementState::Released => {
                    let pressed = self.mouse_down.take();
                    self.drag_scroll = None;
                    self.arrow_hold = None;
                    if let Some(pressed_hit) = pressed {
                        let released_hit = self.hits.at(self.cursor.0, self.cursor.1);
                        if pressed_hit == released_hit {
                            match released_hit {
                                Hit::TabPassword | Hit::TabCookie | Hit::SignIn | Hit::Verify
                                | Hit::Resend | Hit::Back | Hit::Logout | Hit::NavHome | Hit::NavAdd
                                | Hit::AddSearchBtn | Hit::AddUser(_)
                                | Hit::SearchPlacesBtn | Hit::SidebarToggle
                                | Hit::StripLeft | Hit::StripRight | Hit::JoinPlace(_) => {
                                    self.handle_button(released_hit);
                                }
                                Hit::Field(id) => {
                                    self.focus = Some(id);
                                }
                                _ => {}
                            }
                        }
                    }
                    self.request_redraw();
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                if let Screen::Menu(v) = &mut self.screen {
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * 3.0 * theme::ROW_H * self.scale,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    // Over the friends strip: scroll it horizontally.
                    let over_strip = matches!(v.tab, MenuTab::Home)
                        && v.strip_rect.map(|r| r.contains(self.cursor.0, self.cursor.1)).unwrap_or(false);
                    if over_strip {
                        v.strip_scroll = (v.strip_scroll - amount).max(0.0);
                    } else {
                        match v.tab {
                            MenuTab::Home => v.places_scroll = (v.places_scroll - amount).max(0.0),
                            MenuTab::AddFriends => v.add_scroll = (v.add_scroll - amount).max(0.0),
                        }
                    }
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.on_key(event);
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if let Some(f) = self.focused_field_mut() {
                    f.insert(&text);
                    self.needs_redraw = true;
                }
            }
            WindowEvent::Ime(Ime::Enabled | Ime::Disabled) => {}
            WindowEvent::RedrawRequested => {
                self.draw_frame();
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _elwt: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Worker(ev) => self.on_worker_event(ev),
            UserEvent::ImageReady { kind, rgba } => {
                match kind {
                    ImageKind::User(id) => {
                        self.avatars.insert(id, rgba);
                    }
                    ImageKind::Place(id) => {
                        self.place_icons.insert(id, rgba);
                    }
                }
                self.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
        let now = Instant::now();

        // Auto refresh timer.
        if let Screen::Menu(v) = &mut self.screen {
            if !v.refreshing {
                if let Some(next) = v.next_refresh {
                    if now >= next {
                        v.refreshing = true;
                        let tx = self.tx.clone();
                        let _ = tx.send(Command::StartRefresh);
                        self.needs_redraw = true;
                    }
                }
            }
        }

        // Auto repeat for a held scroll arrow.
        let mut arrow_tick: Option<Hit> = None;
        if let Some((hit, next_tick)) = &self.arrow_hold {
            if now >= *next_tick {
                arrow_tick = Some(hit.clone());
            }
        }
        if let Some(hit) = arrow_tick {
            self.scroll_step(&hit);
            self.arrow_hold = Some((hit, now + Duration::from_millis(ARROW_REPEAT_MS)));
            self.needs_redraw = true;
        }

        if self.needs_redraw {
            self.needs_redraw = false;
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }

        // Compute the earliest wake-up for animations / timers.
        let mut wake: Option<Instant> = self.schedule_anim();
        if let Screen::Menu(v) = &self.screen {
            if !v.refreshing {
                if let Some(next) = v.next_refresh {
                    wake = Some(match wake {
                        Some(w) => w.min(next),
                        None => next,
                    });
                }
            }
        }
        match wake {
            Some(t) if t > now => elwt.set_control_flow(ControlFlow::WaitUntil(t)),
            Some(_) => {
                // Animation tick due: redraw on the next frame.
                self.needs_redraw = true;
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
                elwt.set_control_flow(ControlFlow::Poll);
            }
            None => elwt.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl App {
    /// Scroll the active list/strip by a step in the direction of the hit.
    fn scroll_step(&mut self, hit: &Hit) {
        if let Screen::Menu(v) = &mut self.screen {
            let d = 1.5 * theme::ROW_H * self.scale;
            match (&v.tab, hit) {
                (MenuTab::Home, Hit::ScrollUp) => v.places_scroll = (v.places_scroll - d).max(0.0),
                (MenuTab::Home, Hit::ScrollDown) => {
                    let max = v.places_metrics.as_ref().map(|m| (m.content_h - m.h).max(0.0)).unwrap_or(0.0);
                    v.places_scroll = (v.places_scroll + d).min(max);
                }
                (MenuTab::AddFriends, Hit::ScrollUp) => v.add_scroll = (v.add_scroll - d).max(0.0),
                (MenuTab::AddFriends, Hit::ScrollDown) => {
                    let max = v.add_metrics.as_ref().map(|m| (m.content_h - m.h).max(0.0)).unwrap_or(0.0);
                    v.add_scroll = (v.add_scroll + d).min(max);
                }
                (_, Hit::StripLeft) => {
                    let d = 3.0 * (theme::STRIP_CARD_W + 8.0) * self.scale;
                    v.strip_scroll = (v.strip_scroll - d).max(0.0);
                }
                (_, Hit::StripRight) => {
                    let d = 3.0 * (theme::STRIP_CARD_W + 8.0) * self.scale;
                    let max = v.strip_metrics.as_ref().map(|m| (m.content_h - m.h).max(0.0)).unwrap_or(0.0);
                    v.strip_scroll = (v.strip_scroll + d).min(max);
                }
                _ => {}
            }
        }
        self.needs_redraw = true;
    }

    fn handle_button(&mut self, hit: Hit) {
        match hit {
            Hit::TabPassword | Hit::TabCookie => {
                if let Screen::Login(v) = &mut self.screen {
                    v.tab = if hit == Hit::TabPassword { crate::login_ui::LoginTab::Password } else { crate::login_ui::LoginTab::Cookie };
                    v.status = None;
                    self.focus = None;
                }
                self.request_redraw();
            }
            Hit::SignIn | Hit::Verify => {
                self.primary_action();
            }
            Hit::Resend => {
                let tx = self.tx.clone();
                if let Screen::Login(v) = &mut self.screen {
                    if let Some(ts) = &v.two_step {
                        let (login, ticket) = (ts.login.clone(), ts.ticket.clone());
                        let _ = tx.send(Command::Resend2FA { login, ticket });
                        v.status = Some((StatusKind::Info, "Resending…".into()));
                    }
                }
                self.needs_redraw = true;
            }
            Hit::Back => {
                if let Screen::Login(v) = &mut self.screen {
                    v.two_step = None;
                    v.status = None;
                    self.focus = Some(FieldId::Password);
                }
                self.needs_redraw = true;
            }
            Hit::NavHome => {
                if let Screen::Menu(v) = &mut self.screen {
                    v.tab = MenuTab::Home;
                    self.focus = None;
                }
                self.needs_redraw = true;
            }
            Hit::NavAdd => {
                if let Screen::Menu(v) = &mut self.screen {
                    v.tab = MenuTab::AddFriends;
                    self.focus = Some(FieldId::AddSearch);
                }
                self.needs_redraw = true;
            }
            Hit::AddSearchBtn => {
                self.run_user_search();
            }
            Hit::SearchPlacesBtn => {
                self.run_place_search();
            }
            Hit::SidebarToggle => {
                if let Screen::Menu(v) = &mut self.screen {
                    v.collapsed = !v.collapsed;
                }
                self.needs_redraw = true;
            }
            Hit::StripLeft | Hit::StripRight => {
                self.scroll_step(&hit);
            }
            Hit::JoinPlace(place_id) => {
                let _ = place_id; // joining a server is not implemented yet
                if let Screen::Menu(v) = &mut self.screen {
                    v.notice = Some((
                        "Joining places is not implemented yet \u{2014} coming soon".into(),
                        Instant::now() + Duration::from_secs(4),
                    ));
                }
                self.needs_redraw = true;
            }
            Hit::AddUser(user_id) => {
                if let Screen::Menu(v) = &mut self.screen {
                    let already = v.add.pending.contains(&user_id) || v.add.sent.contains(&user_id);
                    if !already {
                        v.add.mark_pending(user_id);
                        self.send_command(Command::AddFriend { user_id });
                    }
                }
                self.needs_redraw = true;
            }
            Hit::Logout => {
                self.send_command(Command::Logout);
            }
            _ => {}
        }
    }

    fn on_key(&mut self, event: KeyEvent) {
        let ctrl = self.mods.contains(ModifiersState::CONTROL);
        // Global shortcuts first.
        if ctrl && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("v")) {
            self.paste_clipboard();
            return;
        }
        if ctrl && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("a")) {
            if let Some(f) = self.focused_field_mut() {
                f.select_all();
                self.request_redraw();
            }
            return;
        }
        if ctrl && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("f")) {
            if matches!(self.screen, Screen::Menu(_)) {
                self.focus = Some(match &self.screen {
                    Screen::Menu(v) if v.tab == MenuTab::AddFriends => FieldId::AddSearch,
                    _ => FieldId::Search,
                });
                self.request_redraw();
            }
            return;
        }

        match &event.logical_key {
            Key::Named(NamedKey::Tab) => {
                self.focus_cycle();
                return;
            }
            Key::Named(NamedKey::Enter) => {
                self.primary_action();
                return;
            }
            Key::Named(NamedKey::Escape) => {
                match &mut self.screen {
                    Screen::Login(v) => {
                        if v.two_step.is_some() {
                            v.two_step = None;
                            v.status = None;
                            v.busy = false;
                            self.focus = Some(FieldId::Password);
                        } else {
                            self.focus = None;
                        }
                    }
                    Screen::Menu(v) => {
                        if v.tab == MenuTab::AddFriends {
                            // Esc goes back to the friends list.
                            v.tab = MenuTab::Home;
                            self.focus = None;
                        } else {
                            self.focus = None;
                        }
                    }
                }
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) => {
                // Keyboard scrolling of the active list.
                let dir = if matches!(&event.logical_key, Key::Named(NamedKey::ArrowUp)) { Hit::ScrollUp } else { Hit::ScrollDown };
                if matches!(self.screen, Screen::Menu(_)) {
                    self.scroll_step(&dir);
                }
                return;
            }
            _ => {}
        }

        // Field editing keys.
        let mods = self.mods;
        let mut edited = false;
        if let Some(f) = self.focused_field_mut() {
            match &event.logical_key {
                Key::Named(NamedKey::Backspace) => {
                    f.backspace();
                    edited = true;
                }
                Key::Named(NamedKey::Delete) => {
                    f.delete();
                    edited = true;
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    f.left();
                    edited = true;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    f.right();
                    edited = true;
                }
                Key::Named(NamedKey::Home) => {
                    f.home();
                    edited = true;
                }
                Key::Named(NamedKey::End) => {
                    f.end();
                    edited = true;
                }
                Key::Named(NamedKey::Space) => {
                    if char_input_allowed(mods) {
                        f.insert(" ");
                        edited = true;
                    }
                }
                Key::Character(text) => {
                    // Plain text entry (letters, digits, punctuation,
                    // Cyrillic…). Shortcut combos are filtered by
                    // char_input_allowed; Ctrl+V/A were already handled above.
                    if char_input_allowed(mods) {
                        f.insert(text);
                        edited = true;
                    }
                }
                _ => {}
            }
        }
        if edited {
            self.request_redraw();
        }
    }
}

/// Whether a `Key::Character` press with these modifiers should insert text.
///
/// * No modifiers, or Shift only — insert.
/// * Ctrl or Alt alone — a shortcut, do not insert (Ctrl+V/A are handled
///   earlier in [`App::on_key`]).
/// * Ctrl+Alt together — that is AltGr on Windows / European layouts, which
///   produces printable characters (`\`, `@`, `#`, `€`…), so insert.
/// * Super (Cmd/Win) — system shortcut, do not insert.
pub fn char_input_allowed(mods: ModifiersState) -> bool {
    let ctrl = mods.contains(ModifiersState::CONTROL);
    let alt = mods.contains(ModifiersState::ALT);
    let sup = mods.contains(ModifiersState::SUPER);
    if sup {
        return false;
    }
    // Exactly one of ctrl/alt => shortcut. Both (or neither) => text.
    !(ctrl ^ alt)
}

#[cfg(test)]
mod tests {
    use super::char_input_allowed;
    use winit::keyboard::ModifiersState;

    #[test]
    fn plain_and_shifted_typing_allowed() {
        assert!(char_input_allowed(ModifiersState::empty()));
        assert!(char_input_allowed(ModifiersState::SHIFT));
    }

    #[test]
    fn shortcut_modifiers_rejected() {
        assert!(!char_input_allowed(ModifiersState::CONTROL));
        assert!(!char_input_allowed(ModifiersState::ALT));
        assert!(!char_input_allowed(ModifiersState::SUPER));
        assert!(!char_input_allowed(ModifiersState::CONTROL | ModifiersState::SHIFT));
    }

    #[test]
    fn altgr_counts_as_text() {
        // AltGr is reported as Ctrl+Alt on Windows/EU layouts.
        assert!(char_input_allowed(ModifiersState::CONTROL | ModifiersState::ALT));
    }
}
