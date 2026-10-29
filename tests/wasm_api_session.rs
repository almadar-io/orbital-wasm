//! The wasm-facing API (`KernelApi`, what `Kernel` forwards to) replays the embed scripted sessions
//! and must produce the C-ABI goldens recorded in orbital-core.

use orbital_wasm::api::{
    check_mutation_access, evaluate_guards, evaluate_many, evaluate_traced, map_listen_payload, resolve_row_binding, scope_rows,
    trace_effects, credential_seed_plan, KernelApi,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn embed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../orbital-core/tests/fixtures/embed")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn reported(result: Result<String, String>) -> Value {
    match result {
        Ok(text) => parsed(&text),
        Err(e) => json!({ "error": e }),
    }
}

fn step(kernel: &KernelApi, step: &Value) -> Value {
    let s = |key: &str| step[key].as_str().unwrap();
    let entity = step.get("entityId").and_then(Value::as_str);
    let text = |key: &str| step[key].to_string();
    match step["op"].as_str().unwrap() {
        "dispatch" => {
            let payload = step.get("payload").map(Value::to_string);
            reported(kernel.dispatch(s("trait"), entity, s("event"), payload.as_deref()))
        }
        "apply" => reported(kernel.apply_server_response(
            &text("response"),
            Some(&text("alreadyDelivered")),
            None,
        )),
        "restore" => reported(kernel.restore(s("trait"), entity, &text("snapshot"))),
        "abandon" => Value::Null,
        "tick" => reported(kernel.tick(s("trait"), s("tick"), entity)),
        "frame" => match kernel.frame(step["nowMs"].as_u64().unwrap()) {
            Ok(None) => Value::Null,
            other => reported(other.map(|o| o.unwrap())),
        },
        "framePause" => {
            kernel.frame_pause();
            Value::Null
        }
        "frameResume" => {
            kernel.frame_resume();
            Value::Null
        }
        "tickCount" => json!(kernel.tick_count()),
        "getState" => json!(kernel.get_state(s("trait"), entity)),
        "snapshot" => reported(kernel.snapshot()),
        "setThemeMode" => {
            let _ = kernel.set_theme_mode(s("mode"));
            Value::Null
        }
        "setUser" => {
            let _ = kernel.set_current_user(&text("user"));
            Value::Null
        }
        "seed" => {
            let _ = kernel.seed(s("entityType"), &text("rows"));
            Value::Null
        }
        "remount" => {
            let _ = kernel.remount(&text("traits"));
            Value::Null
        }
        "mount" => {
            let _ = kernel.mount(&text("traits"));
            Value::Null
        }
        "lastError" => json!(kernel.last_error()),
        other => panic!("unknown op {other}"),
    }
}

fn run_script(script: &Value) -> Vec<Value> {
    let schema = std::fs::read_to_string(
        embed_dir()
            .join("resolved")
            .join(format!("{}.json", script["schema"].as_str().unwrap())),
    )
    .unwrap();
    let kernel = KernelApi::new(&schema, script["clientId"].as_str().unwrap()).unwrap();
    script["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| step(&kernel, s))
        .collect()
}

#[test]
fn every_script_matches_the_c_abi_golden() {
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(embed_dir().join("scripts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    scripts.sort();
    assert_eq!(scripts.len(), 6);
    for path in scripts {
        let script = read_json(&path);
        let expected = read_json(&embed_dir().join("golden").join(path.file_name().unwrap()));
        let outputs = run_script(&script);
        let expected = expected.as_array().unwrap();
        assert_eq!(outputs.len(), expected.len(), "{}", path.display());
        for (i, (got, want)) in outputs.iter().zip(expected).enumerate() {
            assert_eq!(
                got,
                want,
                "{} step {i} {}",
                path.display(),
                script["steps"][i]
            );
        }
    }
}

#[test]
fn control_a_changed_step_breaks_the_golden() {
    let mut script = read_json(&embed_dir().join("scripts/rollback.json"));
    let golden = read_json(&embed_dir().join("golden/rollback.json"));
    script["steps"][2]["event"] = json!("INIT");
    assert_ne!(Value::Array(run_script(&script)), golden);
}

#[test]
fn an_empty_schema_is_refused() {
    let err = KernelApi::new(r#"{"name":"Empty","orbitals":[]}"#, "c").err();
    assert!(matches!(
        err,
        Some(orbital_wasm::api::ApiError::Entry(orbital_core::runtime::kernel::EntryError::NoOrbitals))
    ));
}

#[test]
fn a_failed_call_is_the_error_and_the_last_error() {
    let schema = std::fs::read_to_string(embed_dir().join("resolved/basic-entity.json")).unwrap();
    let kernel = KernelApi::new(&schema, "c").unwrap();
    let err = kernel
        .dispatch("NoSuchTrait", None, "INIT", None)
        .unwrap_err();
    assert_eq!(kernel.last_error(), err);
    assert!(kernel.set_current_user("{not json").is_err());
}

fn norm(value: Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 => json!(f as i64),
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(norm).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, norm(v))).collect()),
        other => other,
    }
}

