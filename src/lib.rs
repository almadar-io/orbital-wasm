//! `orbital_core::embed` for JavaScript hosts. Marshalling only: every decision is the embedded kernel's.
//!
//! `api::KernelApi` is the host-facing surface over JSON strings; `Kernel` is its wasm-bindgen
//! class. A failed operation throws a JS `Error` carrying the message the C ABI puts in
//! `last_error`.

pub mod api;
pub mod credentials;
pub mod gpu;
pub mod stored;

use api::{KernelApi, ServerApi};

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

#[wasm_bindgen(start)]
fn report_panics() {
    std::panic::set_hook(Box::new(|info| console_error(&info.to_string())));
}

fn js(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

#[wasm_bindgen]
pub struct Kernel {
    api: std::rc::Rc<KernelApi>,
    /// The GPU answerer the host installed and where each held ask's completion goes (`installGpu`).
    gpu: std::cell::RefCell<Option<(std::rc::Rc<orbital_canvas::GpuHost>, js_sys::Function)>>,
    /// Whether a drain of the held `gpu/` asks is running.
    draining: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Kernel {
    /// Start answering the held `gpu/` asks, one at a time in ask order, unless a drain is already running.
    fn pump(&self) {
        let Some((gpu, on_completion)) = self.gpu.borrow().clone() else {
            return;
        };
        if self.draining.get() {
            return;
        }
        self.draining.set(true);
        wasm_bindgen_futures::spawn_local(drain_client_gpu(self.api.clone(), gpu, on_completion, self.draining.clone()));
    }

    fn pumped<T>(&self, out: T) -> T {
        self.pump();
        out
    }
}

/// Answer the client kernel's held `gpu/` asks in ask order until none is left (EXEC-GPU-5). Each outcome's route event
/// is handed to `on_completion` as `{ orbital?, traitName, entityId?, event, heldPayload }` for the host to dispatch with
/// `dispatchHeld`, or `null` when the outcome takes no declared route.
async fn drain_client_gpu(
    api: std::rc::Rc<KernelApi>,
    gpu: std::rc::Rc<orbital_canvas::GpuHost>,
    on_completion: js_sys::Function,
    draining: std::rc::Rc<std::cell::Cell<bool>>,
) {
    while let Some(ask) = api.embedded().take_gpu_ask() {
        let outcome = gpu.answer_async(ask.op, &ask.args).await;
        let done = api.embedded().gpu_completion(&ask, outcome);
        let delivered = orbital_core::js_readable::to_js_readable_json(&done)
            .map_err(|e| JsValue::from_str(&e))
            .and_then(|json| on_completion.call1(&JsValue::NULL, &JsValue::from_str(&json)));
        if let Err(error) = delivered {
            if let Some(done) = &done {
                api.embedded().drop_held_payload(done.held_payload);
            }
            console_error(&format!("{}: the completion could not be delivered: {error:?}", ask.op.name()));
        }
    }
    draining.set(false);
}

#[wasm_bindgen]
impl Kernel {
    #[wasm_bindgen(constructor)]
    pub fn new(resolved_schema_json: &str, client_id: &str) -> Result<Kernel, JsError> {
        KernelApi::new(resolved_schema_json, client_id)
            .map(|api| Kernel {
                api: std::rc::Rc::new(api),
                gpu: std::cell::RefCell::new(None),
                draining: std::rc::Rc::new(std::cell::Cell::new(false)),
            })
            .map_err(js)
    }

    /// Install the GPU answerer: from now on every `gpu/` ask is held in the kernel and answered here in ask order, and
    /// each outcome's route event is handed to `onCompletion` (see [`drain_client_gpu`]) for the host to dispatch with
    /// `dispatchHeld`, so no handle or byte crosses to JavaScript.
    #[wasm_bindgen(js_name = installGpu)]
    pub fn install_gpu(&self, gpu: &gpu::GpuHost, on_completion: js_sys::Function) {
        *self.gpu.borrow_mut() = Some((gpu.inner.clone(), on_completion));
        self.api.embedded().hold_gpu();
        self.pump();
    }

    /// Dispatch a held completion: `event` on `traitName` with the payload held under `heldPayload`, taken once.
    #[wasm_bindgen(js_name = dispatchHeld)]
    pub fn dispatch_held(
        &self,
        trait_name: &str,
        entity_id: Option<String>,
        event: &str,
        held_payload: f64,
    ) -> Result<String, JsError> {
        let ticket = flight_id(held_payload)?;
        self.pumped(self.api.dispatch_held(trait_name, entity_id.as_deref(), event, ticket).map_err(js))
    }

    /// Drop a held completion payload no dispatch will take (the host could not deliver it).
    #[wasm_bindgen(js_name = dropHeldPayload)]
    pub fn drop_held_payload(&self, held_payload: f64) -> Result<bool, JsError> {
        Ok(self.api.embedded().drop_held_payload(flight_id(held_payload)?))
    }

    pub fn dispatch(
        &self,
        trait_name: &str,
        entity_id: Option<String>,
        event: &str,
        payload_json: Option<String>,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .dispatch(
                trait_name,
                entity_id.as_deref(),
                event,
                payload_json.as_deref(),
            )
            .map_err(js))
    }

    #[wasm_bindgen(js_name = applyServerResponse)]
    pub fn apply_server_response(
        &self,
        response_json: &str,
        already_delivered_json: Option<String>,
        base_json: Option<String>,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .apply_server_response(
                response_json,
                already_delivered_json.as_deref(),
                base_json.as_deref(),
            )
            .map_err(js))
    }

    #[wasm_bindgen(js_name = openFlight)]
    pub fn open_flight(&self) -> f64 {
        self.api.open_flight() as f64
    }

    #[wasm_bindgen(js_name = foldInOrder)]
    pub fn fold_in_order(
        &self,
        flight: f64,
        response_json: &str,
        already_delivered_json: Option<String>,
        base_json: Option<String>,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .fold_in_order(
                flight as u64,
                response_json,
                already_delivered_json.as_deref(),
                base_json.as_deref(),
            )
            .map_err(js))
    }

    #[wasm_bindgen(js_name = deliverEmitToListeners)]
    pub fn deliver_emit_to_listeners(
        &self,
        flight: f64,
        emitted_json: &str,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .deliver_emit_to_listeners(flight as u64, emitted_json)
            .map_err(js))
    }

    #[wasm_bindgen(js_name = deliverOriginEmit)]
    pub fn deliver_origin_emit(&self, flight: f64, emitted_json: &str) -> Result<String, JsError> {
        self.pumped(self.api
            .deliver_origin_emit(flight as u64, emitted_json)
            .map_err(js))
    }

    #[wasm_bindgen(js_name = nextFold)]
    pub fn next_fold(&self) -> Result<Option<String>, JsError> {
        self.pumped(self.api.next_fold().map_err(js))
    }

    #[wasm_bindgen(js_name = endFlight)]
    pub fn end_flight(&self, flight: f64) -> bool {
        self.api.end_flight(flight as u64)
    }

    #[wasm_bindgen(js_name = framesSince)]
    pub fn frames_since(&self, since: f64) -> Result<String, JsError> {
        self.api.frames_since(since as u64).map_err(js)
    }

    #[wasm_bindgen(js_name = statefulLeg)]
    pub fn stateful_leg(&self, leg_json: &str) -> Result<String, JsError> {
        self.api.stateful_leg(leg_json).map_err(js)
    }

    #[wasm_bindgen(js_name = reseedLeg)]
    pub fn reseed_leg(
        &self,
        seeds_json: &str,
        entity_id: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .reseed_leg(seeds_json, entity_id.as_deref())
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
        route: Option<String>,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .dispatch_mount(seeds_json, entity_id.as_deref(), payload_json.as_deref(), route.as_deref())
            .map_err(js))
    }

    pub fn restore(
        &self,
        trait_name: &str,
        entity_id: Option<String>,
        snapshot_json: &str,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .restore(trait_name, entity_id.as_deref(), snapshot_json)
            .map_err(js))
    }

    pub fn tick(
        &self,
        trait_name: &str,
        tick_name: &str,
        entity_id: Option<String>,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .tick(trait_name, tick_name, entity_id.as_deref())
            .map_err(js))
    }

    /// `undefined` when no tick is due this frame.
    pub fn frame(&self, now_ms: f64) -> Result<Option<String>, JsError> {
        self.pumped(self.api.frame(now_ms as u64).map_err(js))
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

    /// The theme key bound to this client's session (`theme/set`, mirrored from the server's frames), or `undefined`.
    #[wasm_bindgen(js_name = sessionTheme)]
    pub fn session_theme(&self) -> Option<String> {
        self.api.session_theme()
    }

    /// The config the kernel binds as `@config` for a registered trait, as JSON text.
    #[wasm_bindgen(js_name = traitConfig)]
    pub fn trait_config(&self, trait_name: &str) -> Result<String, JsError> {
        self.api.trait_config(trait_name).map_err(js)
    }

    /// The kernel's dispatch mode for a registered trait, as JSON text.
    #[wasm_bindgen(js_name = dispatchMode)]
    pub fn dispatch_mode(&self, trait_name: &str) -> Result<String, JsError> {
        self.api.dispatch_mode(trait_name).map_err(js)
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

    /// The host runs the listeners of every emit it marks `dispatched` (the JS runtime, `orbital-server`).
    #[wasm_bindgen(js_name = setHostFansOut)]
    pub fn set_host_fans_out(&self, fans_out: bool) {
        self.api.set_host_fans_out(fans_out);
    }

    /// Record each dispatch as a trace item while on, for the host's `program/live { kind: "trace" }` streams.
    #[wasm_bindgen(js_name = setTraceOn)]
    pub fn set_trace_on(&self, on: bool) {
        self.api.set_trace_on(on);
    }

    /// The trace items recorded since the last take, in dispatch order: a JSON array.
    #[wasm_bindgen(js_name = takeTrace)]
    pub fn take_trace(&self) -> Result<String, JsError> {
        self.api.take_trace().map_err(js)
    }

    /// Carry each hop's `@entity` before and after it on the dispatch's transitions (`observed`), for a verifier
    /// judging `@ensures`. Off by default (EXEC-STEP-10).
    #[wasm_bindgen(js_name = setRecordEntityAfter)]
    pub fn set_record_entity_after(&self, record: bool) {
        self.api.set_record_entity_after(record);
    }

    /// Carry each tick's `effectResults` on the frame response, for a verifier. Off by default (P-015).
    #[wasm_bindgen(js_name = setFrameEffectResults)]
    pub fn set_frame_effect_results(&self, carry: bool) {
        self.api.set_frame_effect_results(carry);
    }

    /// A row-set reference resolved against this kernel's rows for its current user (`{ rows, total }`).
    #[wasm_bindgen(js_name = resolveRows)]
    pub fn resolve_rows(&self, ref_json: &str) -> Result<String, JsError> {
        self.api.resolve_rows(ref_json).map_err(js)
    }

    pub fn seed(&self, entity_type: &str, rows_json: &str) -> Result<(), JsError> {
        self.api.seed(entity_type, rows_json).map_err(js)
    }

    pub fn remount(&self, traits_json: &str) -> Result<String, JsError> {
        self.pumped(self.api.remount(traits_json).map_err(js))
    }

    pub fn enter(
        &self,
        route_path: Option<String>,
        mount_key: &str,
        traits_json: &str,
    ) -> Result<String, JsError> {
        self.pumped(self.api
            .enter(route_path.as_deref(), mount_key, traits_json)
            .map_err(js))
    }

    pub fn leave(&self) -> Result<String, JsError> {
        self.api.leave().map_err(js)
    }

    /// The page's locale `i18n/t` paints in when a dispatch names none; the program carries its own messages.
    #[wasm_bindgen(js_name = setViewerLocale)]
    pub fn set_viewer_locale(&self, locale: Option<String>) -> Result<(), JsError> {
        self.api.set_viewer_locale(locale).map_err(js)
    }

    pub fn mount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.mount(traits_json).map_err(js)
    }

    #[wasm_bindgen(js_name = bootMounts)]
    pub fn boot_mounts(&self, side: &str) -> Result<String, JsError> {
        self.api.boot_mounts(side).map_err(js)
    }

    #[wasm_bindgen(js_name = routeMounts)]
    pub fn route_mounts(&self, route_path: &str) -> Result<String, JsError> {
        self.api.route_mounts(route_path).map_err(js)
    }

    #[wasm_bindgen(js_name = pageCircuit)]
    pub fn page_circuit(
        &self,
        page: Option<String>,
        route: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .page_circuit(page.as_deref(), route.as_deref())
            .map_err(js)
    }

    #[wasm_bindgen(js_name = contractQuery)]
    pub fn contract_query(&self, query_json: &str) -> Result<String, JsError> {
        self.api.contract_query(query_json).map_err(js)
    }

    pub fn unmount(&self, traits_json: &str) -> Result<String, JsError> {
        self.api.unmount(traits_json).map_err(js)
    }

    /// The host's report of whether a trait's rendered output is on screen; gates its `[visible]` ticks.
    #[wasm_bindgen(js_name = setTraitOnScreen)]
    pub fn set_trait_on_screen(&self, trait_name: &str, on_screen: bool) {
        self.api.set_trait_on_screen(trait_name, on_screen);
    }

    #[wasm_bindgen(js_name = lastError)]
    pub fn last_error(&self) -> String {
        self.api.last_error()
    }
}

