//! A server whose kernel answers its own row requests from a store the host gives it, so the host answers only the
//! rest (http, `os/`, secrets, credentials, clock, random, services). The wire is [`ServerApi`]'s: a flight that
//! still waits on the host lists only the requests the host owns, and `resume` takes only those answers.

use crate::api::{ApiError, ServerApi};
use orbital_core::runtime::kernel::EntryError;
use orbital_core::embed::wire::{IoAnswerWire, IoRequestWire};
use orbital_core::runtime::entity_source::EntitySource;
use orbital_core::runtime::resumable::{BlockingHost, IoRequest};
use orbital_core::runtime::row_query::{QueryCapabilities, WriteCapabilities};
use orbital_core::{EntityRef, OrbitalSchema};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Which flights are held: a dispatch's or a row-set resolve's (their ids are separate counters).
#[derive(Clone, Copy)]
enum Flight {
    Dispatch,
    Resolve,
}

/// What the host's store honours, declared to the kernel for every inline entity of the program.
pub struct StoreCapabilities {
    pub query: Option<QueryCapabilities>,
    pub write: Option<WriteCapabilities>,
}

pub struct StoredServer {
    api: ServerApi,
    source: Arc<dyn EntitySource>,
    held: Mutex<HashMap<u64, Vec<Value>>>,
    held_resolve: Mutex<HashMap<u64, Vec<Value>>>,
}

fn is_row_request(request: &IoRequestWire) -> bool {
    matches!(
        request,
        IoRequestWire::ReadRow { .. }
            | IoRequestWire::ListRows { .. }
            | IoRequestWire::QueryRows { .. }
            | IoRequestWire::WriteRows { .. }
            | IoRequestWire::CommitRows { .. }
            | IoRequestWire::CreateRow { .. }
            | IoRequestWire::WriteRow { .. }
            | IoRequestWire::DeleteRow { .. }
    )
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("serialize: {e}"))
}

impl StoredServer {
    pub fn new(
        resolved_schema_json: &str,
        source: Arc<dyn EntitySource>,
        capabilities: StoreCapabilities,
    ) -> Result<Self, ApiError> {
        let api = ServerApi::new(resolved_schema_json)?;
        let schema: OrbitalSchema = serde_json::from_str(resolved_schema_json)
            .map_err(|e| ApiError::Marshal(format!("parse resolved schema: {e}")))?;
        let (query, write) = (
            json(&capabilities.query).map_err(ApiError::Marshal)?,
            json(&capabilities.write).map_err(ApiError::Marshal)?,
        );
        for orbital in &schema.orbitals {
            let entities =
                std::iter::once(&orbital.entity).chain(orbital.auxiliary_entities.iter().flatten());
            for entity in entities {
                let EntityRef::Inline(def) = entity else {
                    continue;
                };
                if let Some(collection) = &def.collection {
                    source.bind_collection(&def.name, collection);
                }
                api.set_capabilities(&def.name, &query, &write).map_err(ApiError::Marshal)?;
            }
        }
        Ok(Self {
            api,
            source,
            held: Mutex::new(HashMap::new()),
            held_resolve: Mutex::new(HashMap::new()),
        })
    }

    /// Everything of the server that is not a flight: state, mounts, messages, grants, the last error.
    pub fn api(&self) -> &ServerApi {
        &self.api
    }

    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, String> {
        let progress = self.api.start(orbital, request_json)?;
        self.pump(progress, Flight::Dispatch)
    }

    pub fn resume(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.go_on(flight, replies_json, Flight::Dispatch)
    }

    /// [`ServerApi::next`], the started request's row asks answered from the store.
    pub fn next(&self) -> Result<String, String> {
        let next: Value = serde_json::from_str(&self.api.next()?).map_err(|e| format!("parse next: {e}"))?;
        let Value::Object(mut started) = next else {
            return json(&next);
        };
        if let Some(progress) = started.remove("progress") {
            let pumped = self.pump(json(&progress)?, Flight::Dispatch)?;
            started.insert("progress".to_string(), serde_json::from_str(&pumped).map_err(|e| format!("parse progress: {e}"))?);
        }
        json(&started)
    }

    pub fn holds_lane(&self, flight: u64) -> bool {
        self.api.holds_lane(flight)
    }

    pub fn open_session_stream(&self, client: &str) -> u64 {
        self.api.open_session_stream(client)
    }

    pub fn close_session_stream(&self, id: u64) -> bool {
        self.api.close_session_stream(id)
    }

    pub fn resolve_rows(&self, ref_json: &str, viewer_json: &str) -> Result<String, String> {
        let progress = self.api.resolve_rows(ref_json, viewer_json)?;
        self.pump(progress, Flight::Resolve)
    }

    pub fn resume_resolve(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.go_on(flight, replies_json, Flight::Resolve)
    }

    pub fn abandon(&self, flight: u64) -> bool {
        self.held_of(Flight::Dispatch).remove(&flight);
        self.held_of(Flight::Resolve).remove(&flight);
        self.api.abandon(flight)
    }

