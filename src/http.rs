//! The `http` effect for JavaScript hosts: `orbital_core::runtime::http_request`'s sans-I/O driver over JSON
//! strings. The core types are the wire: asks, answers, steps and the driver itself are (de)serialized as they
//! are; this module only moves them across the boundary.

use orbital_core::runtime::http_request::{self, HeldSecrets, HttpAnswer, HttpCallDriver, HttpMethod, HttpStep, StreamFraming};
use orbital_core::Value;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;

/// Credential values a call has been answered with, kept in wasm memory between steps so the driver JSON that
/// crosses to the host carries only a handle to them, never a value. An entry ends with its call's final step.
#[derive(Default)]
struct Vault {
    next: u64,
    held: HashMap<u64, HeldSecrets>,
}

thread_local! {
    static VAULT: RefCell<Vault> = RefCell::new(Vault::default());
}

fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("parse {what}: {e}"))
}

fn out<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("serialize: {e}"))
}

#[derive(Deserialize)]
struct CallIn {
    method: HttpMethod,
    origin: String,
    path: String,
    request: Value,
    #[serde(default)]
    key: Option<String>,
}

fn progress(driver: &mut HttpCallDriver, step: &HttpStep) -> Result<String, String> {
    VAULT.with(|vault| {
        let mut vault = vault.borrow_mut();
        let finished = matches!(step, HttpStep::Done(_));
        let secrets = driver.take_secrets();
        if finished || secrets.is_empty() {
            if let Some(id) = driver.vault() {
                vault.held.remove(&id);
            }
            driver.set_vault(None);
        } else {
            let id = driver.vault().unwrap_or_else(|| {
                vault.next += 1;
                vault.next
            });
            vault.held.insert(id, secrets);
            driver.set_vault(Some(id));
        }
    });
    out(&json!({ "driver": driver, "step": step }))
}

fn revive(driver_json: &str) -> Result<HttpCallDriver, String> {
    let mut driver: HttpCallDriver = parse(driver_json, "driver_json")?;
    if let Some(id) = driver.vault() {
        if let Some(secrets) = VAULT.with(|vault| vault.borrow().held.get(&id).cloned()) {
            driver.restore_secrets(secrets);
        }
    }
    Ok(driver)
}

/// Start a call (`{ method, origin, path, request, key? }`) under the program's `egress`: `{ driver, step }`,
/// where a call that fails to parse is a finished step with `driver: null`.
pub fn http_driver_start(call_json: &str, egress_json: &str) -> Result<String, String> {
    let call: CallIn = parse(call_json, "call_json")?;
    let egress: Vec<String> = parse(egress_json, "egress_json")?;
    match http_request::parse_http_call(call.method, &call.origin, &call.path, &call.request) {
        Ok(spec) => {
            let (mut driver, step) = HttpCallDriver::start(spec, egress, call.key);
            progress(&mut driver, &step)
        }
        Err(refusal) => {
            let step = HttpCallDriver::refused(&refusal, call.key.as_deref());
            out(&json!({ "driver": null, "step": step }))
        }
    }
}

/// Answer the ask the driver is waiting on; `{ driver, step }`.
pub fn http_driver_resume(driver_json: &str, answer_json: &str) -> Result<String, String> {
    let mut driver = revive(driver_json)?;
    let answer: HttpAnswer = parse(answer_json, "answer_json")?;
    let step = driver.resume(answer);
    progress(&mut driver, &step)
}

pub fn is_public_address(ip: &str) -> Result<bool, String> {
    ip.parse().map(http_request::is_public_address).map_err(|e| format!("parse ip: {e}"))
}

/// Frames a chunk for a stream whose driver the caller does not hold: `{ events, carry }` with each event
/// carrying `key`; `framing` is `"sse"` or `"jsonLines"`.
pub fn frame_stream(framing: &str, key: Option<&str>, chunk: &str, carry: &str) -> Result<String, String> {
    let framing = parse_framing(framing)?;
    let mut carry = carry.to_string();
    let events = http_request::frame_stream(framing, key, chunk, &mut carry);
    out(&json!({ "events": events, "carry": carry }))
}

/// The events the unterminated tail in `carry` makes at end of body, for a call with no held driver.
pub fn frame_stream_finish(framing: &str, key: Option<&str>, carry: &str) -> Result<String, String> {
    let framing = parse_framing(framing)?;
    let mut carry = carry.to_string();
    out(&http_request::frame_stream_finish(framing, key, &mut carry))
}

fn parse_framing(framing: &str) -> Result<StreamFraming, String> {
    parse(&json!(framing).to_string(), "framing")
}

/// Releases the credentials the vault holds for a call that ends before its final step (cancelled, aborted).
pub fn http_driver_drop(driver_json: &str) -> Result<(), String> {
    let driver: HttpCallDriver = parse(driver_json, "driver_json")?;
    if let Some(id) = driver.vault() {
        VAULT.with(|vault| vault.borrow_mut().held.remove(&id));
    }
    Ok(())
}

/// `{ events: HttpStreamEvent[], carry: string }` (each event carries the driver's `key`); `carry` is the
/// unframed tail to hand back with the next chunk.
pub fn stream_events(driver_json: &str, chunk: &str, carry: &str) -> Result<String, String> {
    let driver: HttpCallDriver = parse(driver_json, "driver_json")?;
    let mut carry = carry.to_string();
    let events = driver.stream_events(chunk, &mut carry);
    out(&json!({ "events": events, "carry": carry }))
}

/// The events the unterminated tail left in `carry` makes at end of body.
pub fn stream_finish(driver_json: &str, carry: &str) -> Result<String, String> {
    let driver: HttpCallDriver = parse(driver_json, "driver_json")?;
    let mut carry = carry.to_string();
    out(&driver.stream_finish(&mut carry))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_of(progress: &str) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(progress).unwrap()
    }

    #[test]
    fn two_credentials_cross_the_json_boundary_without_a_value_in_the_driver() {
        let call = r#"{"method":"GET","origin":"https://api.example.com","path":"/x","request":{"auth":{"scheme":"basic","user":"USER","credential":"PASS"}}}"#;
        let started = step_of(&http_driver_start(call, r#"["https://api.example.com"]"#).unwrap());
        assert_eq!(started["step"]["ask"]["name"], "USER");
        let driver = started["driver"].to_string();

        let answer = |v: &str| format!(r#"{{"kind":"secret","result":{{"ok":"{v}"}}}}"#);
        let after_user = step_of(&http_driver_resume(&driver, &answer("alice-secret")).unwrap());
        let driver = after_user["driver"].to_string();
        assert!(!driver.contains("alice-secret"), "{driver}");
        assert_eq!(after_user["step"]["ask"]["name"], "PASS");

        let sent = http_driver_resume(&driver, &answer("pass-secret")).unwrap();
        let sent = step_of(&sent);
        assert_eq!(sent["step"]["ask"]["kind"], "http");
        let request = sent["step"]["ask"]["request"].to_string();
        assert!(request.contains("Basic YWxpY2Utc2VjcmV0OnBhc3Mtc2VjcmV0"), "{request}");
        assert!(!sent["driver"].to_string().contains("secret"), "{}", sent["driver"]);
    }

    #[test]
    fn a_call_that_does_not_parse_finishes_with_its_key_and_no_driver() {
        let call = r#"{"method":"GET","origin":"https://api.example.com","path":"nope","request":{},"key":"k"}"#;
        let out = step_of(&http_driver_start(call, "[]").unwrap());
        assert!(out["driver"].is_null());
        assert_eq!(out["step"]["result"]["error"]["key"], "k");
    }
}
