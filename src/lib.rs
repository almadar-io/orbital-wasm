//! `orbital_core::embed` for JavaScript hosts. Marshalling only: every decision is the embedded kernel's.
//!
//! `api::KernelApi` is the host-facing surface over JSON strings; `Kernel` is its wasm-bindgen
//! class. A failed operation throws a JS `Error` carrying the message the C ABI puts in
//! `last_error`.

pub mod api;
pub mod http;

use api::{KernelApi, ServerApi};

use wasm_bindgen::prelude::*;

fn js(message: String) -> JsError {
    JsError::new(&message)
}

#[wasm_bindgen]
pub struct Kernel {
    api: KernelApi,
}

#[wasm_bindgen]
impl Kernel {
    #[wasm_bindgen(constructor)]
    pub fn new(resolved_schema_json: &str, client_id: &str) -> Result<Kernel, JsError> {
        KernelApi::new(resolved_schema_json, client_id)
            .map(|api| Kernel { api })
            .map_err(js)
    }

    pub fn dispatch(
        &self,
        trait_name: &str,
        entity_id: Option<String>,
        event: &str,
        payload_json: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .dispatch(
                trait_name,
                entity_id.as_deref(),
                event,
                payload_json.as_deref(),
            )
            .map_err(js)
    }

    #[wasm_bindgen(js_name = applyServerResponse)]
    pub fn apply_server_response(
        &self,
        response_json: &str,
        already_delivered_json: Option<String>,
        base_json: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .apply_server_response(
                response_json,
                already_delivered_json.as_deref(),
                base_json.as_deref(),
            )
            .map_err(js)
    }

    /// `undefined` when the leg carried no rows to fold against.
    #[wasm_bindgen(js_name = foldBase)]
    pub fn fold_base(
        &self,
        carried_json: Option<String>,
        seeded_json: &str,
        entity_id: Option<String>,
    ) -> Result<Option<String>, JsError> {
        self.api
            .fold_base(carried_json.as_deref(), seeded_json, entity_id.as_deref())
            .map_err(js)
    }

    #[wasm_bindgen(js_name = dispatchMount)]
    pub fn dispatch_mount(
        &self,
        seeds_json: &str,
        entity_id: Option<String>,
        payload_json: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .dispatch_mount(seeds_json, entity_id.as_deref(), payload_json.as_deref())
            .map_err(js)
    }

    pub fn restore(
        &self,
        trait_name: &str,
        entity_id: Option<String>,
        snapshot_json: &str,
    ) -> Result<String, JsError> {
        self.api
            .restore(trait_name, entity_id.as_deref(), snapshot_json)
            .map_err(js)
    }

    pub fn tick(
        &self,
        trait_name: &str,
        tick_name: &str,
        entity_id: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .tick(trait_name, tick_name, entity_id.as_deref())
            .map_err(js)
    }

    /// `undefined` when no tick is due this frame.
    pub fn frame(&self, now_ms: f64) -> Result<Option<String>, JsError> {
        self.api.frame(now_ms as u64).map_err(js)
    }

    #[wasm_bindgen(js_name = framePause)]
    pub fn frame_pause(&self) {
        self.api.frame_pause();
    }

    #[wasm_bindgen(js_name = frameResume)]
    pub fn frame_resume(&self) {
        self.api.frame_resume();
    }

    #[wasm_bindgen(js_name = tickCount)]
    pub fn tick_count(&self) -> u32 {
        u32::try_from(self.api.tick_count()).unwrap_or(u32::MAX)
    }

    #[wasm_bindgen(js_name = getState)]
    pub fn get_state(&self, trait_name: &str, entity_id: Option<String>) -> String {
        self.api.get_state(trait_name, entity_id.as_deref())
    }

    pub fn snapshot(&self) -> Result<String, JsError> {
        self.api.snapshot().map_err(js)
    }

    #[wasm_bindgen(js_name = setThemeMode)]
    pub fn set_theme_mode(&self, mode: &str) -> Result<(), JsError> {
        self.api.set_theme_mode(mode).map_err(js)
    }

    #[wasm_bindgen(js_name = setCurrentUser)]
    pub fn set_current_user(&self, user_json: &str) -> Result<(), JsError> {
        self.api.set_current_user(user_json).map_err(js)
    }

    /// The host runs the listeners of every emit it marks `dispatched` (the JS runtime, `orbital-server`).
    #[wasm_bindgen(js_name = setHostFansOut)]
    pub fn set_host_fans_out(&self, fans_out: bool) {
        self.api.set_host_fans_out(fans_out);
    }

    /// A row-set reference resolved against this kernel's rows for its current user (`{ rows, total }`).
    #[wasm_bindgen(js_name = resolveRows)]
    pub fn resolve_rows(&self, ref_json: &str) -> Result<String, JsError> {
        self.api.resolve_rows(ref_json).map_err(js)
    }

    pub fn seed(&self, entity_type: &str, rows_json: &str) -> Result<(), JsError> {
        self.api.seed(entity_type, rows_json).map_err(js)
    }

    pub fn remount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.remount(traits_json).map_err(js)
    }

    pub fn mount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.mount(traits_json).map_err(js)
    }

    pub fn unmount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.unmount(traits_json).map_err(js)
    }

    #[wasm_bindgen(js_name = lastError)]
    pub fn last_error(&self) -> String {
        self.api.last_error()
    }
}

/// The server role: `start` a request, answer the I/O it asks for, `resume` the flight until it is done.
#[wasm_bindgen]
pub struct Server {
    api: ServerApi,
}