    fn held_of(&self, kind: Flight) -> std::sync::MutexGuard<'_, HashMap<u64, Vec<Value>>> {
        let slot = match kind {
            Flight::Dispatch => &self.held,
            Flight::Resolve => &self.held_resolve,
        };
        slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn go_on(&self, flight: u64, host_replies_json: &str, kind: Flight) -> Result<String, String> {
        let host: Vec<Value> = serde_json::from_str(host_replies_json)
            .map_err(|e| format!("parse replies_json: {e}"))?;
        let mut replies = self.held_of(kind).remove(&flight).unwrap_or_default();
        replies.extend(host);
        let replies_json = json(&replies)?;
        let progress = match kind {
            Flight::Dispatch => self.api.resume(flight, &replies_json)?,
            Flight::Resolve => self.api.resume_resolve(flight, &replies_json)?,
        };
        self.pump(progress, kind)
    }

    /// Answer the row requests of an awaiting progress from the store, resuming while only rows were asked, and
    /// return the first progress that is done or still waits on the host.
    fn pump(&self, mut progress: String, kind: Flight) -> Result<String, String> {
        loop {
            let mut value: Value =
                serde_json::from_str(&progress).map_err(|e| format!("parse progress: {e}"))?;
            if value.get("status").and_then(Value::as_str) != Some("await") {
                return Ok(progress);
            }
            let flight = value
                .get("flight")
                .and_then(Value::as_u64)
                .ok_or("an awaiting progress has no flight")?;
            let requests = value
                .get_mut("requests")
                .and_then(Value::as_array_mut)
                .ok_or("an awaiting progress has no requests")?;
            let mut answered = self.held_of(kind).remove(&flight).unwrap_or_default();
            let mut host_owned = Vec::new();
            for pending in std::mem::take(requests) {
                let wire: IoRequestWire = serde_json::from_value(
                    pending
                        .get("request")
                        .cloned()
                        .ok_or("a pending request has no request")?,
                )
                .map_err(|e| format!("parse request: {e}"))?;
                let request = IoRequest::from(wire.clone());
                let answer = is_row_request(&wire).then(|| {
                    BlockingHost {
                        source: self.source.as_ref(),
                        service: None,
                        substrate: None,
                    }
                    .answer(&request)
                });
                let Some(answer) = answer else {
                    host_owned.push(pending);
                    continue;
                };
                let id = pending
                    .get("id")
                    .cloned()
                    .ok_or("a pending request has no id")?;
                answered.push(serde_json::json!({ "id": id, "answer": serde_json::to_value(IoAnswerWire::from(&answer))
                    .map_err(|e| format!("serialize: {e}"))? }));
            }
            if !host_owned.is_empty() {
                *requests = host_owned;
                self.held_of(kind).insert(flight, answered);
                return json(&value);
            }
            let replies_json = json(&answered)?;
            progress = match kind {
                Flight::Dispatch => self.api.resume(flight, &replies_json)?,
                Flight::Resolve => self.api.resume_resolve(flight, &replies_json)?,
            };
        }
    }
}

/// A server a native binding holds: the host answers every ask.
pub struct ServerHandle(ServerApi);

impl ServerHandle {
    /// The server's fork ([`ServerApi::fork`]).
    pub fn fork(&self) -> Result<Self, EntryError> {
        self.0.fork().map(Self)
    }

    pub fn fork_program(&self, program_json: &str) -> Result<Self, ApiError> {
        self.0.fork_program(program_json).map(Self)
    }

    pub fn plain(resolved_schema_json: &str) -> Result<Self, ApiError> {
        ServerApi::new(resolved_schema_json).map(Self)
    }

    /// Everything of the server that is not a flight.
    pub fn api(&self) -> &ServerApi {
        &self.0
    }

    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, String> {
        self.0.start(orbital, request_json)
    }

    pub fn resume(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.0.resume(flight, replies_json)
    }

    pub fn next(&self) -> Result<String, String> {
        self.0.next()
    }

    pub fn holds_lane(&self, flight: u64) -> bool {
        self.0.holds_lane(flight)
    }

    pub fn open_session_stream(&self, client: &str) -> u64 {
        self.0.open_session_stream(client)
    }

    pub fn close_session_stream(&self, id: u64) -> bool {
        self.0.close_session_stream(id)
    }

    pub fn resolve_rows(&self, ref_json: &str, viewer_json: &str) -> Result<String, String> {
        self.0.resolve_rows(ref_json, viewer_json)
    }

    pub fn resume_resolve(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.0.resume_resolve(flight, replies_json)
    }

    pub fn abandon(&self, flight: u64) -> bool {
        self.0.abandon(flight)
    }
}

/// The server handle `free()` empties; any call after that is an error.
pub struct Slot<T>(Mutex<Option<Arc<T>>>);

impl<T> Slot<T> {
    pub fn new(value: T) -> Self {
        Slot(Mutex::new(Some(Arc::new(value))))
    }

    pub fn take(&self) {
        if let Ok(mut held) = self.0.lock() {
            held.take();
        }
    }

    pub fn get(&self) -> Result<Arc<T>, String> {
        self.0
            .lock()
            .ok()
            .and_then(|held| held.clone())
            .ok_or_else(|| "the server was freed".to_string())
    }
}