/// The server role: `start` a request, answer the I/O it asks for, `resume` the flight until it is done.
#[wasm_bindgen]
pub struct Server {
    api: std::rc::Rc<ServerApi>,
    /// The GPU answerer the host installed (`installGpu`): the server then holds every `gpu/` ask for it.
    gpu: std::cell::RefCell<Option<std::rc::Rc<orbital_canvas::GpuHost>>>,
}

impl Server {
    fn of(api: ServerApi, gpu: Option<std::rc::Rc<orbital_canvas::GpuHost>>) -> Server {
        if gpu.is_some() {
            api.embedded().hold_gpu();
        }
        Server { api: std::rc::Rc::new(api), gpu: std::cell::RefCell::new(gpu) }
    }
}

/// Answer the held `gpu/` ask `ticket` with the installed answerer. A deferring ask under a route runs in the
/// background and answers `serviceDeferred`; its completion is held in the server and handed to `on_completion` as
/// `{ orbital?, traitName, entityId?, event, heldPayload }`, or `null` when the outcome takes no declared route. Any other ask answers `gpuHeld` with the ticket its
/// outcome is held under.
async fn answer_held(
    api: std::rc::Rc<ServerApi>,
    gpu: std::rc::Rc<orbital_canvas::GpuHost>,
    ticket: u64,
    on_completion: js_sys::Function,
) -> orbital_core::embed::wire::IoAnswerWire {
    use orbital_core::embed::wire::{IoAnswerWire, Settled};
    use orbital_core::runtime::os::{OsError, OsErrorKind};
    let Some(ask) = api.embedded().take_gpu_ask(ticket) else {
        let missing: Result<orbital_core::Value, OsError> =
            Err(OsError::new(OsErrorKind::Invalid, format!("no gpu ask is held as {ticket}")));
        return IoAnswerWire::Os { result: Settled::from(missing) };
    };
    if ask.op.defers_completion() && ask.key.is_some() && ask.site.is_some() {
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = gpu.answer_async(ask.op, &ask.args).await;
            let done = api.embedded().gpu_completion(&ask, outcome);
            let delivered = orbital_core::js_readable::to_js_readable_json(&done)
                .map_err(|e| JsValue::from_str(&e))
                .and_then(|json| on_completion.call1(&JsValue::NULL, &JsValue::from_str(&json)));
            if let Err(error) = delivered {
                if let Some(done) = &done {
                    api.embedded().drop_held_payload(done.held_payload);
                }
                console_error(&format!("{}: the completion could not be delivered: {error:?}", ask.op.name()));
            }
        });
        return IoAnswerWire::ServiceDeferred;
    }
    let outcome = gpu.answer_async(ask.op, &ask.args).await;
    IoAnswerWire::GpuHeld { ticket: api.embedded().hold_gpu_answer(outcome) }
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
            .map(|api| Server::of(api, None))
            .map_err(js)
    }

    /// A server holding this one's registered program and none of its session: what a stateless host serves
    /// each request from, instead of registering the program again.
    pub fn fork(&self) -> Result<Server, JsError> {
        let gpu = self.gpu.borrow().clone();
        self.api.fork().map(|api| Server::of(api, gpu)).map_err(js)
    }

    #[wasm_bindgen(js_name = forkProgram)]
    pub fn fork_program(&self, program_json: &str) -> Result<Server, JsError> {
        self.api
            .fork_program(program_json)
            .map(|api| Server::of(api, self.gpu.borrow().clone()))
            .map_err(js)
    }

    /// Install the GPU answerer: from now on the server holds every `gpu/` ask, and the host answers it by ticket
    /// with `answerGpu`, so no argument or answer crosses to JavaScript.
    #[wasm_bindgen(js_name = installGpu)]
    pub fn install_gpu(&self, gpu: &gpu::GpuHost) {
        *self.gpu.borrow_mut() = Some(gpu.inner.clone());
        self.api.embedded().hold_gpu();
    }

    /// Answer the held `gpu/` ask `ticket` (an `IoRequestWire` of kind `gpu`): resolves to the `IoAnswerWire` JSON to
    /// reply with. A deferred completion is handed to `onCompletion` (see [`answer_held`]).
    #[wasm_bindgen(js_name = answerGpu)]
    pub fn answer_gpu(&self, ticket: f64, on_completion: js_sys::Function) -> Result<js_sys::Promise, JsError> {
        let ticket = flight_id(ticket)?;
        let gpu = self
            .gpu
            .borrow()
            .clone()
            .ok_or_else(|| JsError::new("answerGpu: no gpu answerer is installed"))?;
        let api = self.api.clone();
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let answer = answer_held(api, gpu, ticket, on_completion).await;
            orbital_core::js_readable::to_js_readable_json(&answer)
                .map(|json| JsValue::from_str(&json))
                .map_err(|e| JsValue::from_str(&e))
        }))
    }

    /// Drop a held completion payload the host will not start (its call was cancelled).
    #[wasm_bindgen(js_name = dropHeldPayload)]
    pub fn drop_held_payload(&self, ticket: f64) -> Result<bool, JsError> {
        Ok(self.api.embedded().drop_held_payload(flight_id(ticket)?))
    }

    #[wasm_bindgen(js_name = osAllows)]
    pub fn os_allows(
        &self,
        requirement_json: &str,
        orbital: Option<String>,
    ) -> Result<bool, JsError> {
        self.api
            .os_allows(requirement_json, orbital.as_deref())
            .map_err(js)
    }

    #[wasm_bindgen(js_name = egressAllows)]
    pub fn egress_allows(&self, url: &str) -> bool {
        self.api.egress_allows(url)
    }

    #[wasm_bindgen(js_name = setOsAnchors)]
    pub fn set_os_anchors(&self, anchors_json: &str) -> Result<String, JsError> {
        self.api.set_os_anchors(anchors_json).map_err(js)
    }

    #[wasm_bindgen(js_name = osResolveArgs)]
    pub fn os_resolve_args(&self, op: &str, args_json: &str) -> Result<String, JsError> {
        self.api.os_resolve_args(op, args_json).map_err(js)
    }

    /// Feed a deferred raw `os/` outcome into the TLS operation the kernel parked under `key`: `null` when `key` parks
    /// none, `{ ask: { op, args } }` for the next raw ask the host must answer, or `{ done: Settled }`.
    #[wasm_bindgen(js_name = tlsComplete)]
    pub fn tls_complete(&self, key: &str, outcome_json: &str) -> Result<String, JsError> {
        self.api.tls_complete(key, outcome_json).map_err(js)
    }

    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, JsError> {
        self.api.start(orbital, request_json).map_err(js)
    }

    pub fn resume(&self, flight: f64, replies_json: &str) -> Result<String, JsError> {
        self.api
            .resume(flight_id(flight)?, replies_json)
            .map_err(js)
    }

    /// The oldest queued request, started now the lane is free (`NextWire` JSON), or `null`.
    pub fn next(&self) -> Result<String, JsError> {
        self.api.next().map_err(js)
    }

    #[wasm_bindgen(js_name = holdsLane)]
    pub fn holds_lane(&self, flight: f64) -> Result<bool, JsError> {
        Ok(self.api.holds_lane(flight_id(flight)?))
    }

    /// A live stream of `client`'s session opened (EXEC-DIST-9); the id closes it.
    #[wasm_bindgen(js_name = openSessionStream)]
    pub fn open_session_stream(&self, client: &str) -> f64 {
        self.api.open_session_stream(client) as f64
    }

    #[wasm_bindgen(js_name = closeSessionStream)]
    pub fn close_session_stream(&self, id: f64) -> Result<bool, JsError> {
        Ok(self.api.close_session_stream(flight_id(id)?))
    }

    #[wasm_bindgen(js_name = liveItemDelivered)]
    pub fn live_item_delivered(
        &self,
        item_json: &str,
        client_id: Option<String>,
    ) -> Result<bool, JsError> {
        self.api
            .live_item_delivered(item_json, client_id.as_deref())
            .map_err(js)
    }

    #[wasm_bindgen(js_name = dispatchStream)]
    pub fn dispatch_stream(&self) -> Result<String, JsError> {
        self.api.dispatch_stream().map_err(js)
    }

    #[wasm_bindgen(js_name = resolveRows)]
    pub fn resolve_rows(
        &self,
        ref_json: &str,
        client_id: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .resolve_rows(ref_json, client_id.as_deref())
            .map_err(js)
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

    #[wasm_bindgen(js_name = bootMounts)]
    pub fn boot_mounts(&self, side: &str) -> Result<String, JsError> {
        self.api.boot_mounts(side).map_err(js)
    }

    #[wasm_bindgen(js_name = routeMounts)]
    pub fn route_mounts(&self, route_path: &str) -> Result<String, JsError> {
        self.api.route_mounts(route_path).map_err(js)
    }

    #[wasm_bindgen(js_name = pageCircuit)]
    pub fn page_circuit(
        &self,
        page: Option<String>,
        route: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .page_circuit(page.as_deref(), route.as_deref())
            .map_err(js)
    }

    /// One question about the registered program (a `ContractQuery` as JSON): its answer as JSON.
    #[wasm_bindgen(js_name = contractQuery)]
    pub fn contract_query(&self, query_json: &str) -> Result<String, JsError> {
        self.api.contract_query(query_json).map_err(js)
    }

    #[wasm_bindgen(js_name = setManagedProgram)]
    pub fn set_managed_program(&self, program_json: &str) -> Result<(), JsError> {
        self.api.set_managed_program(program_json).map_err(js)
    }

    pub fn mount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.mount(traits_json).map_err(js)
    }

    pub fn remount(&self, traits_json: &str) -> Result<(), JsError> {
        self.api.remount(traits_json).map_err(js)
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

    #[wasm_bindgen(js_name = mountClient)]
    pub fn mount_client(
        &self,
        client: &str,
        orbital: &str,
        traits_json: &str,
    ) -> Result<(), JsError> {
        self.api
            .mount_client(client, orbital, traits_json)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = unmountClient)]
    pub fn unmount_client(
        &self,
        client: &str,
        orbital: &str,
        traits_json: &str,
    ) -> Result<(), JsError> {
        self.api
            .unmount_client(client, orbital, traits_json)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = releaseClient)]
    pub fn release_client(&self, client: &str) {
        self.api.release_client(client);
    }

    #[wasm_bindgen(js_name = relayTick)]
    pub fn relay_tick(&self, orbital: &str, request_json: &str) -> Result<String, JsError> {
        self.api.relay_tick(orbital, request_json).map_err(js)
    }

    #[wasm_bindgen(js_name = setTickRelayInterval)]
    pub fn set_tick_relay_interval(&self, interval_ms: f64) {
        self.api
            .set_tick_relay_interval(interval_ms.max(0.0) as u64);
    }

    #[wasm_bindgen(js_name = flushTickRelay)]
    pub fn flush_tick_relay(&self) -> Result<String, JsError> {
        self.api.flush_tick_relay().map_err(js)
    }

    #[wasm_bindgen(js_name = tickPass)]
    pub fn tick_pass(&self, now_ms: f64) -> Result<String, JsError> {
        self.api.tick_pass(now_ms.max(0.0) as u64).map_err(js)
    }

    #[wasm_bindgen(js_name = setStateful)]
    pub fn set_stateful(&self, stateful: bool) {
        self.api.set_stateful(stateful);
    }

    #[wasm_bindgen(js_name = declaresTicks)]
    pub fn declares_ticks(&self) -> bool {
        self.api.declares_ticks()
    }

    #[wasm_bindgen(js_name = tickPause)]
    pub fn tick_pause(&self) {
        self.api.tick_pause();
    }

    #[wasm_bindgen(js_name = tickResume)]
    pub fn tick_resume(&self) {
        self.api.tick_resume();
    }

    #[wasm_bindgen(js_name = tickPaused)]
    pub fn tick_paused(&self) -> bool {
        self.api.tick_paused()
    }

    #[wasm_bindgen(js_name = lastError)]
    pub fn last_error(&self) -> String {
        self.api.last_error()
    }
}

