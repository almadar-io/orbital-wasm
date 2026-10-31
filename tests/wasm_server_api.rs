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
        "typeContractVersion": 1,
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
fn the_mount_ledger_is_driven_by_json_and_a_bad_list_is_refused() {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    api.mount_client("tab-1", "Main", r#"["Draft"]"#).unwrap();
    api.unmount_client("tab-1", "Main", r#"["Draft"]"#).unwrap();
    api.release_client("tab-1");
    let err = api.mount_client("tab-1", "Main", "not json").unwrap_err();
    assert_eq!(api.last_error(), err);
}

#[test]
fn the_tick_relay_opens_a_window_once_and_flushes_the_newest() {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    let stamped = |x: u32| json!({ "event": "MOVED", "payload": x, "clientId": "tab-1", "tick": "beat", "sourceTrait": "Draft" }).to_string();
    assert_eq!(api.relay_tick("Main", &stamped(1)).unwrap(), "50");
    assert_eq!(api.relay_tick("Main", &stamped(2)).unwrap(), "null");
    let items: Value = serde_json::from_str(&api.flush_tick_relay().unwrap()).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["payload"].as_f64(), Some(2.0));
    assert_eq!(items[0]["originClientId"], json!("tab-1"));
    assert!(api.relay_tick("Main", "not json").is_err());
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
fn egress_allows_answers_the_programs_declared_origins_and_a_control_is_refused() {
    let mut program = schema();
    program["egress"] = json!(["https://auth.example.com"]);
    let api = ServerApi::new(&program.to_string()).unwrap();
    assert!(api.egress_allows("https://auth.example.com/token"));
    assert!(!api.egress_allows("https://evil.example.com/token"));
    assert!(!api.egress_allows("http://auth.example.com/token"));
    assert!(!ServerApi::new(&schema().to_string()).unwrap().egress_allows("https://auth.example.com/token"));
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

#[test]
fn a_host_parses_a_dispatch_request_with_the_kernels_typed_parse() {
    use orbital_wasm::api::parse_event_request;
    let options = json!({ "event": "SAVE", "targetTrait": "Draft", "entityId": "", "viewer": { "id": "u1" } }).to_string();
    let parsed: Value = serde_json::from_str(&parse_event_request("{\"n\":\"x\"}", &options).unwrap()).unwrap();
    assert_eq!(parsed, json!({ "event": "SAVE", "payload": { "n": "x" }, "targetTrait": "Draft", "user": { "id": "u1" } }));
    let refused = parse_event_request("{}", "{}").unwrap_err();
    assert!(refused.starts_with("program/dispatch: not an event request: "), "{refused}");
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

fn with_host_ask(mut program: Value) -> Value {
    let transition = &mut program["orbitals"][0]["traits"][0]["stateMachine"]["transitions"][0];
    transition["effects"].as_array_mut().unwrap().push(json!([
        "random-bytes", 8, { "emit": { "success": "SAVED", "failure": "SAVE_FAILED" } }
    ]));
    program
}

#[test]
fn control_a_stored_server_leaves_the_hosts_asks_to_the_host() {
    let (server, cache) = stored(&with_host_ask(schema()));
    let waiting = progress(&server.start("Main", &save()).unwrap());
    assert_eq!(waiting["status"], "await", "{waiting:#}");
    let kinds: Vec<&str> = waiting["requests"].as_array().unwrap().iter().map(|r| r["request"]["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["random"], "only the host's ask is listed");
    assert_eq!(cache.list("Note").unwrap().len(), 1, "the row was written before the host was asked");
    let id = waiting["requests"][0]["id"].clone();
    let reply = json!([{ "id": id, "answer": { "kind": "random", "result": { "error": "down" } } }]);
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

#[test]
fn the_managed_program_is_set_by_json_and_a_malformed_one_is_refused() {
    let api = ServerApi::new(&schema().to_string()).unwrap();
    api.set_managed_program(r#"{"name":"shop","orbitals":[]}"#).unwrap();
    let err = api.set_managed_program("not json").unwrap_err();
    assert!(err.contains("program_json"), "{err}");
}

fn endpoint_program() -> Value {
    json!({
        "name": "endpoints",
        "typeContractVersion": 1,
        "orbitals": [{
            "name": "Main",
            "entity": { "name": "Note", "persistence": "persistent", "collection": "notes", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [
                { "name": "ViewT", "linkedEntity": "Note", "stateMachine": { "states": [{ "name": "a", "isInitial": true }], "transitions": [{ "from": "a", "to": "a", "event": "INIT", "effects": [] }] } },
                { "name": "HttpT", "linkedEntity": "Note", "stateMachine": { "states": [{ "name": "a", "isInitial": true }], "transitions": [{ "from": "a", "to": "a", "event": "INIT", "effects": [] }] } }
            ],
            "pages": [
                { "name": "Home", "path": "/", "traits": [{ "ref": "ViewT" }] },
                { "name": "Api", "path": "", "protocol": "http", "traits": [{ "ref": "HttpT" }] }
            ]
        }]
    })
}

#[test]
fn the_boot_mounts_are_the_kernels_for_the_side_and_never_a_route_trait() {
    let api = ServerApi::new(&endpoint_program().to_string()).unwrap();
    let server: Value = serde_json::from_str(&api.boot_mounts("server").unwrap()).unwrap();
    assert_eq!(server, json!([{ "trait": "HttpT", "event": "INIT" }]));
}

#[test]
fn control_the_process_side_boots_no_http_trait_and_an_unknown_side_is_refused() {
    let api = ServerApi::new(&endpoint_program().to_string()).unwrap();
    assert_eq!(api.boot_mounts("process").unwrap(), "[]");
    let route: Value = serde_json::from_str(&api.route_mounts("/").unwrap()).unwrap();
    assert_eq!(route, json!([{ "trait": "ViewT", "event": "INIT" }]));
    assert!(api.boot_mounts("everywhere").is_err());
    assert!(api.last_error().contains("parse side"));
}

#[test]
fn a_kernel_opening_a_route_gets_only_that_routes_traits_and_the_client_side_boots_none() {
    let kernel = orbital_wasm::api::KernelApi::new(&endpoint_program().to_string(), "c1").unwrap();
    let route: Value = serde_json::from_str(&kernel.route_mounts("/").unwrap()).unwrap();
    assert_eq!(route, json!([{ "trait": "ViewT", "event": "INIT" }]));
    assert_eq!(kernel.route_mounts("/missing").unwrap(), "[]");
    assert_eq!(kernel.boot_mounts("client").unwrap(), "[]");
}

#[test]
fn the_page_circuit_is_the_kernels_for_the_server_and_kernel_apis() {
    let program = endpoint_program().to_string();
    let api = ServerApi::new(&program).unwrap();
    let kernel = orbital_wasm::api::KernelApi::new(&program, "c1").unwrap();
    let by_route: Value = serde_json::from_str(&api.page_circuit(None, Some("/")).unwrap()).unwrap();
    assert_eq!(by_route["page"], "Home");
    assert_eq!(by_route["notFound"], false);
    assert_eq!(by_route["mounts"], json!([{ "trait": "ViewT", "event": "INIT" }]));
    assert_eq!(by_route["orbitalByTrait"], json!({ "ViewT": "Main" }));
    let by_name: Value = serde_json::from_str(&kernel.page_circuit(Some("Home"), None).unwrap()).unwrap();
    assert_eq!(by_name["page"], "Home");
    assert_eq!(by_name["linkedEntityByTrait"], json!({ "ViewT": "Note" }));
}

#[test]
fn control_an_unknown_page_is_refused_and_recorded() {
    let program = endpoint_program().to_string();
    let api = ServerApi::new(&program).unwrap();
    assert!(api.page_circuit(Some("Nope"), None).is_err());
    assert!(api.last_error().contains("unknown page: Nope"));
    let kernel = orbital_wasm::api::KernelApi::new(&program, "c1").unwrap();
    assert!(kernel.page_circuit(Some("Nope"), None).unwrap_err().contains("unknown page"));
}

#[test]
fn the_trait_surface_is_the_kernels_for_the_kernel_api() {
    let program = endpoint_program().to_string();
    let kernel = orbital_wasm::api::KernelApi::new(&program, "c1").unwrap();
    let surface: Value = serde_json::from_str(&kernel.contract_query(r#"{"kind":"traitSurface","trait":"ViewT"}"#).unwrap()).unwrap();
    assert_eq!(surface["orbital"], "Main");
    assert_eq!(surface["entity"], "Note");
    assert_eq!(surface["initial"], "a");
    let server = ServerApi::new(&program).unwrap();
    let served: Value = serde_json::from_str(&server.contract_query(r#"{"kind":"traitSurface","trait":"ViewT"}"#).unwrap()).unwrap();
    assert_eq!(surface, served);
}

#[test]
fn control_an_unknown_trait_surface_is_refused_and_recorded_on_the_kernel_api() {
    let kernel = orbital_wasm::api::KernelApi::new(&endpoint_program().to_string(), "c1").unwrap();
    let err = kernel.contract_query(r#"{"kind":"traitSurface","trait":"Nope"}"#).unwrap_err();
    assert_eq!(kernel.last_error(), err);
    assert!(kernel.contract_query("{not json").is_err());
}

fn lambda_effects() -> Value {
    serde_json::from_str(r#"[
        ["set", "@entity.id", "me"],
        ["set", "@entity.ok", ["validate/email", "a@b.co"]],
        ["set", "@entity.mood", ["if", ["=", ["object/get", ["default", ["array/first", "@payload.data"], {}], "mood", ""], ""], "calm", ["object/get", ["default", ["array/first", "@payload.data"], {}], "mood"]]],
        ["set", "@entity.trusted", ["object/get", ["default", ["array/first", "@payload.data"], {}], "trustedChannels", []]],
        ["emit", "MOOD_SET", { "mood": "@entity.mood", "hide": ["object/get", ["default", ["array/find", ["list", { "name": "a" }], ["fn", "m", ["=", ["object/get", "@m", "name"], "@entity.mood"]]], {}], "hide", []], "paused": ["object/get", ["default", ["array/first", "@payload.data"], {}], "paused", false] }],
        ["render-ui", "main", { "type": "stack", "children": [
            { "type": "page-header", "title": "Feed", "icon": "leaf" },
            { "type": "stack", "children": ["array/map", ["list", { "name": "a" }], ["fn", "m", { "type": "button", "label": ["object/get", "@m", "name"], "action": "CHOOSE_MOOD", "actionPayload": { "mood": ["object/get", "@m", "name"] }, "variant": ["if", ["=", ["object/get", "@m", "name"], "@entity.mood"], "primary", "ghost"] }]] },
            { "type": "typography", "content": ["str/concat", "Hiding ", ["str/join", ["object/get", ["default", ["array/find", ["list", { "name": "a" }], ["fn", "m", ["=", ["object/get", "@m", "name"], "@entity.mood"]]], {}], "hide", []], ", "]] }
        ] }]
    ]"#).unwrap()
}

fn lambda_program() -> Value {
    json!({
        "name": "lambdas",
        "typeContractVersion": 1,
        "_metadata": { "dispatchFacts": { "busExcluded": [], "targetOptions": [] }, "protocols": [{ "name": "route", "address": null, "viewer": false, "grant": null, "side": "client" }] },
        "orbitals": [{
            "name": "Main",
            "entity": { "name": "Note", "persistence": "persistent", "collection": "notes", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [{
                "name": "Pick", "linkedEntity": "Note",
                "emits": [
                    { "event": "PROFILE_LOADED", "payloadSchema": [{ "name": "data", "type": "array", "items": { "type": "Note" } }] },
                    { "event": "MOOD_SET", "payloadSchema": [{ "name": "mood", "type": "string" }, { "name": "hide", "type": "array", "items": { "type": "string" } }, { "name": "paused", "type": "boolean" }] }
                ],
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }, { "name": "picked" }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "PICK", "effects": [["fetch", "Note", { "emit": { "success": "PROFILE_LOADED", "failure": "PROFILE_FAILED" } }]] },
                        { "from": "idle", "to": "picked", "event": "PROFILE_LOADED", "effects": lambda_effects() }
                    ]
                }
            }]
        }]
    })
}

const NO_LANGUAGE_CHILD: &str = "ORBITAL_WASM_TEST_NO_LANGUAGE";

fn pick(api: &ServerApi) -> Result<String, String> {
    let mut step = api.start("Main", &json!({ "event": "PICK", "targetTrait": "Pick" }).to_string())?;
    loop {
        let value = progress(&step);
        if value["status"] != "await" {
            return Ok(step);
        }
        let replies: Vec<Value> = value["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pending| answer(pending, &|data| json!({ "ok": data }), &|_| json!({ "ok": { "id": "singleton" } })))
            .collect();
        step = api.resume(value["flight"].as_u64().unwrap(), &Value::Array(replies).to_string())?;
    }
}

#[test]
fn a_program_with_lambdas_answers_the_typed_error_without_a_language_bundle_and_the_server_stays_usable() {
    if std::env::var_os(NO_LANGUAGE_CHILD).is_none() {
        let empty = std::env::temp_dir().join(format!("orbital-wasm-no-language-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .arg("a_program_with_lambdas_answers_the_typed_error_without_a_language_bundle_and_the_server_stays_usable")
            .env(NO_LANGUAGE_CHILD, "1")
            .env("HOME", &empty)
            .env("USERPROFILE", &empty)
            .current_dir(&empty)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&empty).ok();
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let api = ServerApi::new(&lambda_program().to_string()).unwrap();
    let rejected = |api: &ServerApi| {
        let step = progress(&pick(api).unwrap());
        step["response"]["rejections"][0]["error"].as_str().unwrap_or_default().to_string()
    };
    let error = rejected(&api);
    assert!(error.contains("_NOT_INSTALLED"), "{error}");
    assert!(pick(&api).is_ok(), "the server answers a following call");
}

#[test]
fn control_the_same_lambda_program_runs_with_the_language_installed() {
    let bundle = orbital_core::language_data::language_bundle_json().unwrap();
    install_language(&bundle).unwrap();
    let api = ServerApi::new(&lambda_program().to_string()).unwrap();
    let step = progress(&pick(&api).unwrap());
    assert_eq!(step["status"], "done", "{step:#}");
    assert_eq!(api.get_state("Pick", None), "picked");
}