fn pj(text: &str) -> Value {
    norm(parsed(text))
}

fn evaluated(context: &Value, expressions: &Value) -> Vec<Value> {
    pj(&evaluate_many(&context.to_string(), &expressions.to_string()).unwrap())
        .as_array()
        .unwrap()
        .clone()
}

fn value_of(outcome: &Value) -> &Value {
    assert_eq!(outcome["kind"], "value", "{outcome}");
    &outcome["value"]
}

#[test]
fn one_crossing_evaluates_every_binding_of_a_render_against_one_context() {
    let context = json!({
        "entity": { "qty": 3, "name": "ada", "tags": ["a", "b"] },
        "payload": { "step": 2 },
        "config": { "limit": 5 },
        "user": { "id": "u1", "role": "admin" },
        "state": "open",
        "now": 1000,
        "locale": "ar"
    });
    let out = evaluated(
        &context,
        &json!([
            ["+", "@entity.qty", "@payload.step"],
            ["=", "@user.role", "admin"],
            "@state",
            "@locale",
            ["time/add", "@now", 1, "ms"],
            ["str/upper", "@entity.name"],
            ["array/len", "@entity.tags"],
            ["<", "@entity.qty", "@config.limit"]
        ]),
    );
    let values: Vec<&Value> = out.iter().map(value_of).collect();
    assert_eq!(
        values,
        vec![&json!(5), &json!(true), &json!("open"), &json!("ar"), &json!("1970-01-01T00:00:01.001Z"), &json!("ADA"), &json!(2), &json!(true)]
    );
}

#[test]
fn control_a_failing_expression_is_its_own_outcome_and_an_unbound_root_reads_null() {
    let context = json!({ "entity": { "qty": 3 } });
    let out = evaluated(&context, &json!([["+", 1, 2], ["/", 1, 0], "@user.id", ["+", 2, 2]]));
    assert_eq!(value_of(&out[1]), &Value::Null, "division by zero answers null (P-010 ruling 2026-10-25), not an error");
    let out = evaluated(&context, &json!([["+", 1, 2], ["/", 1], "@user.id", ["+", 2, 2]]));
    assert_eq!(value_of(&out[0]), &json!(3));
    assert_eq!(out[1]["kind"], "error");
    assert!(out[1]["message"].as_str().unwrap().len() > 0);
    assert_eq!(value_of(&out[2]), &Value::Null);
    assert_eq!(value_of(&out[3]), &json!(4));
}

#[test]
fn a_malformed_context_or_expression_list_is_a_typed_error_not_an_outcome() {
    assert!(evaluate_many("{not json", "[]").is_err());
    assert!(evaluate_many("{}", "{\"not\":\"a list\"}").is_err());
    assert!(evaluate_many("{\"now\":\"soon\"}", "[]").is_err());
}

#[test]
fn guards_are_decided_in_one_crossing_as_booleans_and_a_failing_guard_is_its_own_error() {
    let context = json!({ "entity": { "qty": 3, "tags": [], "name": "", "ok": true } }).to_string();
    let guards = json!([
        [">", "@entity.qty", 1],
        [">", "@entity.qty", 9],
        ["/", 1],
        "@entity.tags",
        "@entity.name",
        "@entity.qty",
        "@entity.ok"
    ]);
    let verdicts = pj(&evaluate_guards(&context, &guards.to_string()).unwrap());
    assert_eq!(verdicts[0], json!({ "kind": "value", "value": true }));
    assert_eq!(verdicts[1], json!({ "kind": "value", "value": false }));
    assert_eq!(verdicts[2]["kind"], "error");
    for not_boolean in [3, 4, 5] {
        assert_eq!(verdicts[not_boolean]["kind"], "error", "a guard is a boolean, never truthiness: {}", verdicts[not_boolean]);
    }
    assert_eq!(verdicts[6], json!({ "kind": "value", "value": true }), "control: a boolean guard");
}

