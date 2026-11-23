//! Menu data worker events: refresh, searches, friends and places.

use crate::app::state::{Screen, StatusKind, BACKOFF_SECS, REFRESH_SECS};
use crate::app::App;
use crate::config;
use crate::model::build_rows;
use crate::roblox::errors::ApiError;
use crate::worker::WorkerEvent;
use std::time::{Duration, Instant};

impl App {
	pub(super) fn on_menu_event(&mut self, ev: WorkerEvent) {
		match ev {
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
					let session_dead =
						matches!(e, ApiError::InvalidCookie(_) | ApiError::TwoStepRequired { .. })
							|| matches!(&e, ApiError::Status { code: 401, .. });
					if session_dead {
						config::clear_session();
						self.enter_login(Some((
							StatusKind::Error,
							format!("Session expired. {e}"),
						)));
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
							v.notice = Some((
								"Friend request sent".into(),
								Instant::now() + Duration::from_secs(4),
							));
						}
						Err(e) => {
							v.add.failed.insert(user_id, e.to_string());
						}
					}
					self.needs_redraw = true;
				}
			}
			WorkerEvent::FriendRequestsDone { result } => {
				if let Screen::Menu(v) = &mut self.screen {
					v.requests.loading = false;
					v.requests.loaded_once = true;
					match result {
						Ok(users) => {
							v.requests.items = users;
							v.requests.error = None;
							v.requests.failed.clear();
							v.requests_scroll = 0.0;
						}
						Err(e) => v.requests.error = Some(e.to_string()),
					}
					self.needs_redraw = true;
				}
			}
			WorkerEvent::RequestResponded { user_id, accept, result } => {
				if let Screen::Menu(v) = &mut self.screen {
					v.requests.acting.retain(|id| *id != user_id);
					match result {
						Ok(()) => {
							v.requests.items.retain(|u| u.id != user_id);
							v.requests.failed.remove(&user_id);
							let note = if accept {
								"Friend request accepted"
							} else {
								"Friend request declined"
							};
							v.notice = Some((note.into(), Instant::now() + Duration::from_secs(4)));
						}
						Err(e) => {
							v.requests.failed.insert(user_id, e.to_string());
						}
					}
					self.needs_redraw = true;
				}
			}
			WorkerEvent::RecentlyPlayedDone { result } => {
				if let Screen::Menu(v) = &mut self.screen {
					v.recent = result;
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
			_ => {}
		}
	}
}
