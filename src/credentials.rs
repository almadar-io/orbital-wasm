//! The credential vault for JavaScript hosts: `orbital_core`'s `EncryptedCredentialStore` over rows the host lists and
//! applies. The store is synchronous and a host's medium is not, so the vault keeps the rows the host last listed in memory
//! and records every change as a write; the host lists, `load`s, runs an operation, then applies `take_writes` to its
//! medium in order. The master key is given by the host and the IVs are the kernel's own. Every rule is the store's.

use orbital_core::runtime::credential_store::{
    is_master_key, EncryptedCredentialStore, RecordedCredentialRows,
};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The service label a kernel-persisted credential (an OAuth connection) carries in the store.
const PERSISTED_SERVICE: &str = "kernel";

pub struct VaultApi {
    store: EncryptedCredentialStore,
    rows: Arc<RecordedCredentialRows>,
    loaded: AtomicBool,
}

fn out<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("serialize: {e}"))
}

impl VaultApi {
    /// A vault with the host's master key (64 hex digits) and, during a rotation window, the previous one. A key that is
    /// not 64 hex digits is refused; no key at all is a vault that reads and writes nothing (`enabled` is false).
    pub fn new(master: Option<&str>, previous: Option<&str>) -> Result<Self, String> {
        for (what, key) in [("master", master), ("previous master", previous)] {
            if key.is_some_and(|key| !is_master_key(key)) {
                return Err(format!("the {what} credential key must be 64 hex digits"));
            }
        }
        let rows = Arc::new(RecordedCredentialRows::default());
        Ok(Self {
            store: EncryptedCredentialStore::new(rows.clone(), master, previous),
            rows,
            loaded: AtomicBool::new(false),
        })
    }

    pub fn enabled(&self) -> bool {
        self.store.enabled()
    }

    /// Replace the rows with the ones the host just listed (a JSON array) and read them. A row that cannot be read fails
    /// the load, naming the row ids; until a load succeeds every read and write fails.
    pub fn load(&self, rows_json: &str) -> Result<usize, String> {
        let rows: Vec<Value> =
            serde_json::from_str(rows_json).map_err(|e| format!("parse rows_json: {e}"))?;
        self.rows.load(rows);
        self.loaded.store(true, Ordering::SeqCst);
        self.store.warm()
    }

