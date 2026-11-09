//! Background network worker.
//!
//! The UI thread never blocks: commands are queued to the worker thread, and
//! results are delivered back through the winit `EventLoopProxy` so the event
//! loop wakes up immediately. Avatar downloads run on a small thread pool and
//! are cached on disk keyed by the thumbnail URL. Note that avatars are
//! fetched from the CDN *without* the session cookie (it must never leak to
//! third-party CDN hosts).

use crate::config::Session;
use crate::model::{Presence, PresenceKind, Snapshot, UserEntry};
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
    LoggedOut,
}

#[derive(Debug)]
pub enum UserEvent {
    Worker(WorkerEvent),
    AvatarReady { user_id: u64, rgba: Arc<Vec<u8>> },
}

pub struct Worker {
    pub tx: Sender<Command>,
}

struct AvatarJob {
    user_id: u64,
    url: String,
}

pub fn spawn(proxy: EventLoopProxy<UserEvent>, session: Option<Session>) -> Worker {
    let (tx, rx) = channel::<Command>();
    let proxy = Arc::new(proxy);

    // --- Avatar download pool ---------------------------------------------
    let (job_tx, job_rx) = channel::<AvatarJob>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    for _ in 0..4 {
        let job_rx = Arc::clone(&job_rx);
        let proxy = Arc::clone(&proxy);
        std::thread::Builder::new()
            .name("rustblox-avatar".into())
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
                if let Some(bytes) = crate::avatar_cache::load(&job.url) {
                    let _ = proxy.send_event(UserEvent::AvatarReady {
                        user_id: job.user_id,
                        rgba: Arc::new(bytes),
                    });
                    continue;
                }
                let agent = crate::avatar_cache::cdn_agent();
                if let Ok(rgba) = social::download_avatar(&agent, &job.url, crate::theme::AVATAR_SIZE) {
                    crate::avatar_cache::store(&job.url, &rgba);
                    let _ = proxy.send_event(UserEvent::AvatarReady {
                        user_id: job.user_id,
                        rgba: Arc::new(rgba),
                    });
                }
            })
            .expect("spawn avatar worker");
    }

    // --- Main worker --------------------------------------------------------
    let pushed: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
    {
        let pushed = Arc::clone(&pushed);
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
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::LoggedOut));
                        }
                        Command::StartRefresh => {
                            if me_id.is_none() {
                                continue;
                            }
                            let snap = run_refresh(&mut client, me_id.unwrap(), job_tx.clone(), &pushed);
                            let _ = proxy.send_event(UserEvent::Worker(WorkerEvent::RefreshDone(snap)));
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
                                                if job_tx.send(AvatarJob { user_id: id, url: url.clone() }).is_ok() {
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
                    }
                }
            })
            .expect("spawn network worker");
    }

    Worker { tx }
}

/// Full refresh pipeline: friends -> presence -> last-online -> game names,
/// then enqueue avatar downloads for URLs we have not pushed yet.
fn run_refresh(
    c: &mut RobloxClient,
    me_id: u64,
    job_tx: Sender<AvatarJob>,
    pushed: &Mutex<HashMap<u64, String>>,
) -> Result<Snapshot, ApiError> {
    let friends = social::fetch_friends(c, me_id)?;
    let ids: Vec<u64> = friends.iter().map(|f| f.id).collect();

    let presences: HashMap<u64, Presence> = if ids.is_empty() {
        HashMap::new()
    } else {
        social::fetch_presence(c, &ids)?
    };
    let last_online = social::fetch_last_online(c, &ids);

    let universes: Vec<u64> = presences
        .values()
        .filter(|p| p.kind == PresenceKind::InGame)
        .filter_map(|p| p.universe_id)
        .collect();
    let games = social::fetch_game_names(c, &universes);

    // Avatars: resolve thumbnail URLs and enqueue downloads for anything not
    // pushed before (dedup across refreshes; disk cache absorbs the rest).
    // Includes our own id so the sidebar profile chip shows the real avatar.
    if let Ok(index) = social::fetch_avatar_index(c, &[ids.as_slice(), &[me_id]].concat()) {
        let mut p = pushed.lock().unwrap();
        for (id, url) in index {
            if p.get(&id).map(|old| old != &url).unwrap_or(true) {
                if job_tx.send(AvatarJob { user_id: id, url: url.clone() }).is_ok() {
                    p.insert(id, url);
                }
            }
        }
    }
    Ok(Snapshot { friends, presences, games, last_online })
}
