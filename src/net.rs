//! Background network worker.
//!
//! The UI thread never blocks: commands are queued to the worker thread, and
//! results are delivered back through the winit `EventLoopProxy` so the event
//! loop wakes up immediately. Avatar downloads run on a small thread pool and
//! are cached on disk keyed by the thumbnail URL. Note that avatars are
//! fetched from the CDN *without* the session cookie (it must never leak to
//! third-party CDN hosts).

use crate::config::Session;
use crate::model::{PlaceCard, Presence, PresenceKind, Snapshot, UserEntry};
use crate::roblox::auth::{self, AuthOutcome};
use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use crate::roblox::social;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use winit::event_loop::EventLoopProxy;

#[derive(Debug)]
pub enum Command {
    LoginPassword { login: String, password: String },
    Verify2FA { login: String, ticket: String, code: String, remember: bool },
    Resend2FA { login: String, ticket: String },
    LoginCookie { token: String },
    StartRefresh,
    SearchUsers { query: String },
    AddFriend { user_id: u64 },
    /// Place search: an empty query requests the default recommendation feed.
    SearchPlaces { query: String },
    /// List incoming friend requests.
    FetchFriendRequests,
    /// Accept (true) or decline (false) an incoming friend request.
    RespondFriendRequest { user_id: u64, accept: bool },
    Logout,
    Shutdown,
}

#[derive(Debug)]
pub enum WorkerEvent {
    Login(Result<AuthOutcome, ApiError>),
    TwoStepSent { ticket: String, media_type: String },
    RefreshDone(Result<Snapshot, ApiError>),
    SearchDone { query: String, result: Result<Vec<UserEntry>, ApiError> },
    FriendRequestDone { user_id: u64, result: Result<(), ApiError> },
    PlacesDone { query: String, result: Result<Vec<PlaceCard>, ApiError> },
    FriendRequestsDone { result: Result<Vec<UserEntry>, ApiError> },
    RequestResponded { user_id: u64, accept: bool, result: Result<(), ApiError> },
    /// Recently played places (discovery "continue" row + local tracking).
    RecentlyPlayedDone { result: Vec<PlaceCard> },
    LoggedOut,
}

/// Which slot a downloaded bitmap belongs to (users and places are kept in
/// separate maps, their id spaces can overlap).
#[derive(Debug, Clone, PartialEq)]
pub enum ImageKind {
    User(u64),
    Place(u64),
}

#[derive(Debug)]
pub enum UserEvent {
    Worker(WorkerEvent),
    ImageReady { kind: ImageKind, rgba: Arc<Vec<u8>> },
}

pub struct Worker {
    pub tx: Sender<Command>,
}

struct ImageJob {
    kind: ImageKind,
    url: String,
}

