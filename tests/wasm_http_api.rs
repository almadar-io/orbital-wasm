//! The wasm-facing `http` driver over JSON strings: asks come out, answers go in, the core decides.

use orbital_wasm::http;
use serde_json::{json, Value};

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

const ORIGIN: &str = "https://api.example.com";

fn start(request: Value, egress: Value) -> Value {
    let call = json!({ "method": "GET", "origin": ORIGIN, "path": "/search", "request": request });
    parsed(&http::http_driver_start(&call.to_string(), &egress.to_string()).unwrap())
}

fn resume(progress: &Value, answer: Value) -> Value {
    parsed(&http::http_driver_resume(&progress["driver"].to_string(), &answer.to_string()).unwrap())
}

fn reply(status: u16, body: &str) -> Value {
    json!({ "kind": "http", "result": { "ok": { "status": status, "headers": [], "body": { "kind": "text", "text": body } } } })
}

#[test]
fn a_plain_get_asks_for_the_request_and_finishes_with_the_reply_payload() {
    let first = start(json!({ "query": { "q": "a b" } }), json!([ORIGIN]));
    assert_eq!(first["step"]["status"], "ask");
    assert_eq!(first["step"]["ask"]["kind"], "http");
    assert_eq!(first["step"]["ask"]["request"]["url"], "https://api.example.com/search?q=a%20b");
    let done = resume(&first, reply(404, "nope"));
    assert_eq!(done["step"]["status"], "done");
    assert_eq!(done["step"]["result"]["ok"]["status"], 404);
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
fn a_missing_credential_finishes_refused_without_an_http_ask() {
    let first = start(json!({ "auth": { "scheme": "bearer", "credential": "KEY" } }), json!([ORIGIN]));
    let done = resume(&first, json!({ "kind": "secret", "result": { "ok": null } }));
    assert_eq!(done["step"]["status"], "done");
    assert_eq!(done["step"]["result"]["error"]["kind"], "refused");
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

#[test]
fn frames_sse_events_hands_back_the_tail_and_flushes_it_at_the_end() {
    let first = parsed(&http::stream_events("sse", "data: one\n\ndata: tw", "").unwrap());
    assert_eq!(first["events"].as_array().unwrap().len(), 1);
    assert_eq!(first["events"][0]["data"], "one");
    let carry = first["carry"].as_str().unwrap().to_string();
    let second = parsed(&http::stream_events("sse", "o\n\n", &carry).unwrap());
    assert_eq!(second["events"][0]["data"], "two");
    let flushed = parsed(&http::stream_finish("jsonLines", "{\"a\":1}").unwrap());
    assert_eq!(flushed.as_array().unwrap().len(), 1);
}