fn traced(context: Value, expression: Value) -> Value {
    pj(&evaluate_traced(&context.to_string(), &expression.to_string()).unwrap())
}

fn steps(trace: &Value, kind: &str) -> Vec<Value> {
    trace["trace"].as_array().unwrap().iter().filter(|s| s["kind"] == kind).map(|s| s["path"].clone()).collect()
}

fn exit_value(trace: &Value, path: Value) -> Value {
    trace["trace"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "exit" && s["path"] == path)
        .map(|s| s["value"].clone())
        .unwrap()
}

#[test]
fn a_traced_evaluation_records_each_node_by_path_and_returns_the_untraced_value() {
    let expression = json!(["+", 1, ["*", 2, 3]]);
    let trace = traced(json!({}), expression.clone());
    assert_eq!(trace["outcome"], json!({ "kind": "value", "value": 7 }));
    let plain = evaluated(&json!({}), &json!([expression]));
    assert_eq!(value_of(&plain[0]), &json!(7));
    assert_eq!(exit_value(&trace, json!([])), json!(7));
    assert_eq!(exit_value(&trace, json!([2])), json!(6));
    assert_eq!(exit_value(&trace, json!([2, 1])), json!(2));
    assert_eq!(trace["trace"][0], json!({ "kind": "enter", "path": [] }));
    assert_eq!(trace["trace"].as_array().unwrap().last().unwrap(), &json!({ "kind": "exit", "path": [], "value": 7 }));
    assert_eq!(steps(&trace, "enter"), vec![json!([]), json!([1]), json!([2]), json!([2, 1]), json!([2, 2])]);
}

#[test]
fn a_traced_binding_is_its_resolved_value() {
    let trace = traced(json!({ "entity": { "qty": 3 } }), json!([">", "@entity.qty", 0]));
    assert_eq!(exit_value(&trace, json!([1])), json!(3));
}

#[test]
fn a_short_circuited_argument_is_skipped_and_control_a_fully_run_call_skips_nothing() {
    let context = json!({ "entity": { "qty": 0 } });
    assert_eq!(steps(&traced(context.clone(), json!(["and", [">", "@entity.qty", 0], ["=", 1, 1]])), "skip"), vec![json!([2])]);
    assert_eq!(steps(&traced(context.clone(), json!(["or", ["=", 1, 1], ["=", 2, 2]])), "skip"), vec![json!([2])]);
    assert_eq!(steps(&traced(context.clone(), json!(["if", [">", "@entity.qty", 5], "big", "small"])), "skip"), vec![json!([2])]);
    assert_eq!(steps(&traced(context, json!(["and", ["=", 1, 1], ["=", 2, 2]])), "skip"), Vec::<Value>::new());
}

#[test]
fn a_lambda_body_is_traced_under_its_own_path_once_per_item() {
    let trace = traced(json!({}), json!(["array/map", ["list", 1, 2, 3], ["fn", "x", ["*", "@x", 2]]]));
    let body_exits: Vec<Value> = trace["trace"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["kind"] == "exit" && s["path"] == json!([2, 2]))
        .map(|s| s["value"].clone())
        .collect();
    assert_eq!(body_exits, vec![json!(2), json!(4), json!(6)]);
    assert_eq!(steps(&trace, "iter").iter().filter(|p| **p == json!([2, 2])).count(), 2);
}

#[test]
fn equal_literal_arguments_trace_as_separate_nodes() {
    let trace = traced(json!({}), json!(["+", 1, 1]));
    assert_eq!(steps(&trace, "enter"), vec![json!([]), json!([1]), json!([2])]);
}

#[test]
fn a_failing_trace_keeps_the_steps_up_to_the_failure_and_control_a_success_has_no_error() {
    let failed = traced(json!({}), json!(["+", 1, ["/", 1]]));
    assert_eq!(failed["outcome"]["kind"], "error");
    let failed_exits: Vec<Value> = failed["trace"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["kind"] == "exit" && s.get("error").is_some())
        .map(|s| s["path"].clone())
        .collect();
    assert_eq!(failed_exits, vec![json!([2]), json!([])]);
    assert!(failed["trace"].as_array().unwrap().iter().any(|s| s["kind"] == "exit" && s["path"] == json!([1])));
    assert!(traced(json!({}), json!(["+", 1, 2]))["trace"].as_array().unwrap().iter().all(|s| s.get("error").is_none()));
}

