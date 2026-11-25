//! Join-flow tests: reply parsing, refusal text, ticket priority, endpoint
//! and udmux extraction from every join-script dialect.

use super::request::{extract_endpoint, extract_udmux};
use super::ticket::{join_attempt_id, ticket_from_parts};
use super::*;

#[test]
fn join_response_parses() {
	let j: JoinResponse = serde_json::from_str(
		r#"{
                "jobId": "abc-123",
                "session_id": "s1",
                "ip": "10.0.0.1",
                "port": 20005,
                "characterAppearanceUrl": "https://avatar.roblox.com/x",
                "dataCenterId": 7,
                "connectionType": "RakNet"
            }"#,
	)
	.unwrap();
	assert_eq!(j.job_id, "abc-123");
	assert_eq!(j.server_addr(), Some(("10.0.0.1", 20005)));
}

#[test]
fn join_response_snake_and_camel() {
	// v1 uses snake_case keys; v2 camelCase. Both must parse.
	let j1: JoinResponse = serde_json::from_str(
		r#"{"jobId":"j","session_id":"s","ip":"h","port":1,"characterAppearanceUrl":"u"}"#,
	)
	.unwrap();
	assert_eq!(j1.session_id, "s");
	let j2: JoinResponse = serde_json::from_str(
		r#"{"jobId":"j","sessionId":"s","ip":"h","port":1,"characterAppearanceUrl":"u"}"#,
	)
	.unwrap();
	assert_eq!(j2.session_id, "s");
}

#[test]
fn appearance_parses() {
	let a: AvatarAppearance = serde_json::from_str(
            r#"{"playerAvatarType":"R15","bodyColors":{"headColorId":26,"torsoColorId":23},"assets":[{"id":1}]}"#,
        )
        .unwrap();
	assert_eq!(a.player_avatar_type, "R15");
	assert_eq!(a.body_colors.head_color_id, 26);
	assert_eq!(a.assets.len(), 1);
}

#[test]
fn join_response_tolerates_nulls() {
	// Roblox answers {"jobId":null,...} when it refuses the join; the
	// typed struct must still parse (the refusal is reported separately).
	let v: serde_json::Value = serde_json::from_str(
            r#"{"jobId":null,"status":5,"joinError":{"joinStatusCode":6,"errorCode":6,"description":"You cannot join this experience"}}"#,
        )
        .unwrap();
	let resp: JoinResponse = serde_json::from_value(v.clone()).unwrap();
	assert!(resp.job_id.is_empty());
	assert_eq!(
		join_refusal_detail(&v),
		"status 5, joinStatusCode=6, errorCode=6, \"You cannot join this experience\""
	);
}

