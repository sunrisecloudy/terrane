//! The `kv` capability — a per-app key/value store. Reacts to `app.removed` by
//! dropping that app's data.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_domain::{AppId, Error, EventRecord, Result};

use super::{arg, Capability, ReadValue, ResourceMethod};
use crate::{decode_event, encode_event, Decision, State};

/// This capability's slice of State: per-app key/value maps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvState {
    pub data: BTreeMap<AppId, BTreeMap<String, String>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Set {
    app: String,
    key: String,
    value: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Deleted {
    app: String,
    key: String,
}

pub struct KvCapability;

impl Capability for KvCapability {
    fn namespace(&self) -> &'static str {
        "kv"
    }

    /// The app-scoped key/value surface backends get on `ctx.resource.kv`.
    fn resource_api(&self) -> Vec<ResourceMethod> {
        vec![
            ResourceMethod::Write {
                name: "set",
                params: &["key", "value"],
            },
            ResourceMethod::Read {
                name: "get",
                params: &["key"],
                read: read_get,
            },
            ResourceMethod::Read {
                name: "all",
                params: &[],
                read: read_all,
            },
            ResourceMethod::Write {
                name: "rm",
                params: &["key"],
            },
        ]
    }

    fn decide(&self, state: &State, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "kv.set" => {
                let app = arg(args, 0, "app")?;
                let key = arg(args, 1, "key")?;
                let value = args.get(2..).unwrap_or_default().join(" ");
                if !state.app.apps.contains_key(&app) {
                    return Err(Error::AppNotFound(app));
                }
                if key.trim().is_empty() {
                    return Err(Error::InvalidInput("key must not be empty".into()));
                }
                Ok(Decision::Commit(vec![encode_event(
                    "kv.set",
                    &Set { app, key, value },
                )?]))
            }
            "kv.rm" | "kv.delete" => {
                let app = arg(args, 0, "app")?;
                let key = arg(args, 1, "key")?;
                let missing = state
                    .kv
                    .data
                    .get(&app)
                    .map(|kv| !kv.contains_key(&key))
                    .unwrap_or(true);
                if missing {
                    return Err(Error::KeyNotFound(app, key));
                }
                Ok(Decision::Commit(vec![encode_event(
                    "kv.deleted",
                    &Deleted { app, key },
                )?]))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "kv.set" => {
                let e: Set = decode_event(record)?;
                state
                    .kv
                    .data
                    .entry(e.app)
                    .or_default()
                    .insert(e.key, e.value);
            }
            "kv.deleted" => {
                let e: Deleted = decode_event(record)?;
                if let Some(kv) = state.kv.data.get_mut(&e.app) {
                    kv.remove(&e.key);
                    if kv.is_empty() {
                        state.kv.data.remove(&e.app);
                    }
                }
            }
            // React to another capability's event: drop a removed app's data.
            "app.removed" => {
                #[derive(BorshDeserialize)]
                struct Removed {
                    id: String,
                }
                let e: Removed = decode_event(record)?;
                state.kv.data.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "kv.set" => {
                let e: Set = decode_event(record).ok()?;
                Some(format!("kv.set {}/{} = {}", e.app, e.key, e.value))
            }
            "kv.deleted" => {
                let e: Deleted = decode_event(record).ok()?;
                Some(format!("kv.deleted {}/{}", e.app, e.key))
            }
            _ => None,
        }
    }
}

/// `ctx.resource.kv.get(key)` — the value for `key` in `app`'s store, or none.
fn read_get(state: &State, app: &str, args: &[String]) -> ReadValue {
    let key = args.first().map(String::as_str).unwrap_or_default();
    ReadValue::OptString(state.kv.data.get(app).and_then(|m| m.get(key).cloned()))
}

/// `ctx.resource.kv.all()` — every key/value pair in `app`'s store.
fn read_all(state: &State, app: &str, _args: &[String]) -> ReadValue {
    ReadValue::StringMap(state.kv.data.get(app).cloned().unwrap_or_default())
}
