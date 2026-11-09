//! Parser and helper tests. Fixture shapes match the live Roblox API
//! responses captured from the official endpoints.

use rustblox::model::{
    build_rows, clean_last_location, humanize_last_online, parse_iso8601_utc, sort_rows, Presence, PresenceKind,
    Snapshot,
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

/// The CURRENT shape of the friends endpoint (Oct 2024 API change): user
/// fields were removed and come back as empty strings/nulls.
const FRIENDS_JSON_NAMELESS: &str = r#"{"data":[
  {"id":2,"name":"","displayName":"","created":"2020-01-01T00:00:00Z","isBanned":null,"description":""},
  {"id":3,"name":"","displayName":"","created":"2020-01-01T00:00:00Z","isBanned":null,"description":""}
],"nextPageCursor":null}"#;

const USERS_BATCH_JSON: &str = r#"{"data":[
  {"hasVerifiedBadge":true,"id":156,"name":"builderman","displayName":"builderman"},
  {"hasVerifiedBadge":false,"id":2,"name":"bob","displayName":"Bobby","isBanned":false}
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
fn parses_friends_response_nameless() {
    // Roblox removed name/displayName from the friends endpoint: the parse
    // must still succeed with empty names (ids drive avatars/presence).
    let f: rustblox::roblox::types::FriendsResponse = serde_json::from_str(FRIENDS_JSON_NAMELESS).unwrap();
    assert_eq!(f.data.len(), 2);
    assert_eq!(f.data[0].name, "");
    assert_eq!(f.data[0].display_name, Some("".to_string()));
    let _ = &f.data[0].id;
}

#[test]
fn parses_users_batch_response() {
    let u: rustblox::roblox::types::UsersBatchResponse = serde_json::from_str(USERS_BATCH_JSON).unwrap();
    assert_eq!(u.data.len(), 2);
    let first = u.data.iter().find(|e| e.id == 156).unwrap();
    assert_eq!(first.name, "builderman");
    assert_eq!(first.display_name.as_deref(), Some("builderman"));
    let second = u.data.iter().find(|e| e.id == 2).unwrap();
    assert_eq!(second.display_name.as_deref(), Some("Bobby"));
    assert_eq!(second.is_banned, Some(false));
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
}

#[test]
fn clean_last_location_variants() {
    assert_eq!(clean_last_location("Website"), None);
    assert_eq!(clean_last_location("Studio"), None);
    assert_eq!(clean_last_location(""), None);
    assert_eq!(clean_last_location("In Game"), None);
    assert_eq!(clean_last_location("Playing: Blox Fruits").as_deref(), Some("Blox Fruits"));
    assert_eq!(clean_last_location("Creating Natural Disaster Survival").as_deref(), Some("Natural Disaster Survival"));
    assert_eq!(clean_last_location("  Desolate Island  ").as_deref(), Some("Desolate Island"));
}

#[test]
fn ingame_label_falls_back_to_last_location() {
    // Universe missing from the games response -> lastLocation fallback.
    let row = rustblox::model::FriendRow {
        friend: rustblox::model::Friend { id: 9, name: "x".into(), display_name: "X".into(), is_banned: false },
        presence: Presence {
            kind: PresenceKind::InGame,
            universe_id: Some(77),
            last_location: "Playing: License Plate Generator".into(),
            ..Default::default()
        },
        game_name: None,
        last_online: None,
    };
    assert_eq!(row.status_label(), "Playing License Plate Generator");

    // Nothing usable -> plain "In game".
    let row = rustblox::model::FriendRow {
        presence: Presence { kind: PresenceKind::InGame, last_location: "Website".into(), ..Default::default() },
        ..row
    };
    assert_eq!(row.status_label(), "In game");

    // Game name from the games API wins over lastLocation.
    let row = rustblox::model::FriendRow {
        game_name: Some("Real Title".into()),
        ..row
    };
    assert_eq!(row.status_label(), "Playing Real Title");

    // Studio with a place name.
    let row = rustblox::model::FriendRow {
        presence: Presence { kind: PresenceKind::Studio, last_location: "Creating My Place".into(), ..Default::default() },
        game_name: None,
        ..row
    };
    assert_eq!(row.status_label(), "In Studio: My Place");
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

#[test]
fn parses_user_search_response() {
    // Shape of GET users.roblox.com/v1/users/search?keyword=builder
    let json = r#"{"data":[
        {"previousUsernames":["BuilderGuy"],"id":156,"name":"builderman","displayName":"Builderman"},
        {"previousUsernames":[],"id":900,"name":"build2","displayName":""}
    ]}"#;
    let sr: rustblox::roblox::types::UserSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(sr.data.len(), 2);
    assert_eq!(sr.data[0].id, 156);
    assert_eq!(sr.data[0].name, "builderman");
    assert_eq!(sr.data[0].display_name.as_deref(), Some("Builderman"));
    assert_eq!(sr.data[1].display_name.as_deref(), Some(""));
    // (search_users() maps empty display names to "" — see UserEntry fallback.)
}

#[test]
fn percent_encoding() {
    use rustblox::roblox::social::percent_encode;
    assert_eq!(percent_encode("builder"), "builder");
    assert_eq!(percent_encode("Cool Guy 99"), "Cool%20Guy%2099");
    assert_eq!(percent_encode("ник"), "%D0%BD%D0%B8%D0%BA");
    assert_eq!(percent_encode("a+b&c=d/e?~_.-"), "a%2Bb%26c%3Dd%2Fe%3F~_.-");
}

#[test]
fn user_entry_display_fallback() {
    use rustblox::model::UserEntry;
    let e = UserEntry { id: 1, name: "bob".into(), display_name: String::new() };
    assert_eq!(e.display_or_name(), "bob");
    let e2 = UserEntry { id: 2, name: "bob2".into(), display_name: "Bobby".into() };
    assert_eq!(e2.display_or_name(), "Bobby");
}