/// The encrypted credential store for a host whose rows answer asynchronously: the host lists its rows and `load`s them,
/// runs an operation, then applies `takeWrites()` to its medium in order. See `credentials::VaultApi`.
#[wasm_bindgen]
pub struct CredentialVault {
    api: credentials::VaultApi,
}

#[wasm_bindgen]
impl CredentialVault {
    #[wasm_bindgen(constructor)]
    pub fn new(
        master_key: Option<String>,
        previous_key: Option<String>,
    ) -> Result<CredentialVault, JsError> {
        credentials::VaultApi::new(master_key.as_deref(), previous_key.as_deref())
            .map(|api| CredentialVault { api })
            .map_err(js)
    }

    #[wasm_bindgen(getter)]
    pub fn enabled(&self) -> bool {
        self.api.enabled()
    }

    pub fn load(&self, rows_json: &str) -> Result<u32, JsError> {
        self.api
            .load(rows_json)
            .map(|count| count as u32)
            .map_err(js)
    }

    pub fn resolve(
        &self,
        name: &str,
        principal: Option<String>,
    ) -> Result<Option<String>, JsError> {
        self.api.resolve(name, principal.as_deref()).map_err(js)
    }

    pub fn label(&self, name: &str, principal: Option<String>) -> Result<Option<String>, JsError> {
        self.api.label(name, principal.as_deref()).map_err(js)
    }

