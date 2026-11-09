//! Parser and helper tests. Fixture shapes match the live Roblox API
//! responses captured from the official endpoints.

use rustblox::model::{
    build_rows, humanize_last_online, parse_iso8601_utc, sort_rows, Presence, PresenceKind, Snapshot,
};
use rustblox::roblox::client::normalize_cookie_token;
use rustblox::roblox::errors::ApiError;
use std::collections::HashMap;

const PRESENCE_JSON: &str = r#"{"userPresences":[
  {"userPresenceType":0,"lastLocation":"Website","placeId":null,"rootPlaceId":null,"gameId":null,"universeId":null,"userId":1},
  {"userPresenceType":2,"lastLocation":"In Game","placeId":1818,"rootPlaceId":1818,"gameId":"abc-123","universeId":99,"userId":2}
]}"#;

const FRIENDS_JSON: &str = r#"{"data":[
  {"id":2,"name":"bob","displayName":"Bob","created":"2020-01-01T00:00:00Z","isBanned":false,"description":""},
  {"id":3,"name":"alice","displayName":"Alice","created":"2020-01-01T00:00:00Z","isBanned":false,"description":""}
],"nextPageCursor":null}"#;

const THUMBS_JSON: &str = r#"{"data":[
  {"targetId":156,"state":"Completed","imageUrl":"https://tr.rbxcdn.com/30DAY-AvatarHeadshot-635A-Png/150/150/AvatarHeadshot/Png/noFilter","version":"TN3"},
  {"targetId":261,"state":"Pending","imageUrl":null}
]}"#;

#[test]
fn parses_presence_response() {
    let p: rustblox::roblox::types::PresenceResponse = serde_json::from_str(PRESENCE_JSON).unwrap();
    assert_eq!(p.user_presences.len(), 2);
    let offline = p.user_presences.iter().find(|e| e.user_id == 1).unwrap();
    let ingame = p.user_presences.iter().find(|e| e.user_id == 2).unwrap();
    assert_eq!(PresenceKind::from_api(offline.presence_type), PresenceKind::Offline);
    assert_eq!(PresenceKind::from_api(ingame.presence_type), PresenceKind::InGame);
    assert_eq!(ingame.universe_id, Some(99));
    assert_eq!(ingame.game_id.as_deref(), Some("abc-123"));
}

#[test]
fn parses_friends_response() {
    let f: rustblox::roblox::types::FriendsResponse = serde_json::from_str(FRIENDS_JSON).unwrap();
    assert_eq!(f.data.len(), 2);
    assert_eq!(f.data[0].display_name.as_deref(), Some("Bob"));
}

#[test]
fn parses_thumbnails_response() {
    let t: rustblox::roblox::types::ThumbsResponse = serde_json::from_str(THUMBS_JSON).unwrap();
    assert_eq!(t.data.len(), 2);
    let done: Vec<_> = t.data.into_iter().filter(|d| d.state == "Completed").collect();
    assert_eq!(done.len(), 1);
    assert!(done[0].image_url.is_some());
}