#[test]
fn refusal_detail_v1_flat_code() {
	let v: serde_json::Value = serde_json::from_str(r#"{"jobId":null,"joinErrorCode":8}"#).unwrap();
	assert_eq!(join_refusal_detail(&v), "joinErrorCode=8");
	let empty: serde_json::Value = serde_json::json!({"jobId": null});
	assert_eq!(join_refusal_detail(&empty), "no reason given (jobId is null)");
}

#[test]
fn refusal_detail_unauthenticated_v1() {
	// The exact body an anonymous /v1/join-game request gets (repro'd
	// against the live endpoint): status 12 + flat message.
	let v: serde_json::Value = serde_json::from_str(
            r#"{"jobId":null,"status":12,"statusData":null,"joinScriptUrl":null,"authenticationUrl":null,"authenticationTicket":null,"message":"Unable to join Game 211","joinScript":null,"joinTicket":null,"queuePosition":0}"#,
        )
        .unwrap();
	assert_eq!(
            join_refusal_detail(&v),
            "status 12 (join refused — the request carried no valid authentication ticket), \"Unable to join Game 211\""
        );
}

#[test]
fn endpoint_extracted_from_join_script_variants() {
	// Classic XML-ish join script.
	let xml = r#"{"jobId":"j1","status":0,"joinScript":"<JoinScript><MachineAddress>203.0.113.7</MachineAddress><MachinePort>20005</MachinePort></JoinScript>"}"#;
	let v: serde_json::Value = serde_json::from_str(xml).unwrap();
	assert_eq!(extract_endpoint(&v), Some(("203.0.113.7".into(), 20005)));
	// JSON keys inside the blob.
	let js = r#"{"jobId":"j2","status":0,"joinScript":"{\"ip\":\"198.51.100.3\",\"port\":20005}"}"#;
	let v2: serde_json::Value = serde_json::from_str(js).unwrap();
	assert_eq!(extract_endpoint(&v2), Some(("198.51.100.3".into(), 20005)));
	// Top-level ip/port make the extractor unnecessary, but it must not
	// misfire on blobs without any endpoint.
	let none: serde_json::Value = serde_json::from_str(
		r#"{"jobId":"j3","status":0,"joinScript":"<JoinScript><JobId>abc</JobId></JoinScript>"}"#,
	)
	.unwrap();
	assert_eq!(extract_endpoint(&none), None);
	let empty: serde_json::Value = serde_json::from_str(r#"{"jobId":"j4"}"#).unwrap();
	assert_eq!(extract_endpoint(&empty), None);
}

#[test]
fn accepted_join_without_address_is_an_error() {
	// A success body without any recoverable endpoint must produce the
	// clear "no server address" error path (via server_addr being None).
	let j: JoinResponse = serde_json::from_str(r#"{"jobId":"abc"}"#).unwrap();
	assert_eq!(j.server_addr(), None);
}

#[test]
fn join_response_null_optional_fields() {
	let j: JoinResponse = serde_json::from_str(
            r#"{"jobId":"j","sessionId":null,"ip":null,"port":null,"characterAppearanceUrl":null,"connectionType":null}"#,
        )
        .unwrap();
	assert_eq!(j.job_id, "j");
	assert!(j.ip.is_empty());
	assert_eq!(j.port, 0);
}

#[test]
fn ticket_from_header_wins() {
	// The live endpoint: empty {} body, ticket in the response header.
	assert_eq!(ticket_from_parts(Some(" T-HEADER "), "{}"), Some("T-HEADER".into()));
}

#[test]
fn ticket_from_body_field_fallback() {
	assert_eq!(ticket_from_parts(None, r#"{"ticket":"T-BODY"}"#), Some("T-BODY".into()));
	assert_eq!(
		ticket_from_parts(None, r#"{"authenticationTicket":"T-ALT"}"#),
		Some("T-ALT".into())
	);
}

#[test]
fn ticket_empty_json_body_is_not_a_ticket() {
	// REGRESSION: the 200 body IS an empty object — returning it as the
	// ticket produced authenticationTicket: "{}" and join status 12.
	assert_eq!(ticket_from_parts(None, "{}"), None);
	assert_eq!(ticket_from_parts(Some("   "), "{}"), None);
	assert_eq!(ticket_from_parts(None, r#"{"ticket": null}"#), None);
	assert_eq!(ticket_from_parts(None, r#"{"ticket": ""}"#), None);
	assert_eq!(ticket_from_parts(None, ""), None);
	assert_eq!(ticket_from_parts(None, "<html>error</html>"), None);
	// Legacy dialect: bare plain-text ticket.
	assert_eq!(ticket_from_parts(None, "PLAINTICKET123"), Some("PLAINTICKET123".into()));
}

#[test]
fn extract_endpoint_from_join_script_object() {
	// Modern dialect: joinScript is a JSON object (RoZod gamejoinv1
	// schema). ServerConnections first.
	let v: serde_json::Value = serde_json::from_str(
		r#"{"jobId":"j1","status":2,"joinScript":{"MachineAddress":"203.0.113.9","ServerPort":20005,
                "ServerConnections":[{"Address":"203.0.113.9","Port":20005}]}}"#,
	)
	.unwrap();
	assert_eq!(extract_endpoint(&v), Some(("203.0.113.9".into(), 20005)));
	// Udmux layer only.
	let v2: serde_json::Value = serde_json::from_str(
        r#"{"jobId":"j2","status":2,"joinScript":{"UdmuxEndpoints":[{"Address":"198.51.100.4","Port":20019}]}}"#,
    )
    .unwrap();
	assert_eq!(extract_endpoint(&v2), Some(("198.51.100.4".into(), 20019)));
	// Flat legacy keys inside the object.
	let v3: serde_json::Value = serde_json::from_str(
		r#"{"jobId":"j3","status":2,"joinScript":{"MachineAddress":"10.1.2.3","ServerPort":20005}}"#,
	)
	.unwrap();
	assert_eq!(extract_endpoint(&v3), Some(("10.1.2.3".into(), 20005)));
	// Object without any endpoint information.
	let v4: serde_json::Value = serde_json::from_str(
		r#"{"jobId":"j4","status":2,"joinScript":{"UserId":1,"BaseUrl":"https://www.roblox.com"}}"#,
	)
	.unwrap();
	assert_eq!(extract_endpoint(&v4), None);
}

#[test]
fn extract_udmux_from_join_script_object() {
	// Field set mirrored from a real TeamTest join script (devforum
	// 4580722): UdmuxEndpoints + TokenValue + PepperId + DSR.
	let v: serde_json::Value = serde_json::from_str(
		r#"{"MachineAddress":"10.186.12.41","ServerPort":53928,"ClientPort":0,
                "ServerConnections":[{"Address":"10.186.12.41","Port":53928}],
                "UdmuxEndpoints":[{"Address":"128.116.48.33","Port":53928}],
                "DirectServerReturn":true,"PepperId":1776353748,
                "TokenValue":"SECRET-TOKEN","NetStackPort":54115,"NetStackTokenValue":""}"#,
	)
	.unwrap();
	let js = v.as_object().unwrap();
	let (host, token, pepper) = extract_udmux(js);
	assert_eq!(host.as_deref(), Some("128.116.48.33:53928"));
	assert_eq!(token.as_deref(), Some("SECRET-TOKEN"));
	assert_eq!(pepper, Some(1776353748));
	// Legacy token field name, no endpoints.
	let v2: serde_json::Value =
		serde_json::from_str(r#"{"UdmuxToken":"LEGACY","PepperId":7}"#).unwrap();
	let (host2, token2, pepper2) = extract_udmux(v2.as_object().unwrap());
	assert_eq!(host2, None);
	assert_eq!(token2.as_deref(), Some("LEGACY"));
	assert_eq!(pepper2, Some(7));
	// Nothing at all.
	let v3: serde_json::Value = serde_json::from_str(r#"{"UserId":1}"#).unwrap();
	let (host3, token3, pepper3) = extract_udmux(v3.as_object().unwrap());
	assert_eq!(host3, None);
	assert_eq!(token3, None);
	assert_eq!(pepper3, None);
}

#[test]
fn netstack_fields_extracted_from_join_script() {
	// Field set mirrored from the live session documented in the udmux
	// reverse-engineering report (place 6243699076): NetStackPort +
	// NetStackTokenValue + TokenGenAlgorithm + early-pubkey material.
	let v: serde_json::Value = serde_json::from_str(
		r#"{"jobId":"j1","status":2,"ip":"10.30.2.71","port":63646,
                "joinScript":{"MachineAddress":"10.30.2.71","ServerPort":63646,
                "UdmuxEndpoints":[{"Address":"128.116.13.34","Port":63646}],
                "TokenValue":"AAAAAAAAAAAAAAAAAAAAAA==","PepperId":1794420914,
                "TokenGenAlgorithm":1,"NetStackPort":64469,
                "NetStackConfig":"{\"port\":64469,\"mtuSize\":1200}",
                "NetStackTokenValue":"BBBBBBBBBBBBBBBBBBBBBB==",
                "EphemeralEarlyPubKey":"CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
                "ClientPublicKeyData":"{\"applications\":{\"RakNetEarlyPublicKey\":{}}}"}}"#,
	)
	.unwrap();
	let resp: JoinResponse = serde_json::from_value(v.clone()).unwrap();
	// Top-level defaults first (the fields live in the joinScript).
	assert_eq!(resp.netstack_port, 0);
	let js = v.get("joinScript").unwrap().as_object().unwrap();
	assert_eq!(extract_udmux(js).0.as_deref(), Some("128.116.13.34:63646"));
	assert_eq!(js.get("NetStackPort").and_then(|p| p.as_u64()), Some(64469));
	assert_eq!(js.get("TokenGenAlgorithm").and_then(|p| p.as_u64()), Some(1));
	// The raw NetStackConfig parses through the best-effort parser.
	let cfg = js.get("NetStackConfig").unwrap().as_str().unwrap();
	let pairs = crate::net::udmux::parse_netstack_config(cfg);
	assert!(pairs.contains(&("port".to_string(), "64469".to_string())));
	assert!(pairs.contains(&("mtuSize".to_string(), "1200".to_string())));
}

#[test]
fn refusal_status_hints() {
	let mk = |status: i64| {
		let v = serde_json::json!({"jobId": null, "status": status});
		join_refusal_detail(&v)
	};
	assert!(mk(12).starts_with("status 12 (join refused"));
	assert!(mk(1).starts_with("status 1 (queued"));
	assert!(mk(6).starts_with("status 6 (the server is full"));
	assert!(mk(14).starts_with("status 14 (invalid, private"));
	assert_eq!(mk(99), "status 99");
}

#[test]
fn join_attempt_id_is_uuid_shaped() {
	let id = join_attempt_id();
	assert_eq!(id.len(), 36);
	assert_eq!(id.matches('-').count(), 4);
	assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
	// Version 4 nibble and variant bits.
	assert_eq!(id.as_bytes()[14], b'4');
	assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
	// Two calls in a row must not collide.
	assert_ne!(join_attempt_id(), join_attempt_id());
}