    #[wasm_bindgen(js_name = snapshotEnv)]
    pub fn snapshot_env(&self) -> Result<String, JsError> {
        self.api.snapshot_env().map_err(js)
    }

    pub fn entries(&self, principal: Option<String>) -> Result<String, JsError> {
        self.api.entries(principal.as_deref()).map_err(js)
    }

    pub fn listing(&self) -> Result<String, JsError> {
        self.api.listing().map_err(js)
    }

    pub fn set(
        &self,
        service: &str,
        name: &str,
        value: &str,
        principal: Option<String>,
        label: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .set(service, name, value, principal.as_deref(), label.as_deref())
            .map_err(js)
    }

    pub fn replace(
        &self,
        name: &str,
        value: &str,
        principal: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .replace(name, value, principal.as_deref())
            .map_err(js)
    }

    pub fn persist(
        &self,
        name: &str,
        value: &str,
        principal: Option<String>,
        label: Option<String>,
    ) -> Result<String, JsError> {
        self.api
            .persist(name, value, principal.as_deref(), label.as_deref())
            .map_err(js)
    }

    pub fn remove(&self, name: &str, principal: Option<String>) -> Result<bool, JsError> {
        self.api.remove(name, principal.as_deref()).map_err(js)
    }

    pub fn rotate(&self) -> Result<String, JsError> {
        self.api.rotate().map_err(js)
    }