    fn loaded(&self) -> Result<(), String> {
        if self.loaded.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("the credential rows have not been loaded".to_string())
        }
    }

    fn ready(&self) -> Result<(), String> {
        self.loaded()?;
        if self.store.warmed() {
            Ok(())
        } else {
            Err("the credential rows could not be read; load them again".to_string())
        }
    }

    pub fn resolve(&self, name: &str, principal: Option<&str>) -> Result<Option<String>, String> {
        self.ready()?;
        Ok(self.store.resolve(name, principal))
    }

    pub fn label(&self, name: &str, principal: Option<&str>) -> Result<Option<String>, String> {
        self.ready()?;
        Ok(self.store.label(name, principal))
    }

    /// The app-wide values as `{ NAME: value }`.
    pub fn snapshot_env(&self) -> Result<String, String> {
        self.ready()?;
        out(&self.store.snapshot_env())
    }

    /// Masked entries of `principal`'s credentials (or the app-wide ones): `[{ service, envVar, last4, updatedAt, principal? }]`.
    pub fn entries(&self, principal: Option<&str>) -> Result<String, String> {
        self.ready()?;
        let entries: Vec<Value> = self
            .store
            .entries(principal)
            .into_iter()
            .map(|entry| {
                let mut shown = Map::new();
                shown.insert("service".into(), json!(entry.service));
                shown.insert("envVar".into(), json!(entry.env_var));
                shown.insert("last4".into(), json!(entry.last4));
                shown.insert("updatedAt".into(), json!(entry.updated_at));
                if let Some(who) = entry.principal {
                    shown.insert("principal".into(), json!(who));
                }
                Value::Object(shown)
            })
            .collect();
        out(&entries)
    }

    /// `{ enabled, entries: [{ service, name }] }` of the app-wide rows, never a value.
    pub fn listing(&self) -> Result<String, String> {
        self.ready()?;
        out(&self.store.listing()?)
    }

    /// Upsert one credential; the masked entry. `label` is a non-secret account name kept in the clear beside it.
    pub fn set(
        &self,
        service: &str,
        name: &str,
        value: &str,
        principal: Option<&str>,
        label: Option<&str>,
    ) -> Result<String, String> {
        self.ready()?;
        let entry = self
            .store
            .set_labeled(service, name, value, principal, label)?;
        let mut shown = Map::new();
        shown.insert("service".into(), json!(entry.service));
        shown.insert("envVar".into(), json!(entry.env_var));
        shown.insert("last4".into(), json!(entry.last4));
        shown.insert("updatedAt".into(), json!(entry.updated_at));
        if let Some(who) = entry.principal {
            shown.insert("principal".into(), json!(who));
        }
        out(&Value::Object(shown))
    }

    /// Replace the value of a credential already stored (a rotated refresh token), keeping its service; the masked entry.
    pub fn replace(
        &self,
        name: &str,
        value: &str,
        principal: Option<&str>,
    ) -> Result<String, String> {
        self.ready()?;
        let held = self
            .store
            .entries(principal)
            .into_iter()
            .find(|entry| entry.env_var == name)
            .ok_or_else(|| format!("credential {name} is not in the credential store"))?;
        self.set(&held.service, name, value, principal, None)
    }

    /// Keep `value` as the credential until it is forgotten, creating it when absent (an OAuth connection's tokens); the
    /// masked entry. `label` is a non-secret account name stored beside it.
    pub fn persist(
        &self,
        name: &str,
        value: &str,
        principal: Option<&str>,
        label: Option<&str>,
    ) -> Result<String, String> {
        self.set(PERSISTED_SERVICE, name, value, principal, label)
    }

    /// Whether a row existed.
    pub fn remove(&self, name: &str, principal: Option<&str>) -> Result<bool, String> {
        self.ready()?;
        self.store.remove(name, principal)
    }

    /// `{ rotated, alreadyCurrent, unreadable }`; a row that cannot be read fails the rotation before any row changes.
    pub fn rotate(&self) -> Result<String, String> {
        self.loaded()?;
        out(&self.store.rotate()?)
    }

    /// Keep `value` sealed as a one-use value of `name` (for `principal`) until `expires_at` (epoch ms); `now` sweeps expired ones.
    pub fn hold(
        &self,
        name: &str,
        value: &str,
        expires_at: f64,
        now: f64,
        principal: Option<&str>,
    ) -> Result<(), String> {
        self.loaded()?;
        self.store
            .hold(name, value, expires_at as i64, now as i64, principal)
    }

    /// Find the one-use value held for `name` (for `principal`) without opening it: `{ ticket, write: "delete", id,
    /// ifPresent: true }` for the host to run as a conditional delete, or none held. Only a host whose delete removed the
    /// row calls `take_finish`; one that found it gone calls `take_cancel`.
    pub fn take_begin(
        &self,
        name: &str,
        principal: Option<&str>,
    ) -> Result<Option<String>, String> {
        self.loaded()?;
        Ok(self.store.take_begin(name, principal)?.map(|held| {
            json!({ "ticket": held.ticket, "write": "delete", "id": held.id, "ifPresent": true }).to_string()
        }))
    }

    /// Open the value of the row the host's conditional delete removed; an expired one is consumed and not returned.
    pub fn take_finish(&self, ticket: f64, now: f64) -> Result<Option<String>, String> {
        self.loaded()?;
        self.store.take_finish(ticket as u64, now as i64)
    }

    pub fn take_cancel(&self, ticket: f64) -> Result<(), String> {
        self.loaded()?;
        self.store.take_cancel(ticket as u64);
        Ok(())
    }

    /// The writes since the last call, in order, for the host to apply to its medium:
    /// `[{ write: "create", id, data } | { write: "update", id, data } | { write: "delete", id }]`. A created row carries
    /// the id in `data`; an update merges `data` over the row.
    pub fn take_writes(&self) -> Result<String, String> {
        out(&self.rows.take_writes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(digit: &str) -> String {
        digit.repeat(32)
    }

    /// A host's medium: the rows it keeps, with the writes of a vault applied in order.
    #[derive(Default)]
    struct Medium(Vec<Value>);

    impl Medium {
        fn apply(&mut self, writes: &str) {
            let writes: Vec<Value> = serde_json::from_str(writes).unwrap();
            for write in writes {
                let id = write["id"].as_str().unwrap().to_string();
                match write["write"].as_str().unwrap() {
                    "create" => self.0.push(write["data"].clone()),
                    "update" => {
                        let row = self
                            .0
                            .iter_mut()
                            .find(|row| row["id"] == id.as_str())
                            .unwrap();
                        for (field, value) in write["data"].as_object().unwrap() {
                            row[field.as_str()] = value.clone();
                        }
                    }
                    "delete" => self.0.retain(|row| row["id"] != id.as_str()),
                    other => panic!("unknown write {other}"),
                }
            }
        }

        /// The conditional delete a host's medium answers: whether this call removed the row.
        fn delete_if_present(&mut self, id: &str) -> bool {
            let before = self.0.len();
            self.0.retain(|row| row["id"] != id);
            self.0.len() != before
        }

        fn json(&self) -> String {
            serde_json::to_string(&self.0).unwrap()
        }

        fn vault(&self, master: &str, previous: Option<&str>) -> VaultApi {
            let vault = VaultApi::new(Some(master), previous).unwrap();
            vault.load(&self.json()).unwrap();
            vault
        }
    }

    #[test]
    fn a_credential_set_through_the_vault_is_read_back_by_a_later_vault_over_the_rows_the_host_applied(
    ) {
        let mut medium = Medium::default();
        let vault = medium.vault(&key("11"), None);
        let entry: Value = serde_json::from_str(
            &vault
                .set(
                    "stripe",
                    "STRIPE_KEY",
                    "sk-1234",
                    None,
                    Some("billing@example.com"),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(entry["last4"], "1234");
        assert_eq!(
            vault.resolve("STRIPE_KEY", None).unwrap().as_deref(),
            Some("sk-1234")
        );
        medium.apply(&vault.take_writes().unwrap());
        assert!(
            !medium.json().contains("sk-1234"),
            "the medium holds ciphertext only"
        );
        assert!(
            medium.json().contains("billing@example.com"),
            "the label is kept in the clear"
        );

        let later = medium.vault(&key("11"), None);
        assert_eq!(
            later.resolve("STRIPE_KEY", None).unwrap().as_deref(),
            Some("sk-1234")
        );
        assert_eq!(
            later.label("STRIPE_KEY", None).unwrap().as_deref(),
            Some("billing@example.com")
        );
        assert_eq!(
            later.resolve("STRIPE_KEY", Some("someone")).unwrap(),
            None,
            "a user slot never borrows the app value"
        );
        let listing: Value = serde_json::from_str(&later.listing().unwrap()).unwrap();
        assert_eq!(
            listing,
            json!({ "enabled": true, "entries": [{ "service": "stripe", "name": "STRIPE_KEY" }] })
        );

        assert!(later.remove("STRIPE_KEY", None).unwrap());
        medium.apply(&later.take_writes().unwrap());
        assert_eq!(
            medium
                .vault(&key("11"), None)
                .resolve("STRIPE_KEY", None)
                .unwrap(),
            None
        );
    }

    #[test]
    fn replace_keeps_the_service_and_needs_a_stored_row_and_persist_creates_it_under_the_kernel_service(
    ) {
        let mut medium = Medium::default();
        let vault = medium.vault(&key("11"), None);
        assert!(vault
            .replace("TOKEN", "v", None)
            .unwrap_err()
            .contains("not in the credential store"));
        vault
            .set("google", "TOKEN", "old-0001", None, None)
            .unwrap();
        vault.replace("TOKEN", "new-0002", None).unwrap();
        assert_eq!(
            vault.resolve("TOKEN", None).unwrap().as_deref(),
            Some("new-0002")
        );
        let listing: Value = serde_json::from_str(&vault.listing().unwrap()).unwrap();
        assert_eq!(listing["entries"][0]["service"], "google");
        vault
            .persist("SESSION", "s-1111", None, Some("ada@example.com"))
            .unwrap();
        let listing: Value = serde_json::from_str(&vault.listing().unwrap()).unwrap();
        assert!(listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["service"] == "kernel" && e["name"] == "SESSION"));
        assert_eq!(
            vault.label("SESSION", None).unwrap().as_deref(),
            Some("ada@example.com")
        );
    }

    #[test]
    fn rotating_re_encrypts_every_row_under_the_new_key_and_the_old_key_is_then_retired() {
        let mut medium = Medium::default();
        let old = medium.vault(&key("11"), None);
        old.set("stripe", "STRIPE_KEY", "sk-1234", None, None)
            .unwrap();
        old.set("openai", "OPENAI_KEY", "per-user-0001", Some("u1"), None)
            .unwrap();
        medium.apply(&old.take_writes().unwrap());

        let rotating = medium.vault(&key("22"), Some(&key("11")));
        let counts: Value = serde_json::from_str(&rotating.rotate().unwrap()).unwrap();
        assert_eq!(
            counts,
            json!({ "rotated": 2, "alreadyCurrent": 0, "unreadable": 0 })
        );
        medium.apply(&rotating.take_writes().unwrap());

        let retired = medium.vault(&key("22"), None);
        assert_eq!(
            retired.resolve("STRIPE_KEY", None).unwrap().as_deref(),
            Some("sk-1234")
        );
        assert_eq!(
            retired
                .resolve("OPENAI_KEY", Some("u1"))
                .unwrap()
                .as_deref(),
            Some("per-user-0001")
        );
        let again: Value = serde_json::from_str(&retired.rotate().unwrap()).unwrap();
        assert_eq!(
            again,
            json!({ "rotated": 0, "alreadyCurrent": 2, "unreadable": 0 })
        );
    }

    #[test]
    fn control_a_corrupt_row_fails_the_load_naming_it_and_every_read_and_write_after_it() {
        let mut medium = Medium::default();
        let vault = medium.vault(&key("11"), None);
        vault
            .set("stripe", "STRIPE_KEY", "sk-1234", None, None)
            .unwrap();
        vault
            .set("stripe", "STRIPE_HOOK", "whsec-9999", None, None)
            .unwrap();
        medium.apply(&vault.take_writes().unwrap());
        let tampered = medium.0[1]["id"].as_str().unwrap().to_string();
        medium.0[1]["authTag"] = json!("00".repeat(16));

        let broken = VaultApi::new(Some(&key("11")), None).unwrap();
        let failure = broken
            .load(&medium.json())
            .expect_err("a corrupt row is a failure, not an absence");
        assert!(failure.contains(&tampered), "{failure}");
        assert!(
            !failure.contains(medium.0[0]["id"].as_str().unwrap()),
            "{failure}"
        );
        assert!(
            broken.resolve("STRIPE_KEY", None).is_err(),
            "the good row is not read as the whole store"
        );
        assert!(broken.entries(None).is_err());
        assert!(broken.listing().is_err());
        assert!(broken.set("stripe", "NEW", "v", None, None).is_err());
        assert!(broken.remove("STRIPE_KEY", None).is_err());
        assert!(rotate_fails_naming(&broken, &tampered));
        assert!(broken.take_writes().unwrap() == "[]", "nothing was written");
    }

    /// A host's take: begin, the medium's conditional delete, then finish (or cancel when another taker won).
    fn take(vault: &VaultApi, medium: &mut Medium, name: &str, now: f64) -> Option<String> {
        let begun = vault.take_begin(name, None).unwrap()?;
        let begun: Value = serde_json::from_str(&begun).unwrap();
        assert_eq!(begun["write"], "delete");
        assert_eq!(begun["ifPresent"], true);
        let ticket = begun["ticket"].as_f64().unwrap();
        if medium.delete_if_present(begun["id"].as_str().unwrap()) {
            vault.take_finish(ticket, now).unwrap()
        } else {
            vault.take_cancel(ticket).unwrap();
            None
        }
    }

    fn rotate_fails_naming(vault: &VaultApi, id: &str) -> bool {
        vault
            .rotate()
            .expect_err("an unreadable row fails the rotation")
            .contains(id)
    }

    #[test]
    fn control_a_vault_that_never_loaded_reads_and_writes_nothing_and_a_bad_key_is_refused_without_echoing_it(
    ) {
        let vault = VaultApi::new(Some(&key("11")), None).unwrap();
        assert!(vault
            .resolve("A", None)
            .unwrap_err()
            .contains("not been loaded"));
        assert!(vault.set("s", "A", "v", None, None).is_err());
        assert!(vault.hold("A", "v", 2.0, 1.0, None).is_err());
        assert!(vault.take_begin("A", None).is_err());
        let refused = VaultApi::new(Some("not-a-key"), None)
            .err()
            .expect("a malformed key is refused");
        assert!(!refused.contains("not-a-key"), "{refused}");
        assert!(VaultApi::new(Some(&key("11")), Some("short")).is_err());
        let keyless = VaultApi::new(None, None).unwrap();
        assert!(!keyless.enabled());
        assert_eq!(keyless.load("[]").unwrap(), 0);
        assert!(keyless.set("s", "A", "v", None, None).is_err());
    }

    #[test]
    fn a_held_one_use_value_survives_a_restart_is_taken_once_and_an_expired_one_is_gone() {
        let mut medium = Medium::default();
        let vault = medium.vault(&key("11"), None);
        vault
            .hold("oauth:grant:S1", "GRANT-SECRET", 2000.0, 1000.0, None)
            .unwrap();
        medium.apply(&vault.take_writes().unwrap());
        assert!(!medium.json().contains("GRANT-SECRET"));

        let restarted = medium.vault(&key("11"), None);
        assert_eq!(
            take(&restarted, &mut medium, "oauth:grant:S1", 1500.0).as_deref(),
            Some("GRANT-SECRET")
        );
        assert!(
            restarted.take_writes().unwrap() == "[]",
            "the delete was the host's conditional one, not a journaled write"
        );
        let after = medium.vault(&key("11"), None);
        assert_eq!(
            take(&after, &mut medium, "oauth:grant:S1", 1500.0),
            None,
            "a take consumes it"
        );

        after.hold("late", "v", 1500.0, 1000.0, None).unwrap();
        medium.apply(&after.take_writes().unwrap());
        let last = medium.vault(&key("11"), None);
        assert_eq!(
            take(&last, &mut medium, "late", 1500.0),
            None,
            "an expired value is not returned"
        );
        assert!(medium.0.is_empty(), "and it is gone");
    }

    #[test]
    fn two_instances_taking_one_held_row_get_exactly_one_value_and_a_lone_instance_gets_it() {
        let mut medium = Medium::default();
        let vault = medium.vault(&key("11"), None);
        vault.hold("grant", "GRANT", 9000.0, 1000.0, None).unwrap();
        medium.apply(&vault.take_writes().unwrap());

        let (a, b) = (
            medium.vault(&key("11"), None),
            medium.vault(&key("11"), None),
        );
        let (ticket_a, ticket_b) = (
            serde_json::from_str::<Value>(&a.take_begin("grant", None).unwrap().unwrap()).unwrap(),
            serde_json::from_str::<Value>(&b.take_begin("grant", None).unwrap().unwrap()).unwrap(),
        );
        let first = medium.delete_if_present(ticket_a["id"].as_str().unwrap());
        let second = medium.delete_if_present(ticket_b["id"].as_str().unwrap());
        assert!(
            first && !second,
            "the medium's delete removes the row for one taker only"
        );
        assert_eq!(
            a.take_finish(ticket_a["ticket"].as_f64().unwrap(), 2000.0)
                .unwrap()
                .as_deref(),
            Some("GRANT")
        );
        b.take_cancel(ticket_b["ticket"].as_f64().unwrap()).unwrap();
        assert!(b
            .take_finish(ticket_b["ticket"].as_f64().unwrap(), 2000.0)
            .is_err());

        vault.hold("solo", "ONE", 9000.0, 1000.0, None).unwrap();
        medium.apply(&vault.take_writes().unwrap());
        let lone = medium.vault(&key("11"), None);
        assert_eq!(
            take(&lone, &mut medium, "solo", 2000.0).as_deref(),
            Some("ONE")
        );
        assert_eq!(a.take_begin("none", None).unwrap(), None);
    }
}
