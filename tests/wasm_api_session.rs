//! The wasm-facing API (`KernelApi`, what `Kernel` forwards to) replays the embed scripted sessions
//! and must produce the C-ABI goldens recorded in orbital-core.

use orbital_wasm::api::KernelApi;
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
