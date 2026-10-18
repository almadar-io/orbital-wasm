//! The wasm-facing API (`KernelApi`, what `Kernel` forwards to) replays the embed scripted sessions
//! and must produce the C-ABI goldens recorded in orbital-core.

use orbital_core::planner;
use orbital_core::OrbitalSchema;
use orbital_wasm::api::{
    check_mutation_access, evaluate_guards, evaluate_many, evaluate_traced, map_listen_payload, resolve_row_binding, scope_rows,
    verify_affordance, verify_credit, verify_expect, verify_judge, verify_plan, verify_replay, verify_transient_probe, trace_effects, credential_seed_plan, KernelApi,
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
    assert_eq!(err.as_deref(), Some("resolved schema declares no orbitals"));
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

const VERIFY_FIXTURES: [&str; 5] = ["entity_payload_skeleton", "02-state-machine", "04-complex-guards", "json-array", "precondition"];

fn verify_fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/verify/{name}.json"))).unwrap()
}

fn native_plan(schema: &OrbitalSchema) -> Vec<Value> {
    let owner_fields = orbital_core::runtime::owner_fields_from_schema(schema);
    let ctx = planner::EntityCtx { schema, owner_fields: &owner_fields, viewer_id: None };
    let mut out = Vec::new();
    for td in planner::inline_traits(schema) {
        let seed_id = td.linked_entity.as_deref().map(|e| planner::effective_seed_id(e, &owner_fields, None));
        for step in planner::plan_trait(td, &ctx, seed_id.as_deref()) {
            let preamble = planner::replay_steps(td, &step.from, &ctx, seed_id.as_deref())
                .map(|hops| hops.into_iter().map(|h| (h.event, h.payload)).collect::<Vec<_>>());
            out.push(json!({
                "trait": step.trait_name, "from": step.from, "event": step.event, "to": step.to,
                "toStates": step.to_states, "variant": step.variant, "payload": step.payload,
                "guarded": step.guarded, "arm": step.arm, "solved": step.solved, "reply": step.reply,
                "preambleEvents": if step.variant == planner::Variant::AutoInit && step.event == "INIT" { None } else { preamble }.map(|p| p.into_iter().map(|(event, payload)| json!({ "event": event, "payload": payload })).collect::<Vec<_>>()),
            }));
        }
    }
    out
}

fn exported_plan(text: &str) -> Vec<Value> {
    let plan = parsed(&verify_plan(text, ""));
    assert_eq!(plan["outcome"], "planned", "{plan}");
    plan["steps"].as_array().unwrap().iter().filter(|s| s["payloadCase"] == "success" && s.get("guardCase").is_none_or(|g| g == "pass")).map(|s| {
        let skip_init = s["variant"] == "auto-init" && s["event"] == "INIT";
        json!({
            "trait": s["trait"], "from": s["from"], "event": s["event"], "to": s["to"],
            "toStates": s["toStates"], "variant": s["variant"], "payload": s["payload"],
            "guarded": s["guarded"], "arm": s.get("arm").cloned().unwrap_or(Value::Null), "solved": s["solved"],
            "reply": s.get("reply").cloned().unwrap_or(Value::Null),
            "preambleEvents": if skip_init { Value::Null } else { s.get("preamble").map_or(Value::Null, |hops| Value::Array(hops.as_array().unwrap().iter().map(|h| json!({ "event": h["event"], "payload": h["payload"] })).collect())) },
        })
    }).collect()
}

#[test]
fn the_exported_plan_is_the_native_plan_on_three_verify_fixtures() {
    for name in VERIFY_FIXTURES {
        let text = verify_fixture(name);
        let schema: OrbitalSchema = serde_json::from_str(&text).unwrap();
        let native = native_plan(&schema);
        assert!(!native.is_empty(), "{name} plans no step");
        assert_eq!(exported_plan(&text), native, "{name}");
        let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/verify/{name}.plan.json"));
        let mut full = parsed(&verify_plan(&text, ""));
        full.as_object_mut().unwrap().remove("contractEvents");
        if std::env::var("VERIFY_PLAN_GOLDEN_WRITE").as_deref() == Ok("1") {
            std::fs::write(&golden_path, serde_json::to_string_pretty(&full).unwrap() + "\n").unwrap();
        }
        assert_eq!(full, read_json(&golden_path), "{name}: the golden plan is stale");
    }
}