pub fn spawn(proxy: EventLoopProxy<UserEvent>, session: Option<Session>) -> Worker {
    let (tx, rx) = channel::<Command>();
    let proxy = Arc::new(proxy);

    // --- Image download pool (avatars + place icons) -----------------------
    let (job_tx, job_rx) = channel::<ImageJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    for _ in 0..4 {
        let job_rx = Arc::clone(&job_rx);
        let proxy = Arc::clone(&proxy);
        std::thread::Builder::new()
            .name("rustblox-images".into())
            .spawn(move || loop {
                let job = {
                    let guard = match job_rx.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    guard.recv()
                };
                let job = match job {
                    Ok(j) => j,
                    Err(_) => return,
                };
                let size = match job.kind {
                    ImageKind::User(_) => crate::theme::AVATAR_SIZE,
                    ImageKind::Place(_) => crate::theme::PLACE_ICON_PX,
                };
                if let Some(bytes) = crate::avatar_cache::load(&job.url, size) {
                    let _ = proxy.send_event(UserEvent::ImageReady {
                        kind: job.kind,
                        rgba: Arc::new(bytes),
                    });
                    continue;
                }
                let agent = crate::avatar_cache::cdn_agent();
                if let Ok(rgba) = social::download_image(&agent, &job.url, size) {
                    crate::avatar_cache::store(&job.url, &rgba);
                    let _ = proxy.send_event(UserEvent::ImageReady {
                        kind: job.kind,
                        rgba: Arc::new(rgba),
                    });
                }
            })
            .expect("spawn image worker");
    }

    // --- Main worker --------------------------------------------------------
    let pushed: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let icons_pushed: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
    {
        let pushed = Arc::clone(&pushed);
        let icons_pushed = Arc::clone(&icons_pushed);
        std::thread::Builder::new()
            .name("rustblox-worker".into())
            .spawn(move || {
                let mut client = RobloxClient::new();
                let mut me_id: Option<u64> = None;

                // Restore-session validation happens once at startup.
                if let Some(sess) = session {
                    match auth::login_cookie(&mut client, &sess.security_token) {
                        Ok(outcome) => {
                            if let AuthOutcome::Session { profile, .. } = &outcome {
                                me_id = Some(profile.id);
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::Login(Ok(outcome))));
                        }
                        Err(e) => {
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::Login(Err(e))));
                        }
                    }
                }

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Command::Shutdown => break,
                        Command::LoginPassword { login, password } => {
                            let outcome = auth::login_password(&mut client, &login, &password);
                            if let Ok(AuthOutcome::Session { profile, .. }) = &outcome {
                                me_id = Some(profile.id);
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::Login(outcome)));
                        }
                        Command::Verify2FA { login, ticket, code, remember } => {
                            let outcome = auth::verify_2fa(&mut client, &login, &ticket, &code, remember);
                            if let Ok(AuthOutcome::Session { profile, .. }) = &outcome {
                                me_id = Some(profile.id);
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::Login(outcome)));
                        }
                        Command::Resend2FA { login, ticket } => {
                            let ev = match auth::resend_2fa(&mut client, &login, &ticket) {
                                Ok((ticket, media_type)) => WorkerEvent::TwoStepSent { ticket, media_type },
                                Err(_) => WorkerEvent::TwoStepSent { ticket, media_type: String::new() },
                            };
                            let _ = proxy.send_event(UserEvent::Worker(ev));
                        }
                        Command::LoginCookie { token } => {
                            let outcome = auth::login_cookie(&mut client, &token);
                            if let Ok(AuthOutcome::Session { profile, .. }) = &outcome {
                                me_id = Some(profile.id);
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::Login(outcome)));
                        }
                        Command::Logout => {
                            auth::logout(&mut client);
                            me_id = None;
                            pushed.lock().unwrap().clear();
                            icons_pushed.lock().unwrap().clear();
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::LoggedOut));
                        }
                        Command::StartRefresh => {
                            if me_id.is_none() {
                                continue;
                            }
                            let snap = run_refresh(&mut client, me_id.unwrap(), job_tx.clone(), &pushed);
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::RefreshDone(snap)));
                            // Piggyback the recently played feed on every
                            // refresh (official "continue" row + local).
                            let recent = collect_recently_played(
                                &mut client,
                                job_tx.clone(),
                                &icons_pushed,
                            );
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::RecentlyPlayedDone { result: recent }));
                        }
                        Command::SearchUsers { query } => {
                            let result = social::search_users(&mut client, &query, 25);
                            match &result {
                                Ok(users) if !users.is_empty() => {
                                    // Resolve headshots for the results so the
                                    // rows get avatars via the download pool.
                                    let ids: Vec<u64> = users.iter().map(|u| u.id).collect();
                                    if let Ok(index) = social::fetch_avatar_index(&mut client, &ids) {
                                        let mut p = pushed.lock().unwrap();
                                        for (id, url) in index {
                                            if p.get(&id).map(|old| old != &url).unwrap_or(true) {
                                                let job = ImageJob { kind: ImageKind::User(id), url: url.clone() };
                                                if job_tx.send(job).is_ok() {
                                                    p.insert(id, url);
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::SearchDone { query, result }));
                        }
                        Command::AddFriend { user_id } => {
                            let result = social::send_friend_request(&mut client, user_id);
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::FriendRequestDone { user_id, result }));
                        }
                        Command::SearchPlaces { query } => {
                            // Empty query -> default recommendation feed.
                            let result = if query.trim().is_empty() {
                                social::recommended_games(&mut client, 18)
                            } else {
                                social::search_games(&mut client, &query, 18)
                            };
                            if let Ok(cards) = &result {
                                push_place_icons(&mut client, cards, &job_tx, &icons_pushed);
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::PlacesDone { query, result }));
                        }
                        Command::FetchFriendRequests => {
                            // The endpoint targets the logged-in user ("my"),
                            // so no user id is needed; unauthenticated calls
                            // fail with 401 and surface as an error message.
                            let result = social::fetch_friend_requests(&mut client);
                            if let Ok(users) = &result {
                                let ids: Vec<u64> = users.iter().map(|u| u.id).collect();
                                if let Ok(index) = social::fetch_avatar_index(&mut client, &ids) {
                                    let mut p = pushed.lock().unwrap();
                                    for (id, url) in index {
                                        if p.get(&id).map(|old| old != &url).unwrap_or(true) {
                                            let job = ImageJob { kind: ImageKind::User(id), url: url.clone() };
                                            if job_tx.send(job).is_ok() {
                                                p.insert(id, url);
                                            }
                                        }
                                    }
                                }
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::FriendRequestsDone { result }));
                        }
                        Command::RespondFriendRequest { user_id, accept } => {
                            let result = if accept {
                                social::accept_friend_request(&mut client, user_id)
                            } else {
                                social::decline_friend_request(&mut client, user_id)
                            };
                            if result.is_ok() {
                                // The friends list changed: force the next
                                // refresh to pick it up immediately.
                                if let Some(id) = me_id {
                                    let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::RefreshDone(run_refresh(&mut client, id, job_tx.clone(), &pushed))));
                                }
                            }
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::RequestResponded { user_id, accept, result }));
                        }
                    }
                }
            })
            .expect("spawn network worker");
    }

    Worker { tx }
}