    #[wasm_bindgen(js_name = connectBegin)]
    pub fn connect_begin(&self, grant_json: &str) -> Result<String, JsError> {
        self.api.connect_begin(grant_json).map_err(js)
    }

    #[wasm_bindgen(js_name = refreshBegin)]
    pub fn refresh_begin(&self, name: &str, principal: Option<String>) -> Result<String, JsError> {
        self.api
            .refresh_begin(name, principal.as_deref())
            .map_err(js)
    }

    #[wasm_bindgen(js_name = connectFinish)]
    pub fn connect_finish(&self, ticket: f64, status: u16, body: &str) -> Result<String, JsError> {
        self.api
            .connect_finish(ticket as u64, status, body)
            .map_err(js)
    }

    #[wasm_bindgen(js_name = connectAbandon)]
    pub fn connect_abandon(&self, ticket: f64) -> Result<bool, JsError> {
        self.api.connect_abandon(ticket as u64).map_err(js)
    }

    #[wasm_bindgen(js_name = takeWrites)]
    pub fn take_writes(&self) -> Result<String, JsError> {
        self.api.take_writes().map_err(js)
    }
}

/// Install the language data a host assembles from its installed `@almadar/core` and `@almadar/std`
/// (`readLanguageBundle`): the operator table that routes `program/*` and the other host effects.
#[wasm_bindgen(js_name = installLanguage)]
pub fn install_language(bundle_json: &str) -> Result<(), JsError> {
    api::install_language(bundle_json).map_err(js)
}