#[test]
fn an_exported_verdict_passes_a_declared_landing_and_fails_any_other_state() {
    for name in VERIFY_FIXTURES {
        let plan = parsed(&verify_plan(&verify_fixture(name), ""));
        let step = plan["steps"].as_array().unwrap().iter()
            .find(|s| s.get("reply").is_none() && s.get("arm").is_none() && s["guarded"] == false && s["payloadCase"] == "success" && s["acceptStates"].is_null())
            .unwrap_or_else(|| panic!("{name} plans an unguarded driven step"));
        let expectation = parsed(&verify_expect(&step.to_string()).unwrap());
        let observe = |after: &str| json!({ "stateBefore": step["from"], "stateAfter": after, "effects": [] });
        let landed = parsed(&verify_judge(&expectation.to_string(), &observe(step["to"].as_str().unwrap()).to_string()).unwrap());
        assert_eq!(landed["status"], "pass", "{name}: {landed}");
        let strayed = parsed(&verify_judge(&expectation.to_string(), &observe("__nowhere__").to_string()).unwrap());
        assert_eq!(strayed, json!({ "status": "fail", "reason": { "kind": "wrong-state", "actual": "__nowhere__" } }), "{name}");
    }
}

#[test]
fn control_a_broken_program_or_scope_is_a_typed_refusal() {
    let broken = parsed(&verify_plan("{not json", ""));
    assert_eq!(broken["outcome"], "refused");
    assert_eq!(broken["failure"]["kind"], "invalid-program");
    let program = verify_fixture("02-state-machine");
    let scoped = parsed(&verify_plan(&program, r#"{"traits":["NoSuchTrait"]}"#));
    assert_eq!(scoped["failure"]["kind"], "unknown-trait");
    assert_eq!(scoped["failure"]["name"], "NoSuchTrait");
    assert!(scoped["failure"]["available"].as_str().unwrap().contains("OrderLifecycle"));
    assert_eq!(parsed(&verify_plan(&program, "[1]"))["failure"]["kind"], "invalid-input");
    let one = parsed(&verify_plan(&program, r#"{"traits":["OrderLifecycle"]}"#));
    assert_eq!(one["outcome"], "planned");
    assert!(verify_judge("{}", "{}").is_err());
}

fn variants_program() -> Value {
    json!({
        "name": "App",
        "orbitals": [
            {
                "name": "Main",
                "entity": { "name": "Job", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [{ "name": "Runner", "linkedEntity": "Job",
                    "stateMachine": {
                        "states": [{ "name": "idle", "isInitial": true }, { "name": "loading" }, { "name": "ready" }, { "name": "big" }, { "name": "small" }, { "name": "gone" }],
                        "events": [
                            { "key": "GO", "name": "GO", "payloadSchema": [{ "name": "size", "type": "number", "required": true }] },
                            { "key": "LEAVE", "name": "LEAVE" }
                        ],
                        "transitions": [
                            { "from": "idle", "to": "idle", "event": "INIT" },
                            { "from": "idle", "to": "loading", "event": "FETCH", "effects": [["fetch", "Job", { "emit": { "success": "LOADED" } }]] },
                            { "from": "loading", "to": "ready", "event": "LOADED" },
                            { "from": "ready", "to": "big", "event": "GO", "guard": [">", "@payload.size", 10] },
                            { "from": "ready", "to": "small", "event": "GO", "guard": ["<=", "@payload.size", 10] },
                            { "from": "ready", "to": "gone", "event": "NEVER", "guard": ["=", 1, 2] },
                            { "from": "ready", "to": "gone", "event": "LEAVE", "effects": [["navigate", "/away"]] }
                        ]
                    } }]
            },
            {
                "name": "Side",
                "entity": { "name": "Note", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
                "traits": [{ "name": "Router", "linkedEntity": "Note",
                    "listens": [{ "event": "FETCH", "triggers": "JUMP", "source": { "kind": "orbital", "orbital": "Main", "trait": "Runner" } }],
                    "stateMachine": {
                        "states": [{ "name": "here", "isInitial": true }],
                        "transitions": [
                            { "from": "here", "to": "here", "event": "INIT" },
                            { "from": "here", "to": "here", "event": "JUMP", "effects": [["navigate", "/other"]] }
                        ]
                    } }]
            }
        ]
    })
}

fn variant_steps(program: &Value) -> Vec<Value> {
    let plan = parsed(&verify_plan(&program.to_string(), r#"{"traits":["Runner"]}"#));
    assert_eq!(plan["outcome"], "planned", "{plan}");
    plan["steps"].as_array().unwrap().clone()
}

fn find<'a>(steps: &'a [Value], event: &str, case: &str, arm: Option<u64>) -> Option<&'a Value> {
    steps.iter().find(|s| s["event"] == event && s["payloadCase"] == case && s.get("arm").and_then(Value::as_u64) == arm)
}

#[test]
fn the_plan_carries_the_malformed_and_guard_fail_variants_with_their_flags() {
    let steps = variant_steps(&variants_program());
    let malformed = find(&steps, "GO", "malformed", Some(0)).expect("a required field plans a malformed arm");
    assert_eq!(malformed["payload"], json!({}));
    assert_eq!(malformed["coverageKey"], "Runner:ready+GO->big[malformed]");
    let pass = find(&steps, "GO", "success", Some(0)).unwrap();
    assert_eq!((pass["guardCase"].as_str(), pass["guardSteerable"].as_bool()), (Some("pass"), Some(true)));
    assert_eq!(pass["coverageKey"], "Runner:ready+GO->big[success]");
    let fail = find(&steps, "GO", "guard-fail", Some(0)).unwrap();
    assert_eq!(fail["guardCase"], "fail");
    assert_eq!(fail["guardSiblingTargets"], json!(["small"]));
    assert_eq!(fail["coverageKey"], "Runner:ready+GO->big[guard-fail]");
    assert!(fail["payload"]["size"].as_i64().unwrap() <= 10, "{fail}");
    assert!(pass["payload"]["size"].as_i64().unwrap() > 10, "{pass}");
    assert_eq!(find(&steps, "INIT", "success", None).unwrap()["coverageKey"], "Runner:idle+INIT->idle");
}

#[test]
fn control_a_constant_guard_plans_only_its_drivable_case_and_an_optional_payload_plans_no_malformed() {
    let steps = variant_steps(&variants_program());
    assert!(find(&steps, "NEVER", "success", Some(0)).is_none(), "the always-false guard has no passing case");
    assert!(find(&steps, "NEVER", "guard-fail", Some(0)).is_some());
    assert!(find(&steps, "LEAVE", "malformed", None).is_none());
    assert!(find(&steps, "FETCH", "malformed", None).is_none());
}

#[test]
fn the_plan_carries_accept_states_and_navigation() {
    let steps = variant_steps(&variants_program());
    let fetch = find(&steps, "FETCH", "success", None).unwrap();
    assert_eq!(fetch["acceptStates"], json!(["loading", "ready"]));
    let loaded = find(&steps, "LOADED", "success", None).unwrap();
    assert_eq!(loaded["acceptStates"], json!(["loading", "ready"]), "the emitted event is fired by the effect: any state of the closure of `from`");
    assert!(find(&steps, "GO", "guard-fail", Some(0)).unwrap().get("acceptStates").is_none());
    assert_eq!(find(&steps, "LEAVE", "success", None).unwrap()["navigates"], true);
    assert_eq!(fetch["navigates"], true, "a listener's triggered arm navigates");
    assert_eq!(find(&steps, "GO", "success", Some(0)).unwrap()["navigates"], false);
}

#[test]
fn an_exported_credit_pass_credits_a_failure_route_from_another_dispatch_and_refuses_bad_input() {
    let hop = json!({ "trait": "Caller", "from": "waiting", "event": "CALL_FAILED", "to": "failed" });
    let steps = json!([
        { "trait": "Caller", "from": "idle", "event": "CALL", "fired": [hop] },
        { "trait": "Caller", "from": "waiting", "event": "CALL_FAILED", "reply": "failure", "fired": [] }
    ]);
    let credited = parsed(&verify_credit(&steps.to_string()).unwrap());
    assert_eq!(credited, json!([{ "index": 1, "stateBefore": "waiting", "stateAfter": "failed" }]));
    let no_hop = json!([{ "trait": "Caller", "from": "waiting", "event": "CALL_FAILED", "reply": "failure" }]);
    assert_eq!(parsed(&verify_credit(&no_hop.to_string()).unwrap()), json!([]));
    assert!(verify_credit("{not json").is_err());
}

#[test]
fn the_plan_carries_the_config_sweep_and_the_boot_credits() {
    let program = json!({
        "name": "App",
        "orbitals": [{ "name": "Main",
            "entity": { "name": "Job", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [{ "name": "Gauge", "linkedEntity": "Job",
                "config": { "animated": { "type": "boolean", "default": true }, "current": { "type": "number", "default": 0 } },
                "stateMachine": { "states": [{ "name": "idle", "isInitial": true }],
                    "transitions": [{ "from": "idle", "to": "idle", "event": "TICK" }] } }] }]
    });
    let plan = parsed(&verify_plan(&program.to_string(), ""));
    assert_eq!(plan["configVariants"][0]["variants"][0]["label"], "animated = false");
    assert_eq!(plan["configVariants"][0]["noDeclaredValues"], json!(["current"]));
    assert_eq!(plan["initCredits"], json!([{ "trait": "Gauge", "state": "idle", "coverageKey": "Gauge:idle+INIT->idle" }]));
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
            ["+", "@now", 1],
            ["str/upper", "@entity.name"],
            ["array/len", "@entity.tags"],
            ["<", "@entity.qty", "@config.limit"]
        ]),
    );
    let values: Vec<&Value> = out.iter().map(value_of).collect();
    assert_eq!(
        values,
        vec![&json!(5), &json!(true), &json!("open"), &json!("ar"), &json!(1001), &json!("ADA"), &json!(2), &json!(true)]
    );
}

#[test]
fn control_a_failing_expression_is_its_own_outcome_and_an_unbound_root_reads_null() {
    let context = json!({ "entity": { "qty": 3 } });
    let out = evaluated(&context, &json!([["+", 1, 2], ["/", 1, 0], "@user.id", ["+", 2, 2]]));
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
fn guards_are_decided_in_one_crossing_by_kernel_truthiness_and_a_failing_guard_is_its_own_error() {
    let context = json!({ "entity": { "qty": 3, "tags": [], "name": "" } }).to_string();
    let guards = json!([
        [">", "@entity.qty", 1],
        [">", "@entity.qty", 9],
        ["/", 1, 0],
        "@entity.tags",
        "@entity.name",
        "@entity.qty"
    ]);
    let verdicts = pj(&evaluate_guards(&context, &guards.to_string()).unwrap());
    assert_eq!(verdicts[0], json!({ "kind": "value", "value": true }));
    assert_eq!(verdicts[1], json!({ "kind": "value", "value": false }));
    assert_eq!(verdicts[2]["kind"], "error");
    assert_eq!(verdicts[3], json!({ "kind": "value", "value": false }));
    assert_eq!(verdicts[4], json!({ "kind": "value", "value": false }));
    assert_eq!(verdicts[5], json!({ "kind": "value", "value": true }));
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
    let failed = traced(json!({}), json!(["+", 1, ["/", 1, 0]]));
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
        &json!({ "rowId": "@payload.id", "double": ["*", "@payload.count", 2], "label": "fixed", "broken": ["/", 1, 0] }).to_string(),
        &payload.to_string(),
    )
    .unwrap();
    assert_eq!(pj(&mapped), json!({ "rowId": "r1", "double": 8, "label": "fixed" }));
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
    assert!(scoped(json!({ "id": "u1" }), &json!(["/", 1, 0]).to_string(), "").is_empty());
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
    assert_eq!(verdict("update", json!({ "id": "u1" }), &json!(["/", 1, 0]).to_string())["allowed"], false);
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
    assert_eq!(fold("@now"), json!({ "kind": "value", "value": 5 }));
    assert_eq!(fold("@user.teams"), json!({ "kind": "list", "values": ["x", "y"] }));
    assert_eq!(fold("@user.profile")["kind"], "inexpressible");
    assert_eq!(fold("@user.missing")["kind"], "unspecialised");
    assert_eq!(fold("@entity.id")["kind"], "inexpressible");
    assert_eq!(fold("plain")["kind"], "inexpressible");
}

#[test]
fn a_string_operator_reads_an_absent_value_as_empty_text_and_control_a_present_string_null_stays() {
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
            ["str/concat", "n=", "@payload.n"]
        ]),
    );
    let values: Vec<&Value> = out.iter().map(value_of).collect();
    assert_eq!(
        values,
        vec![&json!("ab"), &json!(""), &json!(0), &json!(""), &json!("anull"), &json!("NULL"), &json!("n=3")]
    );
}

#[test]
fn the_flow_planners_are_exported_with_the_plan_and_as_host_queries() {
    let program = json!({
        "name": "App",
        "orbitals": [{ "name": "Main",
            "entity": { "name": "Note", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }],
                        "create_policy": ["=", "@user.role", "admin"] },
            "auxiliaryEntities": [{ "name": "Member", "persistence": "runtime", "identity": true, "fields": [
                { "name": "id", "type": "string" }, { "name": "role", "type": "enum", "values": ["admin", "viewer"] }] }],
            "traits": [{ "name": "Flow", "linkedEntity": "Note",
                "stateMachine": { "states": [{ "name": "idle", "isInitial": true }, { "name": "saving" }, { "name": "saved" }, { "name": "failed" }],
                    "transitions": [
                        { "from": "idle", "to": "idle", "event": "INIT" },
                        { "from": "idle", "to": "saving", "event": "START", "effects": [
                            ["persist", "create", "Note", {}, { "emit": { "success": "DONE", "failure": "FAILED" } }],
                            ["render-ui", "main", { "type": "data-grid", "entity": "Note" }]] },
                        { "from": "saving", "to": "saved", "event": "DONE" },
                        { "from": "saving", "to": "failed", "event": "FAILED" }] } }] }]
    });
    let text = program.to_string();
    let plan = parsed(&verify_plan(&text, ""));
    assert_eq!(plan["emitSweep"][0]["coverageKey"], "Flow:idle+DONE->idle[emit]");
    assert_eq!(plan["contractEvents"]["available"], true);
    assert!(plan["contractEvents"]["steps"].is_array());
    assert!(plan["crudChains"].is_array());
    assert!(plan["tickSteps"].is_array() && plan["dataMutations"].is_array());
    assert_eq!(plan["dataMutations"][0]["coverageKey"], "Flow:idle+START->saving[data-mutation:create]");
    let go = parsed(&verify_affordance(&text, r#"{"trait":"Flow","from":"idle","event":"START"}"#, "").unwrap());
    assert_eq!((go["to"].as_str(), go["declared"].as_bool()), (Some("saving"), Some(true)));
    assert_eq!(parsed(&verify_affordance(&text, r#"{"trait":"Nobody","from":"idle","event":"START"}"#, "").unwrap()), Value::Null);
    let probe = parsed(&verify_transient_probe(&text, r#"{"trait":"Flow","from":"saving","event":"FAILED","to":"failed"}"#, "").unwrap());
    assert_eq!(probe["outcome"], "probe");
    assert_eq!(probe["step"]["role"], json!({ "field": "role", "value": "viewer" }));
    assert!(verify_affordance(&text, "{not json", "").is_err());
}

#[test]
fn an_exported_replay_places_a_fresh_machine_in_a_state_or_says_it_cannot() {
    let program = json!({
        "name": "App",
        "orbitals": [{ "name": "Main",
            "entity": { "name": "Job", "persistence": "runtime", "fields": [{ "name": "id", "type": "string" }] },
            "traits": [{ "name": "Flow", "linkedEntity": "Job",
                "stateMachine": { "states": [{ "name": "idle", "isInitial": true }, { "name": "busy" }],
                    "transitions": [{ "from": "idle", "to": "busy", "event": "GO" }] } }] }]
    })
    .to_string();
    let hops = parsed(&verify_replay(&program, r#"{"trait":"Flow","state":"busy"}"#, "").unwrap());
    assert_eq!((hops[0]["event"].as_str(), hops[0]["from"].as_str(), hops[0]["to"].as_str()), (Some("GO"), Some("idle"), Some("busy")));
    assert_eq!(parsed(&verify_replay(&program, r#"{"trait":"Flow","state":"gone"}"#, "").unwrap()), Value::Null);
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
