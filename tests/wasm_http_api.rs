//! The wasm-facing `http` driver over JSON strings: asks come out, answers go in, the core decides.

use orbital_wasm::http;
use serde_json::{json, Value};

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

const ORIGIN: &str = "https://api.example.com";

#[test]
fn dispatch_clock_metadata_survives_driver_round_trips_without_becoming_a_request_option() {
    let call = json!({"method":"GET","origin":ORIGIN,"path":"/search","request":{},"nowMs":1440938160000.0});
    let progress = parsed(&http::http_driver_start(&call.to_string(), &json!([ORIGIN]).to_string()).unwrap());
    assert_eq!(progress["driver"]["nowMs"], json!(1440938160000.0));
    let response = resume(&progress, reply(200, "ok"));
    assert_eq!(response["driver"]["nowMs"], json!(1440938160000.0));
    let unstamped = start(json!({}), json!([ORIGIN]));
    assert!(unstamped["driver"]["nowMs"].is_null());
    let authored = start(json!({"nowMs":1440938160000.0}), json!([ORIGIN]));
    assert_eq!(authored["step"]["result"]["error"]["kind"], "refused");
}

fn start(request: Value, egress: Value) -> Value {
    start_keyed(request, egress, Value::Null)
}

fn start_keyed(request: Value, egress: Value, key: Value) -> Value {
    let call = json!({ "method": "GET", "origin": ORIGIN, "path": "/search", "request": request, "key": key });
    parsed(&http::http_driver_start(&call.to_string(), &egress.to_string()).unwrap())
}

fn resume(progress: &Value, answer: Value) -> Value {
    parsed(&http::http_driver_resume(&progress["driver"].to_string(), &answer.to_string()).unwrap())
}

fn reply(status: u16, body: &str) -> Value {
    json!({ "kind": "http", "result": { "ok": { "status": status, "headers": [], "body": { "kind": "text", "text": body } } } })
}

