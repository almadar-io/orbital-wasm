//! `orbital_core::embed` for JavaScript hosts. Marshalling only: every decision is the embedded kernel's.
//!
//! `api::KernelApi` is the host-facing surface over JSON strings; `Kernel` is its wasm-bindgen
//! class. A failed operation throws a JS `Error` carrying the message the C ABI puts in
//! `last_error`.

pub mod api;
pub mod http;
pub mod oauth;
pub mod stored;

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

    #[wasm_bindgen(js_name = statefulLeg)]
    pub fn stateful_leg(&self, leg_json: &str) -> Result<String, JsError> {
        self.api.stateful_leg(leg_json).map_err(js)
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

    /// A server holding this one's registered program and none of its session: what a stateless host serves
    /// each request from, instead of registering the program again.
    pub fn fork(&self) -> Result<Server, JsError> {
        self.api.fork().map(|api| Server { api }).map_err(js)
    }

    #[wasm_bindgen(js_name = osAllows)]
    pub fn os_allows(&self, requirement_json: &str) -> Result<bool, JsError> {
        self.api.os_allows(requirement_json).map_err(js)
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

    /// Mock-seed the `[runtime]` entities the kernel holds itself (a preview host, per fresh kernel).
    #[wasm_bindgen(js_name = seedRuntimeMock)]
    pub fn seed_runtime_mock(&self, viewer_id: Option<String>) {
        self.api.seed_runtime_mock(viewer_id);
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

/// The verifier's planner: what to fire for a resolved program (`PlanOutcome` as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyPlan)]
pub fn verify_plan(resolved_schema_json: &str, options_json: &str) -> String {
    api::verify_plan(resolved_schema_json, options_json)
}

/// The oracle of one planned step (`StepWire` in, `Expectation` out, as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyExpect)]
pub fn verify_expect(step_json: &str) -> Result<String, JsError> {
    api::verify_expect(step_json).map_err(js)
}

/// The verdict on one observed step (`Expectation` and `Observation` in, `VerdictWire` out, as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyJudge)]
pub fn verify_judge(expectation_json: &str, observation_json: &str) -> Result<String, JsError> {
    api::verify_judge(expectation_json, observation_json).map_err(js)
}

/// The failure-route steps of a walk credited from the hops other steps' dispatches fired
/// (`CreditStep[]` in, `Credit[]` out, as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyCredit)]
pub fn verify_credit(steps_json: &str) -> Result<String, JsError> {
    api::verify_credit(steps_json).map_err(js)
}

/// A rendered affordance the host found, planned (`AffordanceQuery` in, `AffordanceStep` or `null` out, as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyAffordance)]
pub fn verify_affordance(resolved_schema_json: &str, query_json: &str, options_json: &str) -> Result<String, JsError> {
    api::verify_affordance(resolved_schema_json, query_json, options_json).map_err(js)
}

/// The forced failure of a transient failure arm (`TransientQuery` in, `TransientOutcome` or `null` out, as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyTransientProbe)]
pub fn verify_transient_probe(resolved_schema_json: &str, query_json: &str, options_json: &str) -> Result<String, JsError> {
    api::verify_transient_probe(resolved_schema_json, query_json, options_json).map_err(js)
}

/// The hops that place a fresh machine in a state (`ReplayQuery` in, `PreambleEvent[]` or `null` out, as JSON).
#[cfg(feature = "verify")]
#[wasm_bindgen(js_name = verifyReplay)]
pub fn verify_replay(resolved_schema_json: &str, query_json: &str, options_json: &str) -> Result<String, JsError> {
    api::verify_replay(resolved_schema_json, query_json, options_json).map_err(js)
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

#[wasm_bindgen(js_name = osGlobMatches)]
pub fn os_glob_matches(glob: &str, path: &str) -> bool {
    api::os_glob_matches(glob, path)
}

/// Whether the Chrome match pattern `pattern` matches `url`.
#[wasm_bindgen(js_name = tabUrlMatches)]
pub fn tab_url_matches(pattern: &str, url: &str) -> bool {
    api::tab_url_matches(pattern, url)
}

/// The names of the `os/` operations whose keyed call may defer its completion, as a JSON array.
#[wasm_bindgen(js_name = osDeferringOps)]
pub fn os_deferring_ops() -> Result<String, JsError> {
    api::os_deferring_ops().map_err(js)
}

/// The `{ event, payload }` a deferred keyed `os/` call's outcome (`Settled`) fires on its call site, or `null`.
#[wasm_bindgen(js_name = osCompletionEvent)]
pub fn os_completion_event(site_json: &str, key: &str, outcome_json: &str) -> Result<String, JsError> {
    api::os_completion_event(site_json, key, outcome_json).map_err(js)
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

/// The pure half of `oauth-authorize`: parse the client and options (`oauthAuthorizePlan`), then build the
/// authorization URL from the metadata and two host-drawn random values (`oauthAuthorizeBuild`).
#[wasm_bindgen(js_name = oauthAuthorizePlan)]
pub fn oauth_authorize_plan(client_json: &str, options_json: &str) -> Result<String, JsError> {
    oauth::oauth_authorize_plan(client_json, options_json).map_err(js)
}

#[wasm_bindgen(js_name = oauthAuthorizeBuild)]
pub fn oauth_authorize_build(
    plan_json: &str,
    client_id: &str,
    metadata_json: &str,
    state_base64: &str,
    verifier_base64: &str,
) -> Result<String, JsError> {
    oauth::oauth_authorize_build(plan_json, client_id, metadata_json, state_base64, verifier_base64).map_err(js)
}

#[wasm_bindgen(js_name = oauthAuthorizeMockBuild)]
pub fn oauth_authorize_mock_build(plan_json: &str, state_base64: &str) -> Result<String, JsError> {
    oauth::oauth_authorize_mock_build(plan_json, state_base64).map_err(js)
}

#[wasm_bindgen(js_name = oauthFailure)]
pub fn oauth_failure(reason: &str) -> Result<String, JsError> {
    oauth::oauth_failure(reason).map_err(js)
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

/// The credentials a `store`-source boot seeds from the env (`[{ service, envVar }]` as JSON): what the store lacks and the env holds.
#[cfg(feature = "full")]
#[wasm_bindgen(js_name = credentialSeedPlan)]
pub fn credential_seed_plan(declared_json: &str, held_json: &str, present_json: &str) -> Result<String, JsError> {
    api::credential_seed_plan(declared_json, held_json, present_json).map_err(js)
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
    api::scope_rows(bindings_json, rows_json, policy_json, filter_json, entity_type).map_err(js)
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
    api::check_mutation_access(operation, bindings_json, row_json, policy_json, entity_type).map_err(js)
}

/// What a `@user`/`@payload`/`@config`/`@now` term folds to for a row predicate (`RowBindingWire` as JSON).
#[wasm_bindgen(js_name = resolveRowBinding)]
pub fn resolve_row_binding(term: &str, bindings_json: &str) -> Result<String, JsError> {
    api::resolve_row_binding(term, bindings_json).map_err(js)
}
