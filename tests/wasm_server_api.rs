//! The wasm-facing server API (`ServerApi`, what `Server` forwards to) drives a dispatch through its
//! I/O over JSON strings: start, answer what is asked, resume until done.

use orbital_core::runtime::entity_source::{EntitySource, InMemoryEntityCache};
use orbital_wasm::api::{install_language, ServerApi};
use orbital_wasm::stored::{StoreCapabilities, StoredServer};
use std::sync::Arc;
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

fn answer(pending: &Value, create: &dyn Fn(&Value) -> Value, read: &dyn Fn(&Value) -> Value) -> Value {
    let request = &pending["request"];
    let result = match request["kind"].as_str().unwrap() {
        "createRow" => json!({ "kind": "row", "result": create(&request["data"]) }),
        "listRows" => json!({ "kind": "rows", "result": { "ok": [] } }),
        "readRow" => json!({ "kind": "row", "result": read(&request["id"]) }),
        other => panic!("unexpected request {other}"),
    };
    json!({ "id": pending["id"], "answer": result })
}

fn run(create: &dyn Fn(&Value) -> Value) -> (Value, ServerApi) {
    run_with_read(create, &|_| json!({"ok":null}))
}

fn run_with_read(create: &dyn Fn(&Value) -> Value, read: &dyn Fn(&Value) -> Value) -> (Value, ServerApi) {
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
                    .map(|p| answer(p, create, read))
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
    let writes = std::cell::Cell::new(0);
    let (response, api) = run(&|data| {
        writes.set(writes.get() + 1);
        json!({ "ok": data })
    });
    assert_eq!(writes.get(), 1);
    assert_eq!(response["success"], json!(true));
    assert_eq!(response["states"]["Draft"], json!("saved"), "{response:#}");
    assert_eq!(api.get_state("Draft", None), "saved");
    assert_eq!(api.in_flight(), 0);
}

#[test]
fn control_a_refused_write_takes_the_failure_path() {
    let writes = std::cell::Cell::new(0);
    let (response, api) = run(&|_| {
        writes.set(writes.get() + 1);
        json!({ "error": "disk full" })
    });
    assert_eq!(writes.get(), 1);
    assert_eq!(response["effectResults"][0]["error"], "disk full");
    assert_eq!(response["states"]["Draft"], json!("failed"));
    assert_eq!(api.get_state("Draft", None), "failed");
}

