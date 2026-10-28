//! The host-facing operations, one per C-ABI operation, as plain functions over JSON strings.
//! `Kernel` (wasm-bindgen) forwards to these unchanged, so a native test drives the same code the
//! JS host calls. Marshalling only: every decision is `orbital_core::embed`'s.

use orbital_core::effects::MountSeed;
use orbital_core::embed::{parse_theme_mode, EmbeddedKernel, EmbeddedServer, FoldBase};
use orbital_core::runtime::kernel::EntryError;
use orbital_core::runtime::TraitSnapshot;
use orbital_core::{EventResponse, OrbitalSchema, Value};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Mutex;

/// Why a kernel or server entry refused to start: the program did not parse or serialize, or the kernel refused it.
#[derive(Debug)]
pub enum ApiError {
    Marshal(String),
    Entry(EntryError),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Marshal(message) => f.write_str(message),
            Self::Entry(error) => error.fmt(f),
        }
    }
}

impl From<EntryError> for ApiError {
    fn from(error: EntryError) -> Self {
        Self::Entry(error)
    }
}

pub struct KernelApi {
    embedded: EmbeddedKernel,
    last_error: Mutex<String>,
}

fn to_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("serialize: {e}"))
}

fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("parse {what}: {e}"))
}

fn side_of(side: &str) -> Result<orbital_core::protocols::ProtocolSide, String> {
    serde_json::from_value(serde_json::Value::String(side.to_string())).map_err(|e| format!("parse side: {e}"))
}

impl KernelApi {
    pub fn new(resolved_schema_json: &str, client_id: &str) -> Result<Self, ApiError> {
        let schema: OrbitalSchema = parse(resolved_schema_json, "resolved schema").map_err(ApiError::Marshal)?;
        Ok(Self {
            embedded: EmbeddedKernel::new(schema, client_id)?,
            last_error: Mutex::new(String::new()),
        })
    }

    fn record<T>(&self, result: Result<T, String>) -> Result<T, String> {
        if let Err(e) = &result {
            self.set_last_error(e.clone());
        }
        result
    }

