//! A server whose kernel answers its own row requests from a store the host selects, so the host answers only the
//! rest (http, `os/`, secrets, credentials, clock, random, services). The wire is [`ServerApi`]'s: a flight that
//! still waits on the host lists only the requests the host owns, and `resume` takes only those answers.

use crate::api::ServerApi;
use orbital_core::effects::LiveBroadcastItem;
use orbital_core::embed::wire::{IoAnswerWire, IoRequestWire};
use orbital_core::runtime::auth::SignIn;
use orbital_core::runtime::entity_source::EntitySource;
use orbital_core::runtime::resumable::{BlockingHost, IoAnswer, IoRequest};
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

/// Answers any ask on this machine, or declines it (`None`) so the host answers it.
pub type LocalAnswers = Arc<dyn Fn(&IoRequest) -> Option<IoAnswer> + Send + Sync>;

/// The prefix of an error that is a refused bearer, not a failed dispatch.
pub const UNAUTHORIZED: &str = "ORB_UNAUTHORIZED";

/// Receives each live item a dispatch produced; the host routes it to connections with [`ServerHandle::live_recipients`].
pub type LiveSink = Arc<dyn Fn(LiveBroadcastItem) + Send + Sync>;

thread_local! {
    /// The client whose request this thread is serving: the origin of the live messages its calls make.
    static ORIGIN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Serves `client_id`'s request from this thread until it drops.
struct OriginGuard(Option<String>);

impl OriginGuard {
    fn enter(client_id: Option<String>) -> Self {
        Self(ORIGIN.with(|slot| slot.replace(client_id)))
    }
}

impl Drop for OriginGuard {
    fn drop(&mut self) {
        ORIGIN.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

fn client_of(request_json: &str) -> Option<String> {
    serde_json::from_str::<Value>(request_json)
        .ok()?
        .get("clientId")?
        .as_str()
        .map(str::to_string)
}

pub struct StoredServer {
    api: ServerApi,
    live: Option<LiveSink>,
    source: Arc<dyn EntitySource>,
    local: Option<LocalAnswers>,
    sign_in: Option<SignIn>,
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
    ) -> Result<Self, String> {
        let api = ServerApi::new(resolved_schema_json)?;
        let schema: OrbitalSchema = serde_json::from_str(resolved_schema_json)
            .map_err(|e| format!("parse resolved schema: {e}"))?;
        let (query, write) = (json(&capabilities.query)?, json(&capabilities.write)?);
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
                api.set_capabilities(&def.name, &query, &write)?;
            }
        }
        Ok(Self {
            api,
            live: None,
            source,
            local: None,
            sign_in: None,
            held: Mutex::new(HashMap::new()),
            held_resolve: Mutex::new(HashMap::new()),
        })
    }

    /// The server over the store the environment selects (`DATA_BACKEND`, `DATABASE_URL`, `COUCHDB_URL`, the Firebase
    /// variables: the `@almadar/db` contract), its drivers speaking over the host's network.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_env(
        resolved_schema_json: &str,
        env: &HashMap<String, String>,
    ) -> Result<Self, String> {
        use orbital_core::runtime::native_transport::UreqTransport;
        use orbital_core::runtime::store_config::row_store_from_env;
        let store = row_store_from_env(
            &|name| env.get(name).cloned(),
            &|path| std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}")),
            Arc::new(UreqTransport::unchecked()),
        )?;
        Self::new(
            resolved_schema_json,
            store.source,
            StoreCapabilities {
                query: store.query,
                write: store.write,
            },
        )
    }

    /// The server answering every ask on this machine, as `orb serve` does: rows from the store the environment
    /// selects, `http` and `os/` under the program's own egress and grants, secrets and credentials from the
    /// environment, the clock and random bytes, and the refusals of a host with no providers. `on_live` receives each
    /// live item a dispatch produces: a streaming call's messages (to the requesting connection) and the `persist`
    /// successes (to every other), addressed as `orb serve` addresses them.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_env_native(
        resolved_schema_json: &str,
        env: &HashMap<String, String>,
        on_live: LiveSink,
    ) -> Result<Self, String> {
        use orbital_core::runtime::credentials::credentials_from_env;
        use orbital_core::runtime::native_host::NativeHost;
        use orbital_core::runtime::native_transport::UreqTransport;
        use orbital_core::runtime::os_host::OsHandles;
        use orbital_core::runtime::store_config::row_store_from_env;
        let store = row_store_from_env(
            &|name| env.get(name).cloned(),
            &|path| std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}")),
            Arc::new(UreqTransport::unchecked()),
        )?;
        let source = store.source.clone();
        let mut server = Self::new(
            resolved_schema_json,
            store.source,
            StoreCapabilities {
                query: store.query,
                write: store.write,
            },
        )?;
        let (egress, grants) = {
            let embedded = server.api.embedded();
            (embedded.kernel().egress(), embedded.os_grants())
        };
        let sink = on_live.clone();
        let on_event: Arc<orbital_core::runtime::native_host::StreamEvents> =
            Arc::new(move |site, payload| {
                let Some(site) = site else { return };
                let Some(event) = site.emit.on_message.clone() else {
                    return;
                };
                let origin = ORIGIN.with(|slot| slot.borrow().clone());
                sink(LiveBroadcastItem::call_message(
                    event,
                    payload,
                    site.trait_name.clone(),
                    origin,
                ));
            });
        let credentials = Arc::new(credentials_from_env());
        let finder = source.clone();
        let host = Arc::new(NativeHost::new(
            source,
            Arc::new(UreqTransport::with_egress(egress.clone())),
            credentials.clone(),
            egress,
            grants,
            Arc::new(OsHandles::default()),
            on_event,
        ));
        server.local = Some(Arc::new(move |request| Some(host.answer(request))));
        let schema: OrbitalSchema = serde_json::from_str(resolved_schema_json)
            .map_err(|e| format!("parse resolved schema: {e}"))?;
        let clock: orbital_core::runtime::auth::Clock =
            Arc::new(orbital_core::time_util::now_millis);
        server.sign_in = orbital_core::runtime::auth::sign_in_from_env(
            env,
            &schema,
            &orbital_core::runtime::native_transport::feed_host,
            credentials,
            clock,
            Arc::new(move |entity, id| {
                finder
                    .find(entity, id)
                    .map(|row| row.and_then(|row| serde_json::to_value(&*row).ok()))
            }),
        )?;
        server.live = Some(on_live);
        Ok(server)
    }

    /// Everything of the server that is not a flight: state, mounts, messages, grants, the last error.
    pub fn api(&self) -> &ServerApi {
        &self.api
    }

    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, String> {
        let client = client_of(request_json);
        let _serving = OriginGuard::enter(client.clone());
        let progress = self.api.start(orbital, request_json)?;
        let done = self.pump(progress, Flight::Dispatch)?;
        self.publish_persisted(&done, client.as_deref());
        Ok(done)
    }

    pub fn resume(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        self.go_on(flight, replies_json, Flight::Dispatch)
    }

    /// The viewer an `Authorization` header resolves to, as JSON: the user the program's sign-in makes of a verified
    /// bearer, the anonymous viewer for no header, and `null` when the program declares no sign-in (a viewer-less
    /// server). A bearer that does not verify, or one on a server that cannot verify it, is an `UNAUTHORIZED` error.
    pub fn viewer_json(&self, authorization: Option<&str>) -> Result<String, String> {
        let Some(sign_in) = &self.sign_in else {
            return match authorization {
                None => Ok("null".to_string()),
                Some(_) => Err(format!(
                    "{UNAUTHORIZED}: this server declares no sign-in, so it verifies no bearer"
                )),
            };
        };
        let viewer = sign_in
            .viewer(authorization, true)
            .map_err(|e| format!("{UNAUTHORIZED}: {e}"))?;
        json(&viewer)
    }

    /// [`Self::start`] as the viewer `authorization` names. The request's own `user` is never read: it is dropped, and the
    /// viewer is the verified bearer's (the anonymous viewer without one).
    pub fn start_as(
        &self,
        orbital: &str,
        request_json: &str,
        authorization: Option<&str>,
    ) -> Result<String, String> {
        let mut request: Value =
            serde_json::from_str(request_json).map_err(|e| format!("parse request_json: {e}"))?;
        let Some(fields) = request.as_object_mut() else {
            return Err("request_json must be an object".to_string());
        };
        fields.remove("user");
        let viewer: Value =
            serde_json::from_str(&self.viewer_json(authorization)?).map_err(|e| e.to_string())?;
        if !viewer.is_null() {
            fields.insert("user".to_string(), viewer);
        }
        self.start(orbital, &request.to_string())
    }

    fn publish_persisted(&self, progress: &str, client_id: Option<&str>) {
        let Some(live) = &self.live else { return };
        let Ok(value) = serde_json::from_str::<Value>(progress) else {
            return;
        };
        let Some(response) = value.get("response").cloned() else {
            return;
        };
        let Ok(response) = serde_json::from_value::<orbital_core::EventResponse>(response) else {
            return;
        };
        for item in LiveBroadcastItem::persisted(&response, client_id) {
            live(item);
        }
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
                let answer = match &self.local {
                    Some(local) => local(&request),
                    None => None,
                }
                .or_else(|| {
                    is_row_request(&wire).then(|| {
                        BlockingHost {
                            source: self.source.as_ref(),
                            service: None,
                            substrate: None,
                        }
                        .answer(&request)
                    })
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

/// A server a native binding holds: the plain one, whose host answers every ask, or one that answers its own rows.
pub enum ServerHandle {
    Plain(ServerApi),
    Stored(StoredServer),
}

impl ServerHandle {
    pub fn plain(resolved_schema_json: &str) -> Result<Self, String> {
        ServerApi::new(resolved_schema_json).map(Self::Plain)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_env(
        resolved_schema_json: &str,
        env: &HashMap<String, String>,
    ) -> Result<Self, String> {
        StoredServer::from_env(resolved_schema_json, env).map(Self::Stored)
    }

    /// The server answering every ask on this machine ([`StoredServer::from_env_native`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_env_native(
        resolved_schema_json: &str,
        env: &HashMap<String, String>,
        on_live: LiveSink,
    ) -> Result<Self, String> {
        StoredServer::from_env_native(resolved_schema_json, env, on_live).map(Self::Stored)
    }

    /// [`StoredServer::viewer_json`]; a server that answers through its host verifies no bearer.
    pub fn viewer_json(&self, authorization: Option<&str>) -> Result<String, String> {
        match self {
            Self::Stored(stored) => stored.viewer_json(authorization),
            Self::Plain(_) => Err(format!("{UNAUTHORIZED}: this server verifies no bearer")),
        }
    }

    /// [`StoredServer::start_as`].
    pub fn start_as(
        &self,
        orbital: &str,
        request_json: &str,
        authorization: Option<&str>,
    ) -> Result<String, String> {
        match self {
            Self::Stored(stored) => stored.start_as(orbital, request_json, authorization),
            Self::Plain(_) => Err(format!("{UNAUTHORIZED}: this server verifies no bearer")),
        }
    }

    /// Which of `connections` (`[{ "clientId": string | null, "viewer": user | null }]` as JSON) receive the live item
    /// `item_json`, as indexes into it (a JSON array): the kernel's one routing rule — an origin item to the
    /// requesting connection only, a peers item to every other connection whose viewer may read its row.
    pub fn live_recipients(
        &self,
        item_json: &str,
        connections_json: &str,
    ) -> Result<String, String> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Connection {
            client_id: Option<String>,
            viewer: Option<Value>,
        }
        let item: LiveBroadcastItem =
            serde_json::from_str(item_json).map_err(|e| format!("parse item_json: {e}"))?;
        let connections: Vec<Connection> = serde_json::from_str(connections_json)
            .map_err(|e| format!("parse connections_json: {e}"))?;
        let kernel = self.api().embedded().kernel();
        let delivered: Vec<usize> = connections
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                item.delivered_to(c.client_id.as_deref(), |item| match &c.viewer {
                    Some(viewer) => serde_json::from_value::<orbital_core::Value>(viewer.clone())
                        .is_ok_and(|viewer| kernel.live_item_visible_to(item, &viewer)),
                    None => true,
                })
            })
            .map(|(at, _)| at)
            .collect();
        json(&delivered)
    }

    /// Everything of the server that is not a flight.
    pub fn api(&self) -> &ServerApi {
        match self {
            Self::Plain(api) => api,
            Self::Stored(stored) => stored.api(),
        }
    }

    pub fn start(&self, orbital: &str, request_json: &str) -> Result<String, String> {
        match self {
            Self::Plain(api) => api.start(orbital, request_json),
            Self::Stored(stored) => stored.start(orbital, request_json),
        }
    }

    pub fn resume(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        match self {
            Self::Plain(api) => api.resume(flight, replies_json),
            Self::Stored(stored) => stored.resume(flight, replies_json),
        }
    }

    pub fn resolve_rows(&self, ref_json: &str, viewer_json: &str) -> Result<String, String> {
        match self {
            Self::Plain(api) => api.resolve_rows(ref_json, viewer_json),
            Self::Stored(stored) => stored.resolve_rows(ref_json, viewer_json),
        }
    }

    pub fn resume_resolve(&self, flight: u64, replies_json: &str) -> Result<String, String> {
        match self {
            Self::Plain(api) => api.resume_resolve(flight, replies_json),
            Self::Stored(stored) => stored.resume_resolve(flight, replies_json),
        }
    }

    pub fn abandon(&self, flight: u64) -> bool {
        match self {
            Self::Plain(api) => api.abandon(flight),
            Self::Stored(stored) => stored.abandon(flight),
        }
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