#[test]
fn control_a_store_read_error_is_not_treated_as_an_absent_row() {
    let writes = std::cell::Cell::new(0);
    let (response, _) = run_with_read(&|data| {
        writes.set(writes.get() + 1);
        json!({"ok":data})
    }, &|_| json!({"error":"store unavailable"}));
    assert_eq!(writes.get(), 0);
    assert_eq!(response["states"]["Draft"], "failed");
    assert_eq!(response["effectResults"][0]["error"], "store unavailable");
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

#[test]
fn os_allows_answers_the_programs_grants_and_a_control_is_refused() {
    let mut program = schema();
    program["osGrants"] = json!({ "exec": ["git"], "fs": [{ "glob": "/data/**", "modes": ["read"] }] });
    let api = ServerApi::new(&program.to_string()).unwrap();
    let allows = |need: Value| api.os_allows(&need.to_string());
    assert_eq!(allows(json!({"need":"exec","command":"git"})), Ok(true));
    assert_eq!(allows(json!({"need":"exec","command":"rm"})), Ok(false));
    assert_eq!(allows(json!({"need":"fs","path":"/data/a/b","mode":"read"})), Ok(true));
    assert_eq!(allows(json!({"need":"fs","path":"/data/a","mode":"write"})), Ok(false));
    assert_eq!(allows(json!({"need":"fs","path":"/data/../etc","mode":"read"})), Ok(false));
    assert!(allows(json!({"need":"nope"})).is_err());
    assert!(orbital_core::embed::os_glob_matches("/data/**", "/data/x/y"));
    assert!(!orbital_core::embed::os_glob_matches("/data/*", "/data/x/y"));
}

#[test]
fn a_browser_host_matches_tab_urls_with_the_kernels_one_rule() {
    use orbital_core::embed::tab_url_matches;
    assert!(tab_url_matches("https://*.example.com/*", "https://www.example.com/a?b=1"));
    assert!(tab_url_matches("<all_urls>", "https://anything.test/"));
    assert!(!tab_url_matches("https://example.com/*", "http://example.com/"));
    assert!(!tab_url_matches("https://*.example.com/*", "https://badexample.com/"));
    assert!(!tab_url_matches("not a pattern", "https://example.com/"));
}

fn stored(program: &Value) -> (StoredServer, Arc<InMemoryEntityCache>) {
    let cache = Arc::new(InMemoryEntityCache::new());
    let source: Arc<dyn EntitySource> = cache.clone();
    (StoredServer::new(&program.to_string(), source, StoreCapabilities { query: None, write: None }).unwrap(), cache)
}

fn save() -> String {
    json!({ "event": "SAVE", "targetTrait": "Draft" }).to_string()
}

#[test]
fn a_stored_server_answers_its_own_row_asks_and_finishes_in_one_call() {
    let (server, cache) = stored(&schema());
    let done = progress(&server.start("Main", &save()).unwrap());
    assert_eq!(done["status"], "done", "{done:#}");
    assert_eq!(done["response"]["states"]["Draft"], json!("saved"), "{done:#}");
    assert_eq!(cache.list("Note").unwrap().len(), 1, "the row is in the store");
    assert_eq!(server.api().in_flight(), 0);
}

fn with_http(mut program: Value) -> Value {
    program["egress"] = json!(["https://api.example.test"]);
    let transition = &mut program["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0];
    transition["effects"].as_array_mut().unwrap().push(json!([
        "http", "GET", "https://api.example.test", "/ping", {}, { "emit": { "success": "SAVED", "failure": "SAVE_FAILED" } }
    ]));
    program
}

#[test]
fn control_a_stored_server_leaves_the_hosts_asks_to_the_host() {
    let (server, cache) = stored(&with_http(schema()));
    let waiting = progress(&server.start("Main", &save()).unwrap());
    assert_eq!(waiting["status"], "await", "{waiting:#}");
    let kinds: Vec<&str> = waiting["requests"].as_array().unwrap().iter().map(|r| r["request"]["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["http"], "only the host's ask is listed");
    assert_eq!(cache.list("Note").unwrap().len(), 1, "the row was written before the host was asked");
    let id = waiting["requests"][0]["id"].clone();
    let reply = json!([{ "id": id, "answer": { "kind": "http", "result": { "error": { "kind": "failed", "message": "down" } } } }]);
    let done = progress(&server.resume(waiting["flight"].as_u64().unwrap(), &reply.to_string()).unwrap());
    assert_eq!(done["status"], "done", "{done:#}");
    assert_eq!(server.api().in_flight(), 0);
}

#[test]
fn a_stored_server_refuses_an_unknown_flight() {
    let (server, _) = stored(&schema());
    assert!(server.resume(9, "[]").is_err());
    assert!(!server.abandon(9));
}

const STORED_ENV: &str = r#"{"DATA_BACKEND":"mock","NODE_ENV":"development"}"#;

/// The trace the Node add-on (`orbital-node/tests/stored.test.mjs`) and the Python extension
/// (`orbital-python/tests/test_stored.py`) must also produce for the same program over the memory store.
#[test]
fn the_memory_store_trace_is_the_shared_golden() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stored");
    let schema = std::fs::read_to_string(dir.join("notes.schema.json")).unwrap();
    let env: std::collections::HashMap<String, String> = serde_json::from_str(STORED_ENV).unwrap();
    let server = orbital_wasm::stored::ServerHandle::from_env(&schema, &env).unwrap();
    let start = progress(&server.start("Main", &save()).unwrap());
    let state = server.api().get_state("Draft", None);
    let trace = json!({ "start": start, "state": state, "inFlight": server.api().in_flight() });
    let golden = dir.join("notes.trace.json");
    if std::env::var("STORED_GOLDEN_WRITE").is_ok() {
        std::fs::write(&golden, serde_json::to_string_pretty(&trace).unwrap() + "\n").unwrap();
    }
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(&golden).unwrap()).unwrap();
    assert_eq!(trace, expected);
    assert_eq!(trace["start"]["status"], "done");
}

#[test]
fn control_a_production_environment_refuses_the_memory_store() {
    let schema = schema().to_string();
    let env: std::collections::HashMap<String, String> =
        serde_json::from_str(r#"{"DATA_BACKEND":"mock"}"#).unwrap();
    let err = orbital_wasm::stored::ServerHandle::from_env(&schema, &env).err().expect("refused");
    assert!(err.contains("not permitted"), "{err}");
}

#[test]
fn a_native_server_answers_the_hosts_asks_itself_under_the_programs_egress() {
    let program = with_http(schema());
    let env: std::collections::HashMap<String, String> = serde_json::from_str(STORED_ENV).unwrap();
    let events: orbital_wasm::stored::LiveSink = std::sync::Arc::new(|_| {});
    let server = orbital_wasm::stored::ServerHandle::from_env_native(&program.to_string(), &env, events.clone()).unwrap();
    let failure = |done: &Value| -> String {
        let event = done["response"]["emittedEvents"].as_array().unwrap().iter().find(|e| e["event"] == "SAVE_FAILED").cloned();
        event.map(|e| e["payload"]["error"].as_str().unwrap_or_default().to_string()).unwrap_or_default()
    };
    let done = progress(&server.start("Main", &save()).unwrap());
    assert_eq!(done["status"], "done", "no ask reached the host: {done:#}");
    assert!(failure(&done).contains("did not resolve"), "the declared origin was tried and could not be reached: {done:#}");

    let mut undeclared = with_http(schema());
    undeclared["egress"] = json!([]);
    let server = orbital_wasm::stored::ServerHandle::from_env_native(&undeclared.to_string(), &env, events).unwrap();
    let refused = progress(&server.start("Main", &save()).unwrap());
    assert_eq!(refused["status"], "done", "{refused:#}");
    let reason = failure(&refused);
    assert!(!reason.is_empty() && !reason.contains("did not resolve"), "control: an origin the program does not declare is refused before any lookup: {reason}");
}

#[test]
fn the_installed_language_exports_as_a_bundle_a_host_without_node_modules_installs() {
    let bundle = orbital_core::language_data::language_bundle_json().unwrap();
    let value: Value = serde_json::from_str(&bundle).unwrap();
    assert!(value["core"]["themes"].as_array().is_some_and(|t| !t.is_empty()));
    assert!(value["std"]["operators"].is_object() || value["std"]["operators"].is_array());
    let (language, operators, packs) = orbital_core::language_data::parse_language_bundle(&bundle, orbital_core::language_data::running_version()).unwrap();
    let installed = orbital_core::language_data::language_data().unwrap();
    assert_eq!(language.core_version, installed.core_version);
    assert_eq!(language.themes.len(), installed.themes.len());
    assert_eq!(operators.std_version, orbital_core::language_data::operator_data().unwrap().std_version);
    assert_eq!(packs.len(), orbital_core::i18n::packs::vocabulary_packs().unwrap().len());
    install_language(&bundle).unwrap();

    let mut tampered = value.clone();
    tampered["core"]["compiler"] = json!("=0.0.1");
    let err = install_language(&tampered.to_string()).unwrap_err();
    assert!(err.starts_with("ORB_LANGUAGE_DATA_INCOMPATIBLE"), "control: a bundle for another orb is refused: {err}");
}

fn owned_notes() -> Value {
    let mut program = schema();
    program["orbitals"][0]["entity"]["read_policy"] = json!(["=", ["object/get", "@entity", "ownerId"], "@user.id"]);
    program["orbitals"][0]["entity"]["fields"].as_array_mut().unwrap().push(json!({ "name": "ownerId", "type": "string" }));
    program
}

fn live_item(target: &str, origin: &str) -> String {
    json!({ "event": "NOTE_SAVED", "payload": { "id": "d1", "ownerId": "u1" }, "source": { "trait": "Draft" }, "originClientId": origin, "target": target }).to_string()
}

#[test]
fn live_items_are_routed_by_the_kernels_rule_origin_to_its_request_and_peers_by_the_viewers_read_policy() {
    let (server, _) = stored(&owned_notes());
    let server = orbital_wasm::stored::ServerHandle::Stored(server);
    let connections = json!([
        { "clientId": "a", "viewer": { "id": "u1" } },
        { "clientId": "b", "viewer": { "id": "u2" } },
        { "clientId": "c", "viewer": null },
        { "clientId": "d", "viewer": { "id": "u1" } }
    ]).to_string();
    let recipients = |item: String| -> Vec<usize> { serde_json::from_str(&server.live_recipients(&item, &connections).unwrap()).unwrap() };
    assert_eq!(recipients(live_item("origin", "b")), vec![1], "an origin item reaches only the requesting connection");
    assert_eq!(recipients(live_item("peers", "a")), vec![2, 3], "a peers item skips its own connection and the viewer who may not read the row");
    assert_eq!(recipients(live_item("origin", "nobody")), Vec::<usize>::new(), "control: an origin no connection holds reaches none");
}

#[test]
fn signing_in_with_firebase_does_not_open_googleapis_to_the_programs_own_http() {
    let mut program = with_http(schema());
    program["egress"] = json!([]);
    program["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0]["effects"][1][2] = json!("https://www.googleapis.com");
    let env: std::collections::HashMap<String, String> =
        serde_json::from_str(r#"{"DATA_BACKEND":"mock","NODE_ENV":"development","FIREBASE_PROJECT_ID":"test-project"}"#).unwrap();
    let sink: orbital_wasm::stored::LiveSink = std::sync::Arc::new(|_| {});
    let server = orbital_wasm::stored::ServerHandle::from_env_native(&program.to_string(), &env, sink).unwrap();
    let done = progress(&server.start("Main", &save()).unwrap());
    let refusal = done["response"]["emittedEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event"] == "SAVE_FAILED")
        .map(|e| e["payload"]["error"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    assert!(refusal.starts_with("egress denied: https://www.googleapis.com"), "the program's own http still needs its own egress: {done:#}");
}