    fn set_last_error(&self, message: String) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = message;
        }
    }

    pub fn dispatch(
        &self,
        trait_name: &str,
        entity_id: Option<&str>,
        event: &str,
        payload_json: Option<&str>,
    ) -> Result<String, String> {
        self.record((|| {
            self.embedded.owning_orbital(trait_name)?;
            let payload: Value = match payload_json {
                Some(s) if !s.is_empty() => parse(s, "payload_json")?,
                _ => Value::Null,
            };
            to_json(
                &self
                    .embedded
                    .dispatch(trait_name, entity_id, event, payload)?,
            )
        })())
    }

    pub fn apply_server_response(
        &self,
        response_json: &str,
        already_delivered_json: Option<&str>,
        base_json: Option<&str>,
    ) -> Result<String, String> {
        self.record((|| {
            let response: EventResponse = parse(response_json, "response_json")?;
            let already: HashSet<(String, String)> = match already_delivered_json {
                Some(s) if !s.is_empty() => {
                    let pairs: Vec<(String, String)> = parse(s, "already_delivered_json")?;
                    pairs.into_iter().collect()
                }
                _ => HashSet::new(),
            };
            let base: Option<FoldBase> = match base_json {
                Some(s) if !s.is_empty() => Some(parse(s, "base_json")?),
                _ => None,
            };
            to_json(&self.embedded.apply_server_response_since(
                &response,
                &already,
                base.as_ref(),
            )?)
        })())
    }

    /// The leg as a host that holds the circuit state takes it: only the `[shared]` frame rows its traits read.
    pub fn stateful_leg(&self, leg_json: &str) -> Result<String, String> {
        self.record((|| {
            let leg: orbital_core::EventRequest = parse(leg_json, "leg_json")?;
            to_json(&self.embedded.stateful_leg(leg))
        })())
    }

    /// The mount a reconnecting client posts to rebuild its session from the frames and states it holds (`MountSeed[]` in).
    pub fn reseed_leg(&self, seeds_json: &str, entity_id: Option<&str>) -> Result<String, String> {
        self.record((|| {
            let seeds: Vec<MountSeed> = parse(seeds_json, "seeds_json")?;
            to_json(&self.embedded.reseed_leg(&seeds, entity_id))
        })())
    }

    /// The base a posted leg's answer is folded against; `None` when the leg carried no rows.
    pub fn fold_base(
        &self,
        carried_json: Option<&str>,
        seeded_json: &str,
        entity_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        self.record((|| {
            let carried: Option<FoldBase> = match carried_json {
                Some(s) if !s.is_empty() => Some(parse(s, "carried_json")?),
                _ => None,
            };
            let seeded: Vec<String> = parse(seeded_json, "seeded_json")?;
            self.embedded
                .fold_base(carried.as_ref(), &seeded, entity_id)
                .map(|base| to_json(&base))
                .transpose()
        })())
    }

    /// Dispatch a mount batch (`MountSeed[]` as JSON); a `MountWire` as JSON.
    pub fn dispatch_mount(
        &self,
        seeds_json: &str,
        entity_id: Option<&str>,
        payload_json: Option<&str>,
    ) -> Result<String, String> {
        self.record((|| {
            let seeds: Vec<MountSeed> = parse(seeds_json, "seeds_json")?;
            let payload: Value = match payload_json {
                Some(s) if !s.is_empty() => parse(s, "payload_json")?,
                _ => Value::Null,
            };
            to_json(&self.embedded.dispatch_mount(&seeds, entity_id, payload)?)
        })())
    }

    pub fn restore(
        &self,
        trait_name: &str,
        entity_id: Option<&str>,
        snapshot_json: &str,
    ) -> Result<String, String> {
        self.record((|| {
            let snapshot: TraitSnapshot = parse(snapshot_json, "snapshot_json")?;
            to_json(&self.embedded.restore(trait_name, entity_id, snapshot))
        })())
    }

    pub fn tick(
        &self,
        trait_name: &str,
        tick_name: &str,
        entity_id: Option<&str>,
    ) -> Result<String, String> {
        self.record((|| {
            to_json(&self.embedded.tick(trait_name, tick_name, entity_id)?)
        })())
    }

    /// `None` when no tick is due this frame.
    pub fn frame(&self, now_ms: u64) -> Result<Option<String>, String> {
        self.record((|| match self.embedded.frame(now_ms)? {
            None => Ok(None),
            Some(wire) => {
                if let Some(first) = wire.errors.first() {
                    self.set_last_error(first.clone());
                }
                to_json(&wire).map(Some)
            }
        })())
    }

    pub fn frame_pause(&self) {
        self.embedded.frame_pause();
    }

    pub fn frame_resume(&self) {
        self.embedded.frame_resume();
    }

    pub fn tick_count(&self) -> usize {
        self.embedded.tick_count()
    }

    pub fn trait_config(&self, trait_name: &str) -> Result<String, String> {
        self.record((|| to_json(&self.embedded.trait_config(trait_name)?))())
    }

    pub fn dispatch_mode(&self, trait_name: &str) -> Result<String, String> {
        self.record((|| to_json(&self.embedded.dispatch_mode(trait_name)?))())
    }

    pub fn get_state(&self, trait_name: &str, entity_id: Option<&str>) -> String {
        self.embedded.get_state(trait_name, entity_id)
    }

    pub fn snapshot(&self) -> Result<String, String> {
        self.record(to_json(&self.embedded.snapshot()))
    }

    pub fn set_theme_mode(&self, mode: &str) -> Result<(), String> {
        self.record(parse_theme_mode(mode).map(|m| self.embedded.set_theme_mode(m)))
    }

    pub fn set_current_user(&self, user_json: &str) -> Result<(), String> {
        self.record(
            parse::<Value>(user_json, "user_json").map(|u| self.embedded.set_current_user(u)),
        )
    }

    /// The host runs the listeners of every emit it marks `dispatched` (a worklist host).
    pub fn set_host_fans_out(&self, fans_out: bool) {
        self.embedded.set_host_fans_out(fans_out);
    }

    /// Carry each hop's `@entity` before and after it on the dispatch's transitions, for a verifier (EXEC-STEP-10).
    pub fn set_record_entity_after(&self, record: bool) {
        self.embedded.set_record_entity_after(record);
    }

    /// Carry each tick's `effectResults` on the frame response, for a verifier (P-015).
    pub fn set_frame_effect_results(&self, carry: bool) {
        self.embedded.set_frame_effect_results(carry);
    }

    pub fn resolve_rows(&self, ref_json: &str) -> Result<String, String> {
        self.record(
            parse::<orbital_core::runtime::row_query::RowSetRef>(ref_json, "ref_json")
                .and_then(|reference| self.embedded.resolve_rows(&reference))
                .and_then(|page| to_json(&page)),
        )
    }

    pub fn seed(&self, entity_type: &str, rows_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<Value>>(rows_json, "rows_json")
                .and_then(|rows| self.embedded.seed(entity_type, rows)),
        )
    }

    pub fn remount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.remount(t)),
        )
    }

    /// The viewer's locales (`string[]`, the first is painted) and their catalogs (`{ [locale]: { [key]: text } }`), as JSON.
    pub fn set_messages(&self, locales_json: &str, catalogs_json: &str) -> Result<(), String> {
        self.record(parse::<Vec<String>>(locales_json, "locales_json").and_then(|locales| {
            parse::<std::collections::HashMap<String, std::collections::HashMap<String, String>>>(catalogs_json, "catalogs_json")
                .map(|catalogs| self.embedded.set_messages(locales, catalogs))
        }))
    }

    pub fn mount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.mount(t)),
        )
    }

    /// The kernel's mounts for a host start on `side` (`client|server|process|start`), `MountSeed[]` as JSON.
    pub fn boot_mounts(&self, side: &str) -> Result<String, String> {
        self.record(side_of(side).and_then(|side| to_json(&self.embedded.boot_mounts(side))))
    }

    /// The kernel's mounts for a client opening `route_path`, `MountSeed[]` as JSON.
    pub fn route_mounts(&self, route_path: &str) -> Result<String, String> {
        self.record(to_json(&self.embedded.route_mounts(route_path)))
    }

    pub fn unmount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.unmount(t)),
        )
    }

    pub fn set_trait_on_screen(&self, trait_name: &str, on_screen: bool) {
        self.embedded.set_trait_on_screen(trait_name, on_screen);
    }

    pub fn last_error(&self) -> String {
        self.last_error
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

/// The server role: dispatches that stop at their I/O and go on when the host has answered.
pub struct ServerApi {
    embedded: EmbeddedServer,
    last_error: Mutex<String>,
}

impl ServerApi {
    pub fn new(resolved_schema_json: &str) -> Result<Self, ApiError> {
        let schema: OrbitalSchema = parse(resolved_schema_json, "resolved schema").map_err(ApiError::Marshal)?;
        Ok(Self {
            embedded: EmbeddedServer::new(schema)?,
            last_error: Mutex::new(String::new()),
        })
    }

    fn record<T, E: std::fmt::Display>(&self, result: Result<T, E>) -> Result<T, E> {
        if let Err(e) = &result {
            if let Ok(mut slot) = self.last_error.lock() {
                *slot = e.to_string();
            }
        }
        result
    }

    /// A server holding this one's registered program and none of its session (`EmbeddedServer::fork_session`).
    pub fn fork(&self) -> Result<Self, EntryError> {
        Ok(Self {
            embedded: self.record(self.embedded.fork_session())?,
            last_error: Mutex::new(String::new()),
        })
    }

    pub fn fork_program(&self, program_json: &str) -> Result<Self, ApiError> {
        let schema: OrbitalSchema = self.record(parse(program_json, "program_json")).map_err(ApiError::Marshal)?;
        Ok(Self {
            embedded: self.record(self.embedded.fork_program(schema))?,
            last_error: Mutex::new(String::new()),
        })
    }

    /// The server underneath, for a native host that answers its asks from the program's own egress and grants.
    pub fn embedded(&self) -> &EmbeddedServer {
        &self.embedded
    }

    /// Whether the program's `os/` grants cover `requirement_json` (an `OsRequirement`).
    pub fn os_allows(&self, requirement_json: &str) -> Result<bool, String> {
        self.record(self.embedded.os_allows_json(requirement_json))
    }

    /// Place the program's anchored grants and arguments under the host's anchors; answers what grants nothing.
    pub fn set_os_anchors(&self, anchors_json: &str) -> Result<String, String> {
        self.record(self.embedded.set_os_anchors_json(anchors_json))
    }

    /// An `os/` call's arguments with its anchored parts resolved under the host's anchors (`Settled<args, OsError>`).
    pub fn os_resolve_args(&self, op: &str, args_json: &str) -> Result<String, String> {
        self.record(self.embedded.os_resolve_args_json(op, args_json))
    }

    pub fn live_item_delivered(&self, item_json: &str, viewer_json: &str, client_id: Option<&str>) -> Result<bool, String> {
        self.record(self.embedded.live_item_delivered(item_json, viewer_json, client_id))
    }

    pub fn dispatch_stream(&self) -> Result<String, String> {
        self.record(self.embedded.dispatch_stream_json())
    }

    /// Start a request against `orbital`; a `ServerProgressWire` as JSON.
    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, String> {
        self.record(self.embedded.start_json(orbital, request_json))
    }

    /// Answer the requests a flight is waiting on (`IoReplyWire[]` as JSON); a `ServerProgressWire` as JSON.
    pub fn resume(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.record(self.embedded.resume_json(flight, replies_json))
    }

    /// The oldest queued request, started now the lane is free; a `NextWire` as JSON, or `null`.
    pub fn next(&self) -> Result<String, String> {
        self.record(self.embedded.next_json())
    }

    /// Whether `flight` is a dispatch parked holding the lane.
    pub fn holds_lane(&self, flight: u64) -> bool {
        self.embedded.holds_lane(flight)
    }

    /// A live stream of `client`'s session opened; the id closes it.
    pub fn open_session_stream(&self, client: &str) -> u64 {
        self.embedded.open_session_stream(client)
    }

    /// The live stream `id` dropped.
    pub fn close_session_stream(&self, id: u64) -> bool {
        self.embedded.close_session_stream(id)
    }

    /// Resolve a row-set reference (`RowSetRef` as JSON) for a viewer (a user as JSON) through the host-answered
    /// store; a `ResolveProgressWire` as JSON.
    pub fn resolve_rows(&self, ref_json: &str, viewer_json: &str) -> Result<String, String> {
        self.record(self.embedded.resolve_rows_json(ref_json, viewer_json))
    }

    /// Answer the requests a resolve flight is waiting on (`IoReplyWire[]` as JSON); a `ResolveProgressWire` as JSON.
    pub fn resume_resolve(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.record(self.embedded.resume_resolve_json(flight, replies_json))
    }

    pub fn abandon(&self, flight: u64) -> bool {
        self.embedded.abandon(flight)
    }

    pub fn in_flight(&self) -> usize {
        self.embedded.in_flight()
    }

    pub fn get_state(&self, trait_name: &str, entity_id: Option<&str>) -> String {
        self.embedded.get_state(trait_name, entity_id)
    }

    pub fn set_default_user(&self, user_json: &str) -> Result<(), String> {
        self.record(
            parse::<Value>(user_json, "user_json").map(|u| self.embedded.set_default_user(u)),
        )
    }

    /// The program (as JSON) whose declared credentials this server's credential effects manage beside its own.
    pub fn set_managed_program(&self, program_json: &str) -> Result<(), String> {
        self.record(
            parse::<serde_json::Value>(program_json, "program_json").map(|p| self.embedded.set_managed_program(&p)),
        )
    }

    pub fn mount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.mount(t)),
        )
    }

    pub fn remount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.remount(t)),
        )
    }

    /// The program's declared locales (`string[]`) and merged catalogs (`{ [locale]: { [key]: text } }`), as JSON.
    pub fn set_messages(&self, locales_json: &str, catalogs_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(locales_json, "locales_json").and_then(|locales| {
                parse::<std::collections::HashMap<String, std::collections::HashMap<String, String>>>(
                    catalogs_json,
                    "catalogs_json",
                )
                .map(|catalogs| self.embedded.set_messages(locales, catalogs))
            }),
        )
    }

    /// What the host's row store honours for `entity_type`: `QueryCapabilities` and `WriteCapabilities` as JSON (`null` declares nothing).
    pub fn set_capabilities(
        &self,
        entity_type: &str,
        query_json: &str,
        write_json: &str,
    ) -> Result<(), String> {
        self.record(
            self.embedded
                .set_capabilities_json(entity_type, query_json, write_json),
        )
    }

    /// The kernel's mounts for a host start on `side` (`client|server|process|start`), `MountSeed[]` as JSON.
    pub fn boot_mounts(&self, side: &str) -> Result<String, String> {
        self.record(side_of(side).and_then(|side| to_json(&self.embedded.boot_mounts(side))))
    }

    /// The kernel's mounts for a client opening `route_path`, `MountSeed[]` as JSON.
    pub fn route_mounts(&self, route_path: &str) -> Result<String, String> {
        self.record(to_json(&self.embedded.route_mounts(route_path)))
    }

    /// The traits whose mount-scoped ticks fire (`string[]` as JSON): the union of what the host's clients mounted.
    pub fn set_tick_mount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json")
                .map(|t| self.embedded.set_tick_mount(t)),
        )
    }

    pub fn last_error(&self) -> String {
        self.last_error
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

/// Install the language data (pattern vocabulary, theme presets, operator table) a host without
/// `node_modules` hands over, once, before the first kernel is built.
pub fn install_language(bundle_json: &str) -> Result<(), String> {
    orbital_core::language_data::install_language_bundle(bundle_json)
        .map_err(|e| format!("{}: {e}", e.code()))
}

/// Whether the absolute `glob` matches `path`: the one-grant matcher `os/glob` filters with.
pub fn os_glob_matches(glob: &str, path: &str) -> bool {
    orbital_core::embed::os_glob_matches(glob, path)
}

/// The typed event request a `program/dispatch` call names (`EventRequest::from_dispatch`), as JSON: the payload and
/// the option record in, the request the kernel would dispatch out. A record that is no event request is the `Err`.
pub fn parse_event_request(payload_json: &str, options_json: &str) -> Result<String, String> {
    let payload: serde_json::Value = parse(payload_json, "payload_json")?;
    let options: serde_json::Map<String, serde_json::Value> = parse(options_json, "options_json")?;
    to_json(&orbital_core::EventRequest::from_dispatch(payload, |key| options.get(key).cloned())?)
}

/// Whether the Chrome match pattern `pattern` matches `url`.
pub fn tab_url_matches(pattern: &str, url: &str) -> bool {
    orbital_core::embed::tab_url_matches(pattern, url)
}

/// The names of the `os/` operations whose keyed call may defer its completion, as a JSON array.
pub fn os_deferring_ops() -> Result<String, String> {
    orbital_core::embed::os_deferring_ops_json()
}

pub fn os_stream_ops() -> Result<String, String> {
    orbital_core::embed::os_stream_ops_json()
}

/// The `{ event, payload }` a deferred keyed `os/` call's outcome (`Settled`) fires on its call site, or `null`.
pub fn os_completion_event(site_json: &str, key: &str, outcome_json: &str) -> Result<String, String> {
    orbital_core::embed::os_completion_event_json(site_json, key, outcome_json)
}

// Expression evaluation and row access: the kernel's one evaluator over JSON strings, for hosts that bind
// a render's expressions in one crossing. Each forwards to `orbital_core::embed::eval_wire` unchanged.

/// Evaluate `expressions_json` (`SExpr[]`) against `context_json` (`EvalContextWire`): `EvalOutcomeWire[]` as JSON, in order.
pub fn evaluate_many(context_json: &str, expressions_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::evaluate_many_json(context_json, expressions_json)
}

/// Resolve `values_json` (prop values) against `context_json`: every binding, embedded ones included, and every
/// operator call. `EvalOutcomeWire[]` as JSON, in order.
pub fn resolve_values(context_json: &str, values_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::resolve_values_json(context_json, values_json)
}

/// Evaluate `guards_json` (`SExpr[]`) against `context_json`: `GuardOutcomeWire[]` as JSON, in order.
pub fn evaluate_guards(context_json: &str, guards_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::evaluate_guards_json(context_json, guards_json)
}

/// Evaluate one expression with its trace: an `EvalTracedWire` as JSON.
pub fn evaluate_traced(context_json: &str, expression_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::evaluate_traced_json(context_json, expression_json)
}

/// A listen route's `with` mapping (`{ field: SExpr }`, empty for none) applied to the source payload: the mapped payload as JSON.
pub fn map_listen_payload(mapping_json: &str, payload_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::map_listen_payload_json(Some(mapping_json), payload_json)
}

/// The rows (`EntityRow[]` as JSON) that pass the `@read` policy and the call-site filter (each an `SExpr` or empty).
pub fn scope_rows(
    bindings_json: &str,
    rows_json: &str,
    policy_json: &str,
    filter_json: &str,
    entity_type: &str,
) -> Result<String, String> {
    orbital_core::embed::eval_wire::scope_rows_json(
        bindings_json,
        rows_json,
        Some(policy_json),
        Some(filter_json),
        entity_type,
    )
}

/// Whether `row_json` may undergo `operation` (`create`, `update` or `delete`) under `policy_json` (an `SExpr` or empty): a `MutationVerdictWire` as JSON.
pub fn check_mutation_access(
    operation: &str,
    bindings_json: &str,
    row_json: &str,
    policy_json: &str,
    entity_type: &str,
) -> Result<String, String> {
    orbital_core::embed::eval_wire::check_mutation_json(
        operation,
        bindings_json,
        row_json,
        Some(policy_json),
        entity_type,
    )
}

/// What a binding term folds to for a row predicate: a `RowBindingWire` as JSON.
pub fn resolve_row_binding(term: &str, bindings_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::resolve_row_binding_json(term, bindings_json)
}

// ---- traceEffects ----------------------------------------------------------------------------------------

/// Run `effects_json` (`SExpr[]`) in order against the row in `context_json` (`EvalContextWire`): a `TraceEffectsWire`
/// as JSON, with a trace per effect, the events raised, the writes made and the row after them.
pub fn trace_effects(context_json: &str, effects_json: &str) -> Result<String, String> {
    orbital_core::embed::eval_wire::trace_effects_json(context_json, effects_json)
}

// ---- credentialSeedPlan ----------------------------------------------------------------------------------

/// The credentials a `store`-source boot copies from the env into the store (`[{ service, envVar }]` as JSON): the
/// declared ones the store does not hold and the env sets, each once. Only names cross; the host reads each value from its own env.
#[cfg(feature = "full")]
pub fn credential_seed_plan(declared_json: &str, held_json: &str, present_json: &str) -> Result<String, String> {
    orbital_core::credential_admin::seed_plan_json(declared_json, held_json, present_json)
}

pub fn substrate_completion_event(site_json: &str, key: &str, op: &str, args_json: &str, outcome_json: &str) -> Result<String, String> {
    orbital_core::embed::substrate_completion_event_json(site_json, key, op, args_json, outcome_json)
}
