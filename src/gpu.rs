//! The `gpu/*` answerer for a browser host: one `GpuHost` per program instance, over orbital-canvas's handle table and
//! the page's WebGPU device. Marshalling only: args and answers use the os wire (`Value` JSON, a `bytes` argument is
//! standard padded base64 text; an answer is `Settled<Value, OsError>`).

use orbital_core::embed::wire::Settled;
use orbital_core::js_readable::to_js_readable_json;
use orbital_core::runtime::os::{OsError, OsErrorKind, OsOp};
use orbital_core::Value;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct GpuHost {
    inner: orbital_canvas::GpuHost,
}

impl Default for GpuHost {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid(message: String) -> OsError {
    OsError::new(OsErrorKind::Invalid, message)
}

fn args_of(args: &JsValue) -> Result<Vec<Value>, OsError> {
    let text = js_sys::JSON::stringify(args)
        .ok()
        .and_then(|text| text.as_string())
        .ok_or_else(|| invalid("gpu: the arguments are not JSON".to_string()))?;
    serde_json::from_str(&text)
        .map_err(|e| invalid(format!("gpu: the arguments are not a list of values: {e}")))
}

#[wasm_bindgen]
impl GpuHost {
    #[wasm_bindgen(constructor)]
    pub fn new() -> GpuHost {
        GpuHost {
            inner: orbital_canvas::GpuHost::new(),
        }
    }

    /// Answer one `gpu/*` ask; resolves to `{ ok: <value> }` or `{ error: { kind, message } }`.
    pub async fn answer(&self, op: &str, args: JsValue) -> Result<JsValue, JsValue> {
        let reply = match (OsOp::from_name(op), args_of(&args)) {
            (None, _) => Err(invalid(format!("{op} is not an os operation"))),
            (_, Err(error)) => Err(error),
            (Some(op), Ok(args)) => self.inner.answer_async(op, &args).await,
        };
        let json = to_js_readable_json(&Settled::from(reply)).map_err(|e| JsValue::from_str(&e))?;
        js_sys::JSON::parse(&json)
    }

    #[wasm_bindgen(js_name = closeAll)]
    pub fn close_all(&self) {
        self.inner.close_all();
    }
}