#[wasm_bindgen(js_name = osGlobMatches)]
pub fn os_glob_matches(glob: &str, path: &str) -> bool {
    api::os_glob_matches(glob, path)
}

#[wasm_bindgen(js_name = expandFxLayer)]
pub fn expand_fx_layer(
    layer_json: &str,
    epoch_now_ms: f64,
    dimension: &str,
    tile_width: f64,
) -> Result<String, JsError> {
    api::expand_fx_layer(layer_json, epoch_now_ms, dimension, tile_width).map_err(js)
}

#[wasm_bindgen(js_name = resolveFxView)]
pub fn resolve_fx_view(item_json: &str, presets_json: Option<String>) -> Result<String, JsError> {
    api::resolve_fx_view(item_json, presets_json.as_deref()).map_err(js)
}

#[wasm_bindgen(js_name = fxLifecycle)]
pub fn fx_lifecycle(view_json: &str, epoch_now_ms: f64, tick_ms: f64) -> Result<String, JsError> {
    api::fx_lifecycle(view_json, epoch_now_ms, tick_ms).map_err(js)
}

#[wasm_bindgen(js_name = fxHash01)]
pub fn fx_hash01(seed: &str, salt: u32) -> f64 {
    api::fx_hash01(seed, salt)
}

#[wasm_bindgen(js_name = skinWorldMatrices)]
pub fn skin_world_matrices(bones_json: &str, pose_json: &str) -> Result<String, JsError> {
    api::skin_world_matrices(bones_json, pose_json).map_err(js)
}

#[wasm_bindgen(js_name = skinVertices)]
pub fn skin_vertices(
    mesh_json: &str,
    bind_json: &str,
    current_json: &str,
) -> Result<String, JsError> {
    api::skin_vertices(mesh_json, bind_json, current_json).map_err(js)
}

#[wasm_bindgen(js_name = skinComposeAffine)]
pub fn skin_compose_affine(a_json: &str, b_json: &str) -> Result<String, JsError> {
    api::skin_compose_affine(a_json, b_json).map_err(js)
}