fn flight_id(flight: f64) -> Result<u64, JsError> {
    if flight.is_finite()
        && flight >= 0.0
        && flight.fract() == 0.0
        && flight <= 9_007_199_254_740_991.0
    {
        Ok(flight as u64)
    } else {
        Err(JsError::new(&format!("{flight} is not a flight id")))
    }
}

#[wasm_bindgen]
impl Server {
    #[wasm_bindgen(constructor)]
    pub fn new(resolved_schema_json: &str) -> Result<Server, JsError> {
        ServerApi::new(resolved_schema_json)
            .map(|api| Server { api })
            .map_err(js)
    }

    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, JsError> {
        self.api.start(orbital, request_json).map_err(js)
    }

    pub fn resume(&self, flight: f64, replies_json: &str) -> Result<String, JsError> {
        self.api
            .resume(flight_id(flight)?, replies_json)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = resolveRows)]
    pub fn resolve_rows(&self, ref_json: &str, viewer_json: &str) -> Result<String, JsError> {
        self.api.resolve_rows(ref_json, viewer_json).map_err(js)
    }

    #[wasm_bindgen(js_name = resumeResolve)]
    pub fn resume_resolve(&self, flight: f64, replies_json: &str) -> Result<String, JsError> {
        self.api
            .resume_resolve(flight_id(flight)?, replies_json)
            .map_err(js)
    }

    pub fn abandon(&self, flight: f64) -> Result<bool, JsError> {
        Ok(self.api.abandon(flight_id(flight)?))
    }

    #[wasm_bindgen(js_name = inFlight)]
    pub fn in_flight(&self) -> u32 {
        u32::try_from(self.api.in_flight()).unwrap_or(u32::MAX)
    }

    #[wasm_bindgen(js_name = getState)]
    pub fn get_state(&self, trait_name: &str, entity_id: Option<String>) -> String {
        self.api.get_state(trait_name, entity_id.as_deref())
    }

    #[wasm_bindgen(js_name = setDefaultUser)]
    pub fn set_default_user(&self, user_json: &str) -> Result<(), JsError> {
        self.api.set_default_user(user_json).map_err(js)
    }

    pub fn mount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.mount(traits_json).map_err(js)
    }

    pub fn remount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.remount(traits_json).map_err(js)
    }

    #[wasm_bindgen(js_name = setMessages)]
    pub fn set_messages(&self, locales_json: &str, catalogs_json: &str) -> Result<(), JsError> {
        self.api
            .set_messages(locales_json, catalogs_json)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = setCapabilities)]
    pub fn set_capabilities(
        &self,
        entity_type: &str,
        query_json: &str,
        write_json: &str,
    ) -> Result<(), JsError> {
        self.api
            .set_capabilities(entity_type, query_json, write_json)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = setTickMount)]
    pub fn set_tick_mount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.set_tick_mount(traits_json).map_err(js)
    }

    #[wasm_bindgen(js_name = lastError)]
    pub fn last_error(&self) -> String {
        self.api.last_error()
    }
}

/// Install the language data a host assembles from its installed `@almadar/core` and `@almadar/std`
/// (`readLanguageBundle`): the operator table that routes `program/*` and the other host effects.
#[wasm_bindgen(js_name = installLanguage)]
pub fn install_language(bundle_json: &str) -> Result<(), JsError> {
    api::install_language(bundle_json).map_err(js)
}

/// The `http` effect's sans-I/O driver (`orbital_core::runtime::http_request`) over JSON strings; each
/// returns what `http::*` returns, or throws a JS `Error` for a malformed argument.
#[wasm_bindgen(js_name = httpDriverStart)]
pub fn http_driver_start(call_json: &str, egress_json: &str) -> Result<String, JsError> {
    http::http_driver_start(call_json, egress_json).map_err(js)
}

#[wasm_bindgen(js_name = httpDriverResume)]
pub fn http_driver_resume(driver_json: &str, answer_json: &str) -> Result<String, JsError> {
    http::http_driver_resume(driver_json, answer_json).map_err(js)
}

#[wasm_bindgen(js_name = isPublicAddress)]
pub fn is_public_address(ip: &str) -> Result<bool, JsError> {
    http::is_public_address(ip).map_err(js)
}

#[wasm_bindgen(js_name = frameStream)]
pub fn frame_stream(framing: &str, key: Option<String>, chunk: &str, carry: &str) -> Result<String, JsError> {
    http::frame_stream(framing, key.as_deref(), chunk, carry).map_err(js)
}

#[wasm_bindgen(js_name = frameStreamFinish)]
pub fn frame_stream_finish(framing: &str, key: Option<String>, carry: &str) -> Result<String, JsError> {
    http::frame_stream_finish(framing, key.as_deref(), carry).map_err(js)
}

#[wasm_bindgen(js_name = httpDriverDrop)]
pub fn http_driver_drop(driver_json: &str) -> Result<(), JsError> {
    http::http_driver_drop(driver_json).map_err(js)
}

#[wasm_bindgen(js_name = streamEvents)]
pub fn stream_events(driver_json: &str, chunk: &str, carry: &str) -> Result<String, JsError> {
    http::stream_events(driver_json, chunk, carry).map_err(js)
}

#[wasm_bindgen(js_name = streamFinish)]
pub fn stream_finish(driver_json: &str, carry: &str) -> Result<String, JsError> {
    http::stream_finish(driver_json, carry).map_err(js)
}
