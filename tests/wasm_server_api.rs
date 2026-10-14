//! The wasm-facing server API (`ServerApi`, what `Server` forwards to) drives a dispatch through its
//! I/O over JSON strings: start, answer what is asked, resume until done.

use orbital_wasm::api::{install_language, ServerApi};
use serde_json::{json, Value};

fn schema() -> Value {
    let emit = json!({ "emit": { "success": "SAVED", "failure": "SAVE_FAILED" } });
    json!({
        "name": "notes",
        "orbitals": [{
            "name": "Main",
            "entity": { "name": "Note", "persistence": "persistent", "collection": "notes", "fields": [{ "name": "id", "type": "string" }, { "name": "title", "type": "string" }] },
            "traits": [{
                "name": "Draft", "linkedEntity": "Note",
                "stateMachine": {
                    "states": [{ "name": "editing", "isInitial": true }, { "name": "saving" }, { "name": "saved" }, { "name": "failed" }],
                    "transitions": [
                        { "from": "editing", "to": "saving", "event": "SAVE", "effects": [["persist", "create", "Note", { "id": "d1", "title": "draft" }, emit]] },
                        { "from": "saving", "to": "saved", "event": "SAVED" },
                        { "from": "saving", "to": "failed", "event": "SAVE_FAILED" }
                    ]
                }
            }]
        }]
    })
}

fn progress(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn answer(pending: &Value, create: &dyn Fn(&Value) -> Value) -> Value {
    let request = &pending["request"];
    let result = match request["kind"].as_str().unwrap() {
        "createRow" => json!({ "kind": "row", "result": create(&request["data"]) }),
        "listRows" => json!({ "kind": "rows", "result": { "ok": [] } }),
        "readRow" => json!({ "kind": "row", "result": { "error": "missing" } }),
        other => panic!("unexpected request {other}"),
    };
    json!({ "id": pending["id"], "answer": result })
}

fn run(create: &dyn Fn(&Value) -> Value) -> (Value, ServerApi) {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    let mut step = progress(
        &api.start(
            "Main",
            &json!({ "event": "SAVE", "targetTrait": "Draft" }).to_string(),
        )
        .unwrap(),
    );
    assert_eq!(api.in_flight(), 1);
    loop {
        match step["status"].as_str().unwrap() {
            "done" => return (step["response"].clone(), api),
            "await" => {
                let replies: Vec<Value> = step["requests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| answer(p, create))
                    .collect();
                step = progress(
                    &api.resume(
                        step["flight"].as_u64().unwrap(),
                        &Value::Array(replies).to_string(),
                    )
                    .unwrap(),
                );
            }
            other => panic!("unknown status {other}"),
        }
    }
}

#[test]
fn a_dispatch_runs_to_its_response_through_the_hosts_answers() {
    let (response, api) = run(&|data| json!({ "ok": data }));
    assert_eq!(response["success"], json!(true));
    assert_eq!(response["states"]["Draft"], json!("saved"));
    assert_eq!(api.get_state("Draft", None), "saved");
    assert_eq!(api.in_flight(), 0);
}

#[test]
fn control_a_refused_write_takes_the_failure_path() {
    let (response, api) = run(&|_| json!({ "error": "disk full" }));
    assert_eq!(response["states"]["Draft"], json!("failed"));
    assert_eq!(api.get_state("Draft", None), "failed");
}

#[test]
fn a_failed_call_is_the_error_and_the_last_error() {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    let err = api
        .start("Nowhere", &json!({ "event": "SAVE" }).to_string())
        .unwrap_err();
    assert!(err.contains("Orbital not found"), "{err}");
    assert_eq!(api.last_error(), err);
    assert!(api.resume(7, "[]").is_err());
    assert!(ServerApi::new(r#"{"name":"Empty","orbitals":[]}"#).is_err());
}

#[test]
fn the_default_user_and_the_mount_are_set_by_json() {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    api.set_default_user(r#"{"id":"u1","role":"admin"}"#)
        .unwrap();
    api.mount(r#"["Draft"]"#).unwrap();
    api.remount(r#"[]"#).unwrap();
    assert!(api.set_default_user("{").is_err());
    assert!(api.mount("not json").is_err());
}

#[test]
fn a_malformed_language_bundle_is_refused_with_its_code() {
    let err = install_language("{}").unwrap_err();
    assert!(err.starts_with("ORB_LANGUAGE_DATA_INVALID"), "{err}");
    assert!(err.contains("missing `core`"), "{err}");
    assert!(install_language("not json").is_err());
}

#[test]
fn the_tick_mount_is_set_by_json_and_a_bad_list_is_refused() {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    api.set_tick_mount(r#"["Draft"]"#).unwrap();
    api.set_tick_mount("[]").unwrap();
    let err = api.set_tick_mount("not json").unwrap_err();
    assert_eq!(api.last_error(), err);
}