/// Resolve + enqueue place icons for a card list (dedup across pushes).
fn push_place_icons(
    c: &mut RobloxClient,
    cards: &[PlaceCard],
    job_tx: &Sender<ImageJob>,
    icons_pushed: &Mutex<HashMap<u64, String>>,
) {
    let place_ids: Vec<u64> = cards.iter().map(|p| p.root_place_id).collect();
    let icons = social::fetch_game_icons(c, &place_ids);
    let mut p = icons_pushed.lock().unwrap();
    for (place_id, url) in icons {
        if p.get(&place_id).map(|old| old != &url).unwrap_or(true) {
            let job = ImageJob { kind: ImageKind::Place(place_id), url: url.clone() };
            if job_tx.send(job).is_ok() {
                p.insert(place_id, url);
            }
        }
    }
}

/// Recently played = the official discovery "continue" row, extended with
/// locally tracked places (recorded while the client is open). Best effort:
/// never fails, returns whatever source worked.
fn collect_recently_played(
    c: &mut RobloxClient,
    job_tx: Sender<ImageJob>,
    icons_pushed: &Mutex<HashMap<u64, String>>,
) -> Vec<PlaceCard> {
    const LIMIT: usize = 12;
    let mut out: Vec<PlaceCard> = Vec::new();
    if let Ok(cards) = social::recently_played(c, LIMIT) {
        out = cards;
    }
    // Local fallback / supplement from settings.json.
    let local = crate::config::load_settings().recent_places;
    if !local.is_empty() {
        let universes: Vec<u64> = local.iter().map(|r| r.universe_id).filter(|u| *u != 0).collect();
        let names = social::fetch_game_names(c, &universes);
        for r in local {
            if r.root_place_id == 0 || out.iter().any(|c| c.root_place_id == r.root_place_id) {
                continue;
            }
            if out.len() >= LIMIT {
                break;
            }
            let name = if r.name.is_empty() {
                names.get(&r.universe_id).cloned().unwrap_or_default()
            } else {
                r.name
            };
            if name.is_empty() {
                continue;
            }
            out.push(PlaceCard {
                universe_id: r.universe_id,
                root_place_id: r.root_place_id,
                name,
                creator: String::new(),
                player_count: 0,
                likes_pct: None,
                sponsored: false,
            });
        }
    }
    if !out.is_empty() {
        push_place_icons(c, &out, &job_tx, icons_pushed);
    }
    out
}

/// Full refresh pipeline: friends -> presence -> last-online -> game names,
/// then enqueue avatar downloads for URLs we have not pushed yet.
fn run_refresh(
    c: &mut RobloxClient,
    me_id: u64,
    job_tx: Sender<ImageJob>,
    pushed: &Mutex<HashMap<u64, String>>,
) -> Result<Snapshot, ApiError> {
    let friends = social::fetch_friends(c, me_id)?;
    let ids: Vec<u64> = friends.iter().map(|f| f.id).collect();

    // Presence for friends AND ourselves: our own entry feeds the local
    // "recently played" tracker (settings.json).
    let mut lookup: Vec<u64> = ids.clone();
    lookup.push(me_id);
    let presences: HashMap<u64, Presence> = social::fetch_presence(c, &lookup)?;
    let last_online = social::fetch_last_online(c, &ids);

    let universes: Vec<u64> = presences
        .values()
        .filter(|p| p.kind == PresenceKind::InGame)
        .filter_map(|p| p.universe_id)
        .collect();
    let games = social::fetch_game_names(c, &universes);

    // Track the place we are playing right now for the recently played row.
    if let Some(me) = presences.get(&me_id) {
        if matches!(me.kind, PresenceKind::InGame | PresenceKind::Studio) {
            if let (Some(root), uni) = (me.root_place_id, me.universe_id) {
                let name = uni
                    .and_then(|u| games.get(&u).cloned())
                    .or_else(|| crate::model::clean_last_location(&me.last_location))
                    .unwrap_or_default();
                crate::config::note_recent_place(uni.unwrap_or(0), root, &name);
            }
        }
    }

    // Avatars: resolve thumbnail URLs and enqueue downloads for anything not
    // pushed before (dedup across refreshes; disk cache absorbs the rest).
    // Includes our own id so the sidebar profile chip shows the real avatar.
    if let Ok(index) = social::fetch_avatar_index(c, &[ids.as_slice(), &[me_id]].concat()) {
        let mut p = pushed.lock().unwrap();
        for (id, url) in index {
            if p.get(&id).map(|old| old != &url).unwrap_or(true) {
                let job = ImageJob { kind: ImageKind::User(id), url: url.clone() };
                if job_tx.send(job).is_ok() {
                    p.insert(id, url);
                }
            }
        }
    }
    Ok(Snapshot { friends, presences, games, last_online })
}
