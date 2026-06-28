//! The `kv` capability — a per-app key/value store. Reacts to `app.removed` by
//! dropping that app's data.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_api::Capability;
use terrane_cap_api::{
    arg, decode_event, encode_event, ensure_app_exists, state_mut, state_ref, AppId, CapManifest,
    CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord, EventSpec, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

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
    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "kv.set" },
                CommandSpec { name: "kv.rm" },
                CommandSpec { name: "kv.delete" },
            ],
            events: vec![
                EventSpec { kind: "kv.set" },
                EventSpec { kind: "kv.deleted" },
            ],
            queries: Vec::new(),
            resources: resource_methods(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "kv.set" => {
                let app = arg(args, 0, "app")?;
                let key = arg(args, 1, "key")?;
                let value = args.get(2..).unwrap_or_default().join(" ");
                ensure_app_exists(ctx.bus, &app)?;
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
                let missing = state_ref::<KvState>(ctx.state, "kv")?
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

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "kv.set" => {
                let e: Set = decode_event(record)?;
                state_mut::<KvState>(state, "kv")?
                    .data
                    .entry(e.app)
                    .or_default()
                    .insert(e.key, e.value);
            }
            "kv.deleted" => {
                let e: Deleted = decode_event(record)?;
                let state = state_mut::<KvState>(state, "kv")?;
                if let Some(kv) = state.data.get_mut(&e.app) {
                    kv.remove(&e.key);
                    if kv.is_empty() {
                        state.data.remove(&e.app);
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
                state_mut::<KvState>(state, "kv")?.data.remove(&e.id);
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

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "get" => read_get(ctx.state, ctx.app, args),
            "all" => read_all(ctx.state, ctx.app, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: kv.{other}"
            ))),
        }
    }
}

fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "set",
            params: &["key", "value"],
        },
        ResourceMethod::Read {
            name: "get",
            params: &["key"],
        },
        ResourceMethod::Read {
            name: "all",
            params: &[],
        },
        ResourceMethod::Write {
            name: "rm",
            params: &["key"],
        },
    ]
}

/// `ctx.resource.kv.get(key)` — the value for `key` in `app`'s store, or none.
fn read_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let key = args.first().map(String::as_str).unwrap_or_default();
    Ok(ReadValue::OptString(
        state_ref::<KvState>(state, "kv")?
            .data
            .get(app)
            .and_then(|m| m.get(key).cloned()),
    ))
}

/// `ctx.resource.kv.all()` — every key/value pair in `app`'s store.
fn read_all(state: &dyn StateStore, app: &str, _args: &[String]) -> Result<ReadValue> {
    Ok(ReadValue::StringMap(
        state_ref::<KvState>(state, "kv")?
            .data
            .get(app)
            .cloned()
            .unwrap_or_default(),
    ))
}
