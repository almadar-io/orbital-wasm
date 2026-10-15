//! The `http` effect for JavaScript hosts: `orbital_core::runtime::http_request`'s sans-I/O driver over JSON
//! strings. The driver owns every decision (validation, credentials, OAuth, pagination, framing); this module
//! only moves its asks and answers across the boundary.

use orbital_core::embed::wire::{HttpFailureWire, HttpRequestWire, HttpResponseWire, Settled};
use orbital_core::runtime::http_request::{self, HttpAnswer, HttpAsk, HttpCallDriver, HttpStep};
use orbital_core::runtime::http_service::{HttpResponse, TransportError};
use orbital_core::Value;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("parse {what}: {e}"))
}

fn word<T: serde::de::DeserializeOwned>(word: &str, what: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(word.to_string())).map_err(|e| format!("parse {what}: {e}"))
}

fn out<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("serialize: {e}"))
}

#[derive(Deserialize)]
struct CallIn {
    method: String,
    origin: String,
    path: String,
    request: Value,
}

/// What the driver asks of the host.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum AskWire {
    Secret { name: String },
    #[serde(rename_all = "camelCase")]
    SecretSet { name: String, value: String, ttl_ms: u64 },
    Http { request: HttpRequestWire },
    HttpStream { request: HttpRequestWire, framing: Value },
}

/// What the host answers.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum AnswerWire {
    Secret { result: Settled<Option<String>, String> },
    SecretSet { result: Settled<(), String> },
    Http { result: Settled<HttpResponseWire, HttpFailureWire> },
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum StepWire {
    Ask { ask: AskWire },
    Done { result: Settled<Value, Value> },
}

fn ask_wire(ask: &HttpAsk) -> Result<AskWire, String> {
    Ok(match ask {
        HttpAsk::Secret { name } => AskWire::Secret { name: name.clone() },
        HttpAsk::SecretSet { name, value, ttl_ms } => AskWire::SecretSet { name: name.clone(), value: value.clone(), ttl_ms: *ttl_ms },
        HttpAsk::Http { request } => AskWire::Http { request: HttpRequestWire::from(request) },
        HttpAsk::HttpStream { request, framing } => AskWire::HttpStream {
            request: HttpRequestWire::from(request),
            framing: serde_json::to_value(framing).map_err(|e| format!("serialize: {e}"))?,
        },
    })
}

fn step_wire(step: &HttpStep) -> Result<StepWire, String> {
    Ok(match step {
        HttpStep::Ask(ask) => StepWire::Ask { ask: ask_wire(ask)? },
        HttpStep::Done(Ok(payload)) => StepWire::Done { result: Settled::Ok { ok: payload.clone() } },
        HttpStep::Done(Err(payload)) => StepWire::Done { result: Settled::Error { error: payload.clone() } },
    })
}

fn progress(driver: &HttpCallDriver, step: &HttpStep) -> Result<String, String> {
    out(&json!({ "driver": driver, "step": step_wire(step)? }))
}

/// Start a call (`{ method, origin, path, request }`) under the program's `egress`:
/// `{ driver, step }`, where a refused call is a finished step with `driver: null`.
pub fn http_driver_start(call_json: &str, egress_json: &str) -> Result<String, String> {
    let call: CallIn = parse(call_json, "call_json")?;
    let egress: Vec<String> = parse(egress_json, "egress_json")?;
    match http_request::parse_http_call(word(&call.method, "method")?, &call.origin, &call.path, &call.request) {
        Ok(spec) => {
            let (driver, step) = HttpCallDriver::start(spec, egress);
            progress(&driver, &step)
        }
        Err(refusal) => out(&json!({
            "driver": null,
            "step": StepWire::Done { result: Settled::Error { error: http_request::refusal_payload(&refusal) } },
        })),
    }
}

/// Answer the ask the driver is waiting on; `{ driver, step }`.
pub fn http_driver_resume(driver_json: &str, answer_json: &str) -> Result<String, String> {
    let mut driver: HttpCallDriver = parse(driver_json, "driver_json")?;
    let answer = match parse::<AnswerWire>(answer_json, "answer_json")? {
        AnswerWire::Secret { result } => HttpAnswer::Secret(result.into()),
        AnswerWire::SecretSet { result } => HttpAnswer::SecretSet(result.into()),
        AnswerWire::Http { result } => {
            let result: Result<HttpResponseWire, HttpFailureWire> = result.into();
            HttpAnswer::Http(result.map(HttpResponse::from).map_err(TransportError::from))
        }
    };
    let step = driver.resume(answer);
    progress(&driver, &step)
}

pub fn is_public_address(ip: &str) -> Result<bool, String> {
    ip.parse().map(http_request::is_public_address).map_err(|e| format!("parse ip: {e}"))
}

/// `{ events: HttpStreamEvent[], carry: string }`; `carry` is the unframed tail to hand back with the next chunk.
pub fn stream_events(framing: &str, chunk: &str, carry: &str) -> Result<String, String> {
    let mut carry = carry.to_string();
    let events = http_request::stream_events(word(framing, "framing")?, chunk, &mut carry);
    out(&json!({ "events": events, "carry": carry }))
}

/// The events the unterminated tail left in `carry` makes at end of body.
pub fn stream_finish(framing: &str, carry: &str) -> Result<String, String> {
    out(&http_request::stream_finish(word(framing, "framing")?, carry))
}