#[wasm_bindgen(js_name = skinInvertAffine)]
pub fn skin_invert_affine(matrix_json: &str, bone: u32) -> Result<String, JsError> {
    api::skin_invert_affine(matrix_json, bone).map_err(js)
}

#[wasm_bindgen(js_name = skinApplyAffine)]
pub fn skin_apply_affine(matrix_json: &str, x: f64, y: f64) -> Result<String, JsError> {
    api::skin_apply_affine(matrix_json, x, y).map_err(js)
}

#[wasm_bindgen(js_name = skinTriangleAffine)]
pub fn skin_triangle_affine(src_json: &str, dst_json: &str) -> Result<String, JsError> {
    api::skin_triangle_affine(src_json, dst_json).map_err(js)
}

#[wasm_bindgen(js_name = skinVertexBounds)]
pub fn skin_vertex_bounds(points_json: &str) -> Result<String, JsError> {
    api::skin_vertex_bounds(points_json).map_err(js)
}

/// The typed event request a `program/dispatch` call names, as an object; a record that is no event request throws the reason.
#[wasm_bindgen(js_name = parseEventRequest)]
pub fn parse_event_request(payload_json: &str, options_json: &str) -> Result<JsValue, JsError> {
    let request = api::parse_event_request(payload_json, options_json).map_err(js)?;
    js_sys::JSON::parse(&request)
        .map_err(|_| JsError::new("parseEventRequest: the kernel wrote no JSON"))
}

/// The kernel's verdict on language packs (`[{ package, definition }]` as JSON): throws the refusal that names each pack, section and word.
#[wasm_bindgen(js_name = validatePacks)]
pub fn validate_packs(packs_json: &str) -> Result<(), JsError> {
    api::validate_packs(packs_json).map_err(js)
}

/// The program's emitter-to-listener wire table (`EventWire[]` as JSON), by the source rule the kernel delivers by.
#[wasm_bindgen(js_name = eventWires)]
pub fn event_wires(schema_json: &str) -> Result<String, JsError> {
    api::event_wires(schema_json).map_err(js)
}

/// Whether the Chrome match pattern `pattern` matches `url`.
#[wasm_bindgen(js_name = tabUrlMatches)]
pub fn tab_url_matches(pattern: &str, url: &str) -> bool {
    api::tab_url_matches(pattern, url)
}

/// One `ContractQuery` (as JSON) answered for a host with no registered program, as JSON.
#[wasm_bindgen(js_name = contractQueryWithoutProgram)]
pub fn contract_query_without_program(query_json: &str) -> Result<String, JsError> {
    api::contract_query_without_program(query_json).map_err(js)
}

/// The names of the `os/` operations whose keyed call may defer its completion, as a JSON array.
#[wasm_bindgen(js_name = osDeferringOps)]
pub fn os_deferring_ops() -> Result<String, JsError> {
    api::os_deferring_ops().map_err(js)
}

/// The names of the `os/` operations whose keyed call is a stream, as a JSON array.
#[wasm_bindgen(js_name = osStreamOps)]
pub fn os_stream_ops() -> Result<String, JsError> {
    api::os_stream_ops().map_err(js)
}

/// The `{ event, payload }` a deferred keyed `os/` call's outcome (`Settled`) fires on its call site, or `null`.
#[wasm_bindgen(js_name = osCompletionEvent)]
pub fn os_completion_event(
    site_json: &str,
    key: &str,
    outcome_json: &str,
) -> Result<String, JsError> {
    api::os_completion_event(site_json, key, outcome_json).map_err(js)
}

#[wasm_bindgen(js_name = isPublicAddress)]
pub fn is_public_address(ip: &str) -> Result<bool, JsError> {
    ip.parse()
        .map(orbital_core::runtime::http_wire::is_public_address)
        .map_err(|e| js(format!("parse ip: {e}")))
}

/// Evaluate `expressionsJson` (`SExpr[]`) against `contextJson` (`EvalContextWire`) in one crossing:
/// `EvalOutcomeWire[]` as JSON, in order.
#[wasm_bindgen(js_name = evaluateMany)]
pub fn evaluate_many(context_json: &str, expressions_json: &str) -> Result<String, JsError> {
    api::evaluate_many(context_json, expressions_json).map_err(js)
}

/// Resolve every prop value of a render against `contextJson` in one crossing: each binding, embedded ones
/// included, and each operator call (`EvalOutcomeWire[]` as JSON, in order).
#[wasm_bindgen(js_name = resolveValues)]
pub fn resolve_values(context_json: &str, values_json: &str) -> Result<String, JsError> {
    api::resolve_values(context_json, values_json).map_err(js)
}

/// Every guard of a render against a context in one crossing (`GuardOutcomeWire[]` as JSON).
#[wasm_bindgen(js_name = evaluateGuards)]
pub fn evaluate_guards(context_json: &str, guards_json: &str) -> Result<String, JsError> {
    api::evaluate_guards(context_json, guards_json).map_err(js)
}

/// One expression with its trace (`EvalTracedWire` as JSON).
#[wasm_bindgen(js_name = evaluateTraced)]
pub fn evaluate_traced(context_json: &str, expression_json: &str) -> Result<String, JsError> {
    api::evaluate_traced(context_json, expression_json).map_err(js)
}