#[test]
fn login_error_envelopes() {
    let e = ApiError::from_body(401, br#"{"errors":[{"code":1,"message":"Incorrect username or password. Please try again."}]}"#);
    assert!(matches!(e, ApiError::InvalidCredentials));
    // Roblox also returns code 1 with HTTP 403 in some flows.
    let e = ApiError::from_body(403, br#"{"errors":[{"code":1,"message":"Incorrect username or password. Please try again."}]}"#);
    assert!(matches!(e, ApiError::InvalidCredentials));

    let e = ApiError::from_body(403, br#"{"code":0,"message":"Token Validation Failed"}"#);
    assert!(matches!(e, ApiError::Status { code: 403, .. }));

    let e = ApiError::from_body(401, br#"{"errors":[{"code":8,"message":"Verification required captcha challenge"}]}"#);
    assert!(matches!(e, ApiError::CaptchaRequired));

    let e = ApiError::from_body(401, br#"{"errors":[{"code":9002,"subcode":0,"message":"Authentication token is missing"}]}"#);
    assert!(matches!(e, ApiError::Status { code: 401, .. }));
}

#[test]
fn two_step_body_parses() {
    let body = br#"{"user":null,"twoStepVerificationData":{"mediaType":"Email","ticket":"TICKET-1"},"errors":[]}"#;
    let lr: rustblox::roblox::types::LoginResponse = serde_json::from_slice(body).unwrap();
    let ts = lr.two_step.expect("two step data");
    assert_eq!(ts.ticket, "TICKET-1");
    assert_eq!(ts.media_type, "Email");
}

#[test]
fn iso8601_parsing() {
    assert_eq!(parse_iso8601_utc("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso8601_utc("2024-05-01T12:34:56.789Z"), Some(1714566896));
    assert_eq!(parse_iso8601_utc("2024-05-01T12:34:56Z"), Some(1714566896));
    assert_eq!(parse_iso8601_utc("garbage"), None);
}

#[test]
fn humanize_and_format_date() {
    assert_eq!(humanize_last_online(rustblox::model::now_unix() - 30), "just now");
    assert_eq!(humanize_last_online(rustblox::model::now_unix() - 3600), "1 hour ago");
    let d = rustblox::model::format_date(1714566896);
    assert_eq!(d, "2024-05-01");
}

#[test]
fn rows_sorting_and_labels() {
    let snap = Snapshot {
        friends: vec![
            rustblox::model::Friend { id: 2, name: "bob".into(), display_name: "Bob".into(), is_banned: false },
            rustblox::model::Friend { id: 3, name: "alice".into(), display_name: "Alice".into(), is_banned: false },
            rustblox::model::Friend { id: 4, name: "carl".into(), display_name: "Carl".into(), is_banned: false },
        ],
        presences: {
            let mut m = HashMap::new();
            m.insert(
                2,
                Presence { kind: PresenceKind::InGame, universe_id: Some(99), ..Default::default() },
            );
            m.insert(3, Presence { kind: PresenceKind::Online, ..Default::default() });
            m.insert(4, Presence { kind: PresenceKind::Offline, ..Default::default() });
            m
        },
        games: HashMap::from([(99u64, "Natural Disaster Survival".to_string())]),
        last_online: HashMap::new(),
    };
    let mut rows = build_rows(&snap);
    sort_rows(&mut rows);
    assert_eq!(rows[0].friend.display_name, "Bob"); // InGame first
    assert_eq!(rows[1].friend.display_name, "Alice");
    assert_eq!(rows[2].friend.display_name, "Carl");
    assert_eq!(rows[0].status_label(), "Playing Natural Disaster Survival");
    assert_eq!(rows[1].status_label(), "Online");
    assert_eq!(rows[2].status_label(), "Offline");
    assert!(rows[2].matches_filter("car"));
    assert!(!rows[0].matches_filter("zzz"));
}

#[test]
fn cookie_token_normalization() {
    let bare = "|abc123def";
    assert_eq!(normalize_cookie_token(bare), "|abc123def");
    let warned = "_|WARNING:-DO-NOT-SHARE-THIS--Sharing-this-will-allow-someone-to-log-in-as-you-and-to-steal-your-ROBLOX-items.|_F9A2B8C4";
    assert_eq!(normalize_cookie_token(warned), "F9A2B8C4");
    let warned2 = "_|WARNING:-DO-NOT-SHARE-THIS--Sharing-this-will-allow-someone-to-log-in-as-you-and-to-steal-your-ROBLOX-items.|F9A2B8C4";
    assert_eq!(normalize_cookie_token(warned2), "F9A2B8C4");
    let full = "Set-Cookie: .ROBLOSECURITY=F9A2B8C4; Path=/; HttpOnly; Secure";
    assert_eq!(normalize_cookie_token(full), "F9A2B8C4");
    assert_eq!(normalize_cookie_token("  \"F9A2B8C4\"  "), "F9A2B8C4");
    assert_eq!(normalize_cookie_token("   "), "");
}