#[test]
fn tracing_one_expression_leaves_the_next_evaluation_untraced() {
    let _ = traced(json!({}), json!(["+", 1, 2]));
    let plain = evaluated(&json!({}), &json!([["+", 1, 2]]));
    assert_eq!(value_of(&plain[0]), &json!(3));
}

#[test]
fn a_listen_mapping_rewrites_the_payload_and_control_no_mapping_passes_it_through() {
    let payload = json!({ "id": "r1", "count": 4, "extra": true });
    let mapped = map_listen_payload(
        &json!({ "rowId": "@payload.id", "double": ["*", "@payload.count", 2], "label": "fixed", "broken": ["/", 1], "nulled": ["/", 1, 0] }).to_string(),
        &payload.to_string(),
    )
    .unwrap();
    assert_eq!(pj(&mapped), json!({ "rowId": "r1", "double": 8, "label": "fixed", "nulled": null }));
    assert_eq!(pj(&map_listen_payload("", &payload.to_string()).unwrap()), payload);
}

fn rows() -> Value {
    json!([{ "id": "a", "owner": "u1", "qty": 1 }, { "id": "b", "owner": "u2", "qty": 5 }, { "id": "c", "owner": "u1", "qty": 9 }])
}

fn scoped(user: Value, policy: &str, filter: &str) -> Vec<String> {
    let kept = scope_rows(&json!({ "user": user }).to_string(), &rows().to_string(), policy, filter, "Ticket").unwrap();
    pj(&kept).as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_read_policy_narrows_rows_to_the_viewer_and_the_filter_narrows_further() {
    let owner = json!(["=", "@entity.owner", "@user.id"]).to_string();
    let big = json!([">", "@entity.qty", 3]).to_string();
    assert_eq!(scoped(json!({ "id": "u1" }), &owner, ""), vec!["a", "c"]);
    assert_eq!(scoped(json!({ "id": "u2" }), &owner, ""), vec!["b"]);
    assert_eq!(scoped(json!({ "id": "u1" }), &owner, &big), vec!["c"]);
    assert_eq!(scoped(json!({ "id": "u1" }), "", &big), vec!["b", "c"]);
}

#[test]
fn control_no_policy_and_no_filter_keeps_every_row_and_a_failing_policy_drops_them_all() {
    assert_eq!(scoped(json!({ "id": "u1" }), "", ""), vec!["a", "b", "c"]);
    assert!(scoped(json!({ "id": "u1" }), &json!(["/", 1]).to_string(), "").is_empty());
}

fn verdict(op: &str, user: Value, policy: &str) -> Value {
    let row = json!({ "id": "a", "owner": "u1" });
    pj(&check_mutation_access(op, &json!({ "user": user }).to_string(), &row.to_string(), policy, "Ticket").unwrap())
}

#[test]
fn a_mutation_directive_allows_the_owner_and_denies_another_viewer_with_the_reason() {
    let policy = json!(["=", "@entity.owner", "@user.id"]).to_string();
    for op in ["create", "update", "delete"] {
        assert_eq!(verdict(op, json!({ "id": "u1" }), &policy), json!({ "allowed": true }));
        let denied = verdict(op, json!({ "id": "u2" }), &policy);
        assert_eq!(denied["allowed"], false);
        assert!(denied["message"].as_str().unwrap().contains("Ticket"));
    }
}

#[test]
fn control_a_mutation_with_no_directive_is_allowed_and_a_failing_directive_denies() {
    assert_eq!(verdict("update", json!({ "id": "u2" }), ""), json!({ "allowed": true }));
    assert_eq!(verdict("update", json!({ "id": "u1" }), &json!(["/", 1]).to_string())["allowed"], false);
    assert!(check_mutation_access("read", "{}", "{}", "", "Ticket").is_err());
}

#[test]
fn a_row_binding_term_folds_to_a_scalar_a_list_or_a_typed_refusal() {
    let bindings = json!({
        "user": { "id": "u1", "teams": ["x", "y"], "profile": { "n": 1 } },
        "payload": { "q": 7 },
        "config": { "k": null },
        "now": 5
    })
    .to_string();
    let fold = |term: &str| pj(&resolve_row_binding(term, &bindings).unwrap());
    assert_eq!(fold("@user.id"), json!({ "kind": "value", "value": "u1" }));
    assert_eq!(fold("@payload.q"), json!({ "kind": "value", "value": 7 }));
    assert_eq!(fold("@config.k"), json!({ "kind": "value", "value": null }));
    assert_eq!(fold("@now"), json!({ "kind": "value", "value": "1970-01-01T00:00:00.005Z" }));
    assert_eq!(fold("@user.teams"), json!({ "kind": "list", "values": ["x", "y"] }));
    assert_eq!(fold("@user.profile")["kind"], "inexpressible");
    assert_eq!(fold("@user.missing")["kind"], "unspecialised");
    assert_eq!(fold("@entity.id")["kind"], "inexpressible");
    assert_eq!(fold("plain")["kind"], "inexpressible");
}

#[test]
fn a_string_operator_refuses_an_absent_value_and_control_a_present_string_null_stays() {
    let context = json!({ "payload": { "present": "null", "n": 3 } });
    let out = evaluated(
        &context,
        &json!([
            ["str/concat", "a", "@payload.missing", "b"],
            ["str/upper", "@payload.missing"],
            ["str/len", "@payload.missing"],
            ["str/trim", "@payload.missing"],
            ["str/concat", "a", "@payload.present"],
            ["str/upper", "@payload.present"],
            ["str/concat", "n=", "@payload.n"],
            ["str/upper", ["default", "@payload.missing", ""]]
        ]),
    );
    for absent in &out[..4] {
        assert_eq!(absent["kind"], "error", "an absent value is not a string (EXEC-EVAL-2): {absent}");
    }
    let values: Vec<&Value> = out[4..].iter().map(value_of).collect();
    assert_eq!(values, vec![&json!("anull"), &json!("NULL"), &json!("n=3"), &json!("")], "controls: present values, and `default` says what absence means");
}

#[test]
fn trace_effects_runs_a_list_against_a_row_through_the_wasm_api() {
    let context = json!({ "entity": { "n": 1 }, "state": "idle" });
    let effects = json!([["set", "@entity.n", ["+", "@entity.n", 4]], ["emit", "DONE", { "n": "@entity.n" }]]);
    let out = pj(&trace_effects(&context.to_string(), &effects.to_string()).unwrap());
    assert_eq!(out["entity"]["n"].as_f64(), Some(5.0));
    assert_eq!(out["emitted"][0]["event"], "DONE");
    assert_eq!(out["emitted"][0]["payload"]["n"].as_f64(), Some(5.0));
    assert_eq!(out["effects"][0]["status"], "executed");
    assert!(trace_effects("{}", "not json").unwrap_err().contains("parse effects"), "a malformed list is an error");
}

#[test]
fn credential_seed_plan_names_the_declared_credentials_the_store_lacks_from_the_names_the_env_sets() {
    let declared = r#"[{"service":"stripe","envVar":"STRIPE_KEY"},{"service":"stripe","envVar":"STRIPE_WEBHOOK"}]"#;
    let present = r#"["STRIPE_KEY","STRIPE_WEBHOOK"]"#;
    let plan = credential_seed_plan(declared, r#"["STRIPE_WEBHOOK"]"#, present).unwrap();
    assert_eq!(plan, r#"[{"service":"stripe","envVar":"STRIPE_KEY"}]"#);
    assert_eq!(credential_seed_plan(declared, "[]", "[]").unwrap(), "[]", "control: nothing set, nothing to seed");
}

fn greeting_schema() -> String {
    json!({
        "name": "site",
        "typeContractVersion": 1,
        "orbitals": [{
            "name": "Home",
            "entity": { "name": "HomeView", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [{
                "name": "HomePage", "linkedEntity": "HomeView",
                "stateMachine": {
                    "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "INIT",
                        "effects": [["render-ui", "main", { "type": "typography", "content": ["i18n/t", "site:hero.title"] }]] }]
                }
            }]
        }]
    })
    .to_string()
}

#[test]
fn the_client_kernel_paints_i18n_from_the_catalogs_the_host_sets() {
    let kernel = KernelApi::new(&greeting_schema(), "c1").unwrap();
    kernel.set_messages(r#"["en"]"#, r#"{"en":{"site:hero.title":"Hello"}}"#).unwrap();
    let wire = parsed(&kernel.dispatch("HomePage", None, "INIT", None).unwrap());
    assert!(wire["response"]["clientEffectsByTrait"].to_string().contains("\"Hello\""), "{wire:#}");
}

#[test]
fn control_a_malformed_catalog_is_refused_by_name() {
    let kernel = KernelApi::new(&greeting_schema(), "c1").unwrap();
    let err = kernel.set_messages(r#"["en"]"#, r#"{"en":["not","a","map"]}"#).unwrap_err();
    assert!(err.contains("catalogs_json"), "{err}");
}

fn visible_schema() -> String {
    let tick = |name: &str, visible: bool| {
        json!({
            "name": name, "linkedEntity": "E", "dispatch": "client",
            "stateMachine": { "states": [{ "name": "on", "isInitial": true }], "transitions": [] },
            "ticks": [{ "name": "beat", "interval": 0, "whileVisible": visible, "effects": [["set", "@entity.n", 1]] }]
        })
    };
    json!({
        "name": "App", "typeContractVersion": 1,
        "orbitals": [{
            "name": "Main",
            "entity": { "name": "E", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }, { "name": "n", "type": "number" }] },
            "traits": [tick("V", true), tick("P", false)]
        }]
    })
    .to_string()
}

fn fired_names(kernel: &KernelApi, now_ms: u64) -> Vec<String> {
    let mut names: Vec<String> = kernel
        .frame(now_ms)
        .unwrap()
        .map(|wire| parsed(&wire)["fired"].as_array().unwrap().iter().map(|t| t["trait"].as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn a_visible_tick_is_skipped_once_the_host_reports_its_trait_off_screen() {
    let kernel = KernelApi::new(&visible_schema(), "c1").unwrap();
    kernel.set_trait_on_screen("V", false);
    kernel.set_trait_on_screen("P", false);
    assert_eq!(fired_names(&kernel, 0), ["P"], "EXEC-TICK-4: only the plain tick fires off screen");
    kernel.set_trait_on_screen("V", true);
    assert_eq!(fired_names(&kernel, 1), ["P", "V"], "back on screen it resumes");
}

#[test]
fn control_a_visible_tick_the_host_never_reported_fires() {
    let kernel = KernelApi::new(&visible_schema(), "c1").unwrap();
    assert_eq!(fired_names(&kernel, 0), ["P", "V"]);
}

fn ordered_kernel() -> KernelApi {
    let schema = std::fs::read_to_string(embed_dir().join("resolved").join("basic-entity.json")).unwrap();
    KernelApi::new(&schema, "golden-client").unwrap()
}

fn flight_of(kernel: &KernelApi, event: &str) -> u64 {
    parsed(&kernel.dispatch("TaskCRUD", None, event, None).unwrap())["flight"].as_u64().expect("the dispatch owes a leg")
}

const ANSWER: &str = r#"{"success":true,"transitioned":true,"states":{"TaskCRUD":"listing"},"emittedEvents":[]}"#;

#[test]
fn an_answer_ahead_of_an_older_flight_is_queued_and_folded_by_next_fold_once_that_flight_ends() {
    let kernel = ordered_kernel();
    let first = flight_of(&kernel, "INIT");
    let second = flight_of(&kernel, "INIT");
    assert!(first < second);
    let queued = parsed(&kernel.fold_in_order(second, ANSWER, None, None).unwrap());
    assert_eq!(queued["status"], "queued");
    assert!(kernel.next_fold().unwrap().is_none());
    assert_eq!(parsed(&kernel.fold_in_order(first, ANSWER, None, None).unwrap())["status"], "applied");
    assert!(kernel.end_flight(first));
    let next = parsed(&kernel.next_fold().unwrap().expect("the queued answer folds"));
    assert_eq!(next["outcome"], "applied");
    assert_eq!(next["ticket"], queued["ticket"]);
    assert!(kernel.next_fold().unwrap().is_none());
}

#[test]
fn control_answers_in_dispatch_order_fold_at_once_and_an_unknown_flight_is_an_error() {
    let kernel = ordered_kernel();
    let first = flight_of(&kernel, "INIT");
    assert_eq!(parsed(&kernel.fold_in_order(first, ANSWER, None, None).unwrap())["status"], "applied");
    assert!(kernel.fold_in_order(first + 100, ANSWER, None, None).is_err());
    assert!(!kernel.end_flight(first + 100));
}