/// Effects run in order against a row (`TraceEffectsWire` as JSON): a trace per effect, the events raised, the writes made.
#[wasm_bindgen(js_name = traceEffects)]
pub fn trace_effects(context_json: &str, effects_json: &str) -> Result<String, JsError> {
    api::trace_effects(context_json, effects_json).map_err(js)
}

/// A listen route's `with` mapping applied to the source payload (the mapped payload as JSON).
#[wasm_bindgen(js_name = mapListenPayload)]
pub fn map_listen_payload(mapping_json: &str, payload_json: &str) -> Result<String, JsError> {
    api::map_listen_payload(mapping_json, payload_json).map_err(js)
}

/// The rows that pass a `@read` policy and a call-site filter (each an `SExpr` as JSON, or empty).
#[wasm_bindgen(js_name = scopeRows)]
pub fn scope_rows(
    bindings_json: &str,
    rows_json: &str,
    policy_json: &str,
    filter_json: &str,
    entity_type: &str,
) -> Result<String, JsError> {
    api::scope_rows(
        bindings_json,
        rows_json,
        policy_json,
        filter_json,
        entity_type,
    )
    .map_err(js)
}

/// Whether a row may be created, updated or deleted under a directive (`MutationVerdictWire` as JSON).
#[wasm_bindgen(js_name = checkMutationAccess)]
pub fn check_mutation_access(
    operation: &str,
    bindings_json: &str,
    row_json: &str,
    policy_json: &str,
    entity_type: &str,
) -> Result<String, JsError> {
    api::check_mutation_access(operation, bindings_json, row_json, policy_json, entity_type)
        .map_err(js)
}

/// What a `@user`/`@payload`/`@config`/`@now` term folds to for a row predicate (`RowBindingWire` as JSON).
#[wasm_bindgen(js_name = resolveRowBinding)]
pub fn resolve_row_binding(term: &str, bindings_json: &str) -> Result<String, JsError> {
    api::resolve_row_binding(term, bindings_json).map_err(js)
}

#[wasm_bindgen(js_name = substrateCompletionEvent)]
pub fn substrate_completion_event(
    site_json: &str,
    key: &str,
    op: &str,
    args_json: &str,
    outcome_json: &str,
) -> Result<String, JsError> {
    api::substrate_completion_event(site_json, key, op, args_json, outcome_json).map_err(js)
}

pub use orbital_core::embed::memory::MemoryStoreApi;
pub use orbital_core::runtime::{LazyProgram, LazySource};
pub use orbital_core::OrbitalSchema;

#[derive(Default)]
#[wasm_bindgen]
pub struct MemoryStore {
    api: MemoryStoreApi,
}

#[wasm_bindgen]
impl MemoryStore {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            api: MemoryStoreApi::new(),
        }
    }
    #[wasm_bindgen(js_name = getById)]
    pub fn get_by_id(&self, entity_type: &str, id: &str) -> Result<String, JsError> {
        self.api.get_by_id(entity_type, id).map_err(js)
    }
    #[wasm_bindgen(js_name = list)]
    pub fn list(&self, entity_type: &str) -> Result<String, JsError> {
        self.api.list(entity_type).map_err(js)
    }
    #[wasm_bindgen(js_name = create)]
    pub fn create(&self, entity_type: &str, data_json: &str) -> Result<String, JsError> {
        self.api.create(entity_type, data_json).map_err(js)
    }
    #[wasm_bindgen(js_name = update)]
    pub fn update(&self, entity_type: &str, id: &str, data_json: &str) -> Result<String, JsError> {
        self.api.update(entity_type, id, data_json).map_err(js)
    }
    #[wasm_bindgen(js_name = delete)]
    pub fn delete(&self, entity_type: &str, id: &str) -> Result<(), JsError> {
        self.api.delete(entity_type, id).map_err(js)
    }
    #[wasm_bindgen(js_name = queryPage)]
    pub fn query_page(&self, entity_type: &str, query_json: &str) -> Result<String, JsError> {
        self.api.query_page(entity_type, query_json).map_err(js)
    }
    #[wasm_bindgen(js_name = queryIds)]
    pub fn query_ids(&self, entity_type: &str, query_json: &str) -> Result<String, JsError> {
        self.api.query_ids(entity_type, query_json).map_err(js)
    }
    #[wasm_bindgen(js_name = writeWhere)]
    pub fn write_where(&self, op_json: &str) -> Result<String, JsError> {
        self.api.write_where(op_json).map_err(js)
    }
    #[wasm_bindgen(js_name = writeBatch)]
    pub fn write_batch(&self, batch_json: &str) -> Result<String, JsError> {
        self.api.write_batch(batch_json).map_err(js)
    }
    #[wasm_bindgen(js_name = queryCapabilities)]
    pub fn query_capabilities(&self) -> Result<String, JsError> {
        self.api.query_capabilities().map_err(js)
    }
    #[wasm_bindgen(js_name = writeCapabilities)]
    pub fn write_capabilities(&self) -> Result<String, JsError> {
        self.api.write_capabilities().map_err(js)
    }
    #[wasm_bindgen(js_name = snapshot)]
    pub fn snapshot(&self) -> Result<String, JsError> {
        self.api.snapshot().map_err(js)
    }
    #[wasm_bindgen(js_name = bindCollection)]
    pub fn bind_collection(&self, entity_type: &str, collection: &str) {
        self.api.bind_collection(entity_type, collection);
    }
    #[wasm_bindgen]
    pub fn clear(&self) {
        self.api.clear();
    }
}
