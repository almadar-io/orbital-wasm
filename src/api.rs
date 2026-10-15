//! The host-facing operations, one per C-ABI operation, as plain functions over JSON strings.
//! `Kernel` (wasm-bindgen) forwards to these unchanged, so a native test drives the same code the
//! JS host calls. Marshalling only: every decision is `orbital_core::embed`'s.

use orbital_core::effects::MountSeed;
use orbital_core::embed::{parse_theme_mode, EmbeddedKernel, EmbeddedServer, FoldBase};
use orbital_core::runtime::TraitSnapshot;
use orbital_core::{EventResponse, OrbitalSchema, Value};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Mutex;

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

impl KernelApi {
    pub fn new(resolved_schema_json: &str, client_id: &str) -> Result<Self, String> {
        let schema: OrbitalSchema = parse(resolved_schema_json, "resolved schema")?;
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

    pub fn mount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.mount(t)),
        )
    }

    pub fn unmount(&self, traits_json: &str) -> Result<(), String> {
        self.record(
            parse::<Vec<String>>(traits_json, "traits_json").map(|t| self.embedded.unmount(t)),
        )
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
    pub fn new(resolved_schema_json: &str) -> Result<Self, String> {
        let schema: OrbitalSchema = parse(resolved_schema_json, "resolved schema")?;
        Ok(Self {
            embedded: EmbeddedServer::new(schema)?,
            last_error: Mutex::new(String::new()),
        })
    }

    fn record<T>(&self, result: Result<T, String>) -> Result<T, String> {
        if let Err(e) = &result {
            if let Ok(mut slot) = self.last_error.lock() {
                *slot = e.clone();
            }
        }
        result
    }

    /// Start a request against `orbital`; a `ServerProgressWire` as JSON.
    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, String> {
        self.record(self.embedded.start_json(orbital, request_json))
    }

    /// Answer the requests a flight is waiting on (`IoReplyWire[]` as JSON); a `ServerProgressWire` as JSON.
    pub fn resume(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.record(self.embedded.resume_json(flight, replies_json))
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