#[test]
fn jwt_bearer_round_trips_signs_and_caches_without_serializing_credentials() {
    for (alg, pem) in [
        ("RS256", include_str!("fixtures/rs256-pkcs8.pem")),
        ("ES256", include_str!("fixtures/es256-pkcs8.pem")),
    ] {
        let credential = json!({"client_email":"service@example.com","private_key":pem}).to_string();
        let first = start_keyed(json!({"auth":{
            "scheme":"jwt-bearer","key":{"credential":"ACCOUNT","scope":"user"},"alg":alg,
            "claims":{"scope":"calendar","aud":"https://auth.example.com/token","iat":1000,"exp":4600},
            "tokenUrl":"https://auth.example.com/token","expiresMarginMs":300000,
            "storeAs":{"name":"google:token","scope":"user"}
        }}), json!([ORIGIN,"https://auth.example.com"]), json!("job"));
        assert_eq!(first["step"]["ask"], json!({"kind":"secret","name":"google:token","scope":"user"}));
        let key = resume(&first, json!({"kind":"secret","result":{"ok":null}}));
        assert_eq!(key["step"]["ask"], json!({"kind":"secret","name":"ACCOUNT","scope":"user"}));
        let grant = resume(&key, json!({"kind":"secret","result":{"ok":credential}}));
        assert_eq!(grant["step"]["ask"]["kind"], "http");
        assert_eq!(grant["step"]["ask"]["request"]["method"], "POST");
        assert_eq!(grant["step"]["ask"]["request"]["url"], "https://auth.example.com/token");
        let serialized = grant["driver"].to_string();
        assert!(!serialized.contains("service@example.com"));
        assert!(!serialized.contains("PRIVATE KEY"));
        let stored = resume(&grant, reply(200, r#"{"access_token":"T1","expires_in":3600}"#));
        assert_eq!(stored["step"]["ask"], json!({"kind":"secretSet","name":"google:token","value":"T1","ttlMs":3300000,"scope":"user"}));
        let sent = resume(&stored, json!({"kind":"secretSet","result":{"ok":null}}));
        assert!(sent["step"]["ask"]["request"]["headers"].as_array().unwrap().contains(&json!(["Authorization","Bearer T1"])));
        let done = resume(&sent, reply(200, "ok"));
        assert_eq!(done["step"]["result"]["ok"]["key"], "job");
        let missing = resume(&key, json!({"kind":"secret","result":{"ok":null}}));
        assert_eq!(missing["step"]["result"]["error"]["credential"], "ACCOUNT");
        assert_eq!(missing["step"]["result"]["error"]["kind"], "unconfigured");
        assert_eq!(missing["step"]["result"]["error"]["key"], "job");
    }
}

#[test]
fn same_named_app_and_user_credentials_survive_wasm_vault_round_trips() {
    let first = start(json!({"auth":{"scheme":"basic",
        "user":{"credential":"SAME","scope":"app"},
        "credential":{"credential":"SAME","scope":"user"}}}), json!([ORIGIN]));
    assert_eq!(first["step"]["ask"], json!({"kind":"secret","name":"SAME"}));
    let second = resume(&first, json!({"kind":"secret","result":{"ok":"APP"}}));
    assert_eq!(second["step"]["ask"], json!({"kind":"secret","name":"SAME","scope":"user"}));
    let sent = resume(&second, json!({"kind":"secret","result":{"ok":"USER"}}));
    assert!(sent["step"]["ask"]["request"]["headers"].as_array().unwrap().contains(&json!(["Authorization","Basic QVBQOlVTRVI="])));
}

#[test]
fn a_plain_get_asks_for_the_request_and_finishes_with_the_reply_payload() {
    let first = start(json!({ "query": { "q": "a b" } }), json!([ORIGIN]));
    assert_eq!(first["step"]["kind"], "ask");
    assert_eq!(first["step"]["ask"]["kind"], "http");
    assert_eq!(first["step"]["ask"]["request"]["url"], "https://api.example.com/search?q=a%20b");
    let done = resume(&first, reply(404, "nope"));
    assert_eq!(done["step"]["kind"], "done");
    assert_eq!(done["step"]["result"]["ok"]["status"].as_f64(), Some(404.0));
    assert_eq!(done["step"]["result"]["ok"]["body"], "nope");
}

#[test]
fn a_bearer_credential_is_asked_before_the_request_and_sent_as_a_header() {
    let first = start(json!({ "auth": { "scheme": "bearer", "credential": "KEY" } }), json!([ORIGIN]));
    assert_eq!(first["step"]["ask"], json!({ "kind": "secret", "name": "KEY" }));
    let second = resume(&first, json!({ "kind": "secret", "result": { "ok": "tok" } }));
    assert!(second["step"]["ask"]["request"]["headers"].as_array().unwrap().contains(&json!(["Authorization", "Bearer tok"])));
}

#[test]
fn a_missing_credential_finishes_unconfigured_naming_it_without_an_http_ask() {
    let first = start(json!({ "auth": { "scheme": "bearer", "credential": "KEY" } }), json!([ORIGIN]));
    let done = resume(&first, json!({ "kind": "secret", "result": { "ok": null } }));
    assert_eq!(done["step"]["kind"], "done");
    assert_eq!(done["step"]["result"]["error"]["kind"], "unconfigured");
    assert_eq!(done["step"]["result"]["error"]["credential"], "KEY");
}

#[test]
fn a_user_scoped_slot_asks_with_its_scope_and_an_app_slot_keeps_the_old_wire() {
    let first = start(
        json!({ "auth": { "scheme": "bearer", "credential": { "credential": "SESSION", "scope": "user" } } }),
        json!([ORIGIN]),
    );
    assert_eq!(first["step"]["ask"], json!({ "kind": "secret", "name": "SESSION", "scope": "user" }));
}

#[test]
fn a_multipart_call_asks_for_random_bytes_and_a_retry_carries_its_delay() {
    let request = json!({ "multipart": { "kind": "related", "parts": [{ "text": "x" }] },
        "retry": { "max": 1, "on": ["5xx"], "backoffMs": 40 } });
    let refused = start(request.clone(), json!([ORIGIN]));
    assert_eq!(refused["step"]["result"]["error"]["kind"], "refused");
    assert_eq!(refused["driver"], Value::Null);
    let call = json!({"method":"POST","origin":ORIGIN,"path":"/upload","request":request});
    let first = parsed(&http::http_driver_start(&call.to_string(), &json!([ORIGIN]).to_string()).unwrap());
    assert_eq!(first["step"]["ask"], json!({ "kind": "random", "bytes": 16 }));
    let sent = resume(&first, json!({ "kind": "random", "result": { "ok": "AAECAwQFBgcICQoLDA0ODw==" } }));
    assert_eq!(sent["step"]["ask"]["kind"], "http");
    assert!(sent["step"]["ask"].get("delayMs").is_none());
    let retried = resume(&sent, reply(503, "busy"));
    assert_eq!(retried["step"]["ask"]["delayMs"], 40);
    assert_eq!(retried["step"]["ask"]["request"], sent["step"]["ask"]["request"]);
    let bad = http::http_driver_resume(&sent["driver"].to_string(), r#"{"kind":"random","result":{"ok":"!!"}}"#);
    assert!(bad.is_err(), "undecodable random bytes are an error, not a call outcome");
}

#[test]
fn an_origin_outside_egress_is_refused() {
    let done = start(json!({}), json!([]));
    assert_eq!(done["step"]["result"]["error"]["kind"], "refused");
}

#[test]
fn a_timeout_finishes_as_a_timeout_failure() {
    let first = start(json!({}), json!([ORIGIN]));
    let done = resume(&first, json!({ "kind": "http", "result": { "error": { "kind": "timedOut" } } }));
    assert_eq!(done["step"]["result"]["error"]["kind"], "timeout");
}

#[test]
fn a_host_refusal_finishes_as_refused() {
    let first = start(json!({}), json!([ORIGIN]));
    let done = resume(&first, json!({ "kind": "http", "result": { "error": { "kind": "refused", "message": "private address" } } }));
    assert_eq!(done["step"]["result"]["error"]["kind"], "refused");
}

#[test]
fn malformed_arguments_are_errors_not_refusals() {
    assert!(http::http_driver_start("{", "[]").is_err());
    assert!(http::http_driver_start(&json!({ "method": "TRACE", "origin": ORIGIN, "path": "/", "request": {} }).to_string(), "[]").is_err());
    assert!(http::http_driver_resume("{}", "{}").is_err());
}

#[test]
fn classifies_addresses() {
    assert!(http::is_public_address("93.184.216.34").unwrap());
    for private in ["127.0.0.1", "10.0.0.1", "169.254.169.254", "0.0.0.0", "::1", "fd00::1"] {
        assert!(!http::is_public_address(private).unwrap(), "{private}");
    }
    assert!(http::is_public_address("not an ip").is_err());
}

fn streaming(framing: &str) -> String {
    start_keyed(json!({ "stream": framing }), json!([ORIGIN]), json!("k1"))["driver"].to_string()
}

#[test]
fn frames_sse_events_with_the_drivers_key_hands_back_the_tail_and_flushes_it_at_the_end() {
    let driver = streaming("sse");
    let first = parsed(&http::stream_events(&driver, "data: one\n\ndata: tw", "").unwrap());
    assert_eq!(first["events"].as_array().unwrap().len(), 1);
    assert_eq!(first["events"][0]["data"], "one");
    assert_eq!(first["events"][0]["key"], "k1");
    let carry = first["carry"].as_str().unwrap().to_string();
    let second = parsed(&http::stream_events(&driver, "o\n\n", &carry).unwrap());
    assert_eq!(second["events"][0]["data"], "two");
    let lines = streaming("json-lines");
    let flushed = parsed(&http::stream_finish(&lines, "{\"a\":1}").unwrap());
    assert_eq!(flushed.as_array().unwrap().len(), 1);
    assert_eq!(flushed[0]["key"], "k1");
}

#[test]
fn a_streaming_call_asks_for_a_stream() {
    let first = parsed(&streaming_progress());
    assert_eq!(first["step"]["ask"]["kind"], "httpStream");
    assert_eq!(first["step"]["ask"]["framing"], "sse");
}

fn streaming_progress() -> String {
    let call = json!({ "method": "GET", "origin": ORIGIN, "path": "/s", "request": { "stream": "sse" } });
    http::http_driver_start(&call.to_string(), &json!([ORIGIN]).to_string()).unwrap()
}

#[test]
fn the_key_is_echoed_in_replies_and_refusals_and_null_when_unkeyed() {
    let keyed = start_keyed(json!({}), json!([ORIGIN]), json!("k1"));
    let done = resume(&keyed, reply(200, "ok"));
    assert_eq!(done["step"]["result"]["ok"]["key"], "k1");
    let refused = start_keyed(json!({}), json!([]), json!("k1"));
    assert_eq!(refused["step"]["result"]["error"]["key"], "k1");
    let unkeyed = resume(&start(json!({}), json!([ORIGIN])), reply(200, "ok"));
    assert_eq!(unkeyed["step"]["result"]["ok"]["key"], Value::Null);
}

#[test]
fn a_call_that_fails_to_parse_is_refused_with_no_driver() {
    let done = start(json!({ "bogus": 1 }), json!([ORIGIN]));
    assert_eq!(done["driver"], Value::Null);
    assert_eq!(done["step"]["result"]["error"]["kind"], "refused");
}

#[test]
fn frame_stream_frames_with_the_asks_key_without_a_driver() {
    let first = parsed(&http::frame_stream("sse", Some("k9"), "data: one\n\ndata: tw", "").unwrap());
    assert_eq!(first["events"].as_array().unwrap().len(), 1);
    assert_eq!(first["events"][0]["data"], "one");
    assert_eq!(first["events"][0]["key"], "k9");
    let carry = first["carry"].as_str().unwrap().to_string();
    let second = parsed(&http::frame_stream("sse", Some("k9"), "o\n\n", &carry).unwrap());
    assert_eq!(second["events"][0]["data"], "two");
    let lines = parsed(&http::frame_stream("jsonLines", Some("k9"), "{\"a\":1}\n{\"b\"", "").unwrap());
    assert_eq!(lines["events"].as_array().unwrap().len(), 1);
    let flushed = parsed(&http::frame_stream_finish("jsonLines", Some("k9"), lines["carry"].as_str().unwrap()).unwrap());
    assert_eq!(flushed.as_array().unwrap().len(), 1, "the unterminated tail is flushed as its own event");
    assert_eq!(flushed[0]["key"], "k9");
    assert!(http::frame_stream_finish("jsonLines", Some("k9"), "").unwrap().starts_with("[]"), "an empty tail yields nothing");
    let tail = parsed(&http::frame_stream_finish("jsonLines", Some("k9"), "{\"a\":1}").unwrap());
    assert_eq!(tail[0]["key"], "k9");
}

#[test]
fn frame_stream_unkeyed_events_carry_a_null_key_and_an_unknown_framing_is_an_error() {
    let unkeyed = parsed(&http::frame_stream("sse", None, "data: x\n\n", "").unwrap());
    assert_eq!(unkeyed["events"][0]["key"], Value::Null);
    assert!(http::frame_stream("xml", None, "a", "").is_err());
    assert!(http::frame_stream_finish("xml", None, "").is_err());
}

#[test]
fn dropping_a_driver_releases_its_held_credentials() {
    let call = json!({ "method": "GET", "origin": ORIGIN, "path": "/x", "request": { "auth": { "scheme": "basic", "user": "USER", "credential": "PASS" } } });
    let started = parsed(&http::http_driver_start(&call.to_string(), &json!([ORIGIN]).to_string()).unwrap());
    let after_user = resume(&started, json!({ "kind": "secret", "result": { "ok": "alice" } }));
    let driver = after_user["driver"].to_string();
    http::http_driver_drop(&driver).unwrap();
    let last = resume(&after_user, json!({ "kind": "secret", "result": { "ok": "pw" } }));
    assert_eq!(last["step"]["kind"], "ask");
    assert!(!last["step"]["ask"]["request"]["headers"].to_string().contains("YWxpY2U6"), "the dropped user value is gone from the vault");
    assert!(http::http_driver_drop("{").is_err());
    http::http_driver_drop(&started["driver"].to_string()).unwrap();
}

mod rotate {
// `token-exchange` `rotate` through the wasm-facing JSON driver: the rotation ask carries the replacement token,
// the serialized driver never does, and the access token is cached only after the rotation is answered.

    use orbital_wasm::http;
    use serde_json::{json, Value};

    const ORIGIN: &str = "https://api.example.com";
    const AUTH: &str = "https://auth.example.com";

    fn parsed(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn resume(progress: &Value, answer: Value) -> Value {
        parsed(&http::http_driver_resume(&progress["driver"].to_string(), &answer.to_string()).unwrap())
    }

    fn reply(status: u16, body: &str) -> Value {
        json!({ "kind": "http", "result": { "ok": { "status": status, "headers": [], "body": { "kind": "text", "text": body } } } })
    }

    fn start(extra: Value) -> Value {
        let mut auth = json!({
            "scheme": "token-exchange", "tokenUrl": format!("{AUTH}/token"), "grant": "refresh_token",
            "body": { "form": { "refresh_token": { "credential": "RT" } } },
            "accessField": "id_token", "storeAs": { "name": "ID", "scope": "app" },
            "rotate": { "field": "refresh_token", "credential": "RT" }
        });
        for (k, v) in extra.as_object().unwrap() {
            if v.is_null() {
                auth.as_object_mut().unwrap().remove(k);
            } else {
                auth[k] = v.clone();
            }
        }
        let call = json!({ "method": "GET", "origin": ORIGIN, "path": "/x", "request": { "auth": auth }, "key": "k" });
        parsed(&http::http_driver_start(&call.to_string(), &json!([ORIGIN, AUTH]).to_string()).unwrap())
    }

    fn at_grant_reply() -> Value {
        let first = start(json!({}));
        assert_eq!(first["step"]["ask"], json!({"kind":"secret","name":"ID"}));
        let rt = resume(&first, json!({"kind":"secret","result":{"ok":null}}));
        assert_eq!(rt["step"]["ask"], json!({"kind":"secret","name":"RT"}));
        let grant = resume(&rt, json!({"kind":"secret","result":{"ok":"refresh-1"}}));
        assert_eq!(grant["step"]["ask"]["kind"], "http");
        grant
    }

    #[test]
    fn a_rotation_is_asked_before_the_cache_write_and_the_driver_json_holds_no_token() {
        let grant = at_grant_reply();
        let rotate = resume(&grant, reply(200, r#"{"id_token":"ID-1","expires_in":3600,"refresh_token":"refresh-2"}"#));
        assert_eq!(rotate["step"]["ask"], json!({"kind":"secretRotate","name":"RT","value":"refresh-2"}));
        let driver = rotate["driver"].to_string();
        assert!(!driver.contains("refresh-1") && !driver.contains("refresh-2") && !driver.contains("ID-1"), "{driver}");
        let stored = resume(&rotate, json!({"kind":"secretRotate","result":{"ok":null}}));
        assert_eq!(stored["step"]["ask"]["kind"], "secretSet");
        assert_eq!(stored["step"]["ask"]["name"], "ID");
        assert_eq!(stored["step"]["ask"]["value"], "ID-1");
        let request = resume(&stored, json!({"kind":"secretSet","result":{"ok":null}}));
        assert_eq!(request["step"]["ask"]["kind"], "http");
        assert_eq!(request["step"]["ask"]["request"]["url"], format!("{ORIGIN}/x"));
    }

    #[test]
    fn a_failed_rotation_finishes_in_the_grant_phase_without_a_cache_write_or_request() {
        let grant = at_grant_reply();
        let rotate = resume(&grant, reply(200, r#"{"id_token":"ID-1","expires_in":3600,"refresh_token":"refresh-2"}"#));
        let done = resume(&rotate, json!({"kind":"secretRotate","result":{"error":"store down"}}));
        let error = &done["step"]["result"]["error"];
        assert_eq!(error["kind"], "failed");
        assert_eq!(error["phase"], "grant");
        assert_eq!(error["key"], "k");
        assert!(!done.to_string().contains("refresh-2"));
    }

    #[test]
    fn a_rejected_grant_reports_its_status_and_body_and_never_rotates() {
        let grant = at_grant_reply();
        let done = resume(&grant, reply(400, r#"{"error":"INVALID_REFRESH_TOKEN"}"#));
        let error = &done["step"]["result"]["error"];
        assert_eq!(error["phase"], "grant");
        assert_eq!(error["grantStatus"].as_f64(), Some(400.0));
        assert_eq!(error["grantBody"], r#"{"error":"INVALID_REFRESH_TOKEN"}"#);
    }

    #[test]
    fn rotate_without_a_refresh_grant_is_refused_at_start() {
        let done = start(json!({ "grant": null }));
        assert_eq!(done["step"]["result"]["error"]["kind"], "refused");
    }

}

mod oauth_connect {
    //! An `oauth-client` connect across the JSON boundary: the principal rides the call, the taken grant and
    //! the PKCE verifier stay in the vault, and a driver whose vault entry is gone fails closed.
    use super::*;

    const ISSUER: &str = "https://idp.example.com";
    const META: &str = r#"{"issuer":"https://idp.example.com","authorization_endpoint":"https://idp.example.com/authorize","token_endpoint":"https://idp.example.com/token"}"#;

    fn start_connect(principal: Value) -> Value {
        let client = json!({"tag":"google","mode":"real","issuer":ISSUER,"clientId":"CID","clientSecret":"CSEC","clientAuth":"post"});
        let request = json!({"auth":{"scheme":"oauth-client","op":"connect","client":client,"provider":"google","code":"C1","state":"S1"}});
        let call = json!({"method":"POST","origin":"oauth-endpoint","path":"token","request":request,"key":"k","principal":principal});
        parsed(&http::http_driver_start(&call.to_string(), &json!([ISSUER]).to_string()).unwrap())
    }

    fn secret(value: &str) -> Value {
        json!({"kind":"secret","result":{"ok":value}})
    }

    fn oauth(reply: Value) -> Value {
        json!({"kind":"oauth","result":{"ok":reply}})
    }

    fn at_take() -> Value {
        let started = start_connect(json!("u1"));
        assert_eq!(started["step"]["ask"]["name"], "CID");
        let secret_ask = resume(&started, secret("client-id-value"));
        assert_eq!(secret_ask["step"]["ask"]["name"], "CSEC");
        let available = resume(&secret_ask, secret("client-secret-value"));
        assert_eq!(available["step"]["ask"]["kind"], "oauth");
        assert_eq!(available["step"]["ask"]["ask"]["ask"], "connectionAvailable");
        resume(&available, oauth(json!({"reply":"flag","value":true})))
    }

    #[test]
    fn the_flow_asks_availability_then_the_take_and_keeps_every_secret_in_the_vault() {
        let take = at_take();
        assert_eq!(take["step"]["ask"]["ask"], json!({"ask":"pendingTake","state":"S1"}));
        let grant = json!({"provider":"google","redirectUri":"https://app/cb","verifier":"VERIFIER-VALUE","subject":"u1"});
        let metadata = resume(&take, oauth(json!({"reply":"taken","taken":{"found":"grant","grant":grant}})));
        assert_eq!(metadata["step"]["ask"]["ask"], json!({"ask":"metadataGet","issuer":ISSUER}));
        for progress in [&take, &metadata] {
            let serialized = progress["driver"].to_string();
            for secret in ["client-id-value", "client-secret-value", "VERIFIER-VALUE"] {
                assert!(!serialized.contains(secret), "{secret} crossed the boundary");
            }
        }
        let meta: Value = serde_json::from_str(META).unwrap();
        let token = resume(&metadata, oauth(json!({"reply":"metadata","metadata":meta})));
        let request = &token["step"]["ask"]["request"];
        assert_eq!(request["url"], "https://idp.example.com/token");
        assert!(request["body"]["text"].as_str().unwrap().contains("code_verifier=VERIFIER-VALUE"));
        assert!(!token["driver"].to_string().contains("VERIFIER-VALUE"));
    }

    #[test]
    fn a_provider_mismatch_after_the_take_sends_nothing() {
        let take = at_take();
        let grant = json!({"provider":"github","redirectUri":"https://app/cb","verifier":"V","subject":"u1"});
        let done = resume(&take, oauth(json!({"reply":"taken","taken":{"found":"grant","grant":grant}})));
        let error = &done["step"]["result"]["error"];
        assert_eq!((error["kind"].as_str(), error["reason"].as_str()), (Some("oauth"), Some("provider-mismatch")));
        assert_eq!(error["key"], "k");
    }

    #[test]
    fn a_connect_without_a_signed_in_principal_fails_before_the_store_is_asked() {
        let started = start_connect(Value::Null);
        let secret_ask = resume(&started, secret("id"));
        let done = resume(&secret_ask, secret("secret"));
        assert_eq!(done["step"]["result"]["error"]["reason"], "not-signed-in");
    }

    #[test]
    fn a_driver_whose_vault_entry_is_gone_fails_closed() {
        let take = at_take();
        let grant = json!({"provider":"google","redirectUri":"https://app/cb","verifier":"V","subject":"u1"});
        let metadata = resume(&take, oauth(json!({"reply":"taken","taken":{"found":"grant","grant":grant}})));
        http::http_driver_drop(&metadata["driver"].to_string()).unwrap();
        let meta: Value = serde_json::from_str(META).unwrap();
        let done = resume(&metadata, oauth(json!({"reply":"metadata","metadata":meta})));
        assert_eq!(done["step"]["kind"], "done", "no request may be sent without the held grant: {done}");
        assert!(done["step"]["result"]["error"].is_object());
    }
}

