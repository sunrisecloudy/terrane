//! The `app` capability — the catalog of saved apps.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_event, encode_event, state_mut, state_ref, AppId, CapManifest, CommandCtx,
    CommandSpec, Decision, Effect, Error, EventRecord, EventSpec, QueryCtx, QuerySpec, QueryValue,
    Result, StateStore,
};

mod doc;

/// A saved app, as the user sees it in their catalog. `source` is where the
/// app's body lives — a path to its bundle (UI + backend).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRecord {
    pub id: AppId,
    pub name: String,
    pub source: Option<String>,
    pub runtime: String,
}

/// This capability's slice of State.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    pub apps: BTreeMap<AppId, AppRecord>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Added {
    id: String,
    name: String,
    source: Option<String>,
    runtime: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Removed {
    id: String,
}

pub struct AppCapability;

impl Capability for AppCapability {
    fn namespace(&self) -> &'static str {
        "app"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "app.add" },
                CommandSpec { name: "app.import" },
                CommandSpec { name: "app.remove" },
            ],
            events: vec![
                EventSpec { kind: "app.added" },
                EventSpec {
                    kind: "app.removed",
                },
            ],
            queries: vec![QuerySpec { name: "app.exists" }],
            resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::app_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "app.import" => {
                let (source, storage_backend, storage_path) = parse_import(args)?;
                Ok(Decision::Effect(Effect::ImportAppBundle {
                    source,
                    storage_backend,
                    storage_path,
                }))
            }
            "app.add" => {
                let (id, app_name, source, runtime) = parse_add(args)?;
                if id.trim().is_empty() {
                    return Err(Error::InvalidInput("app id must not be empty".into()));
                }
                if app_name.trim().is_empty() {
                    return Err(Error::InvalidInput("app name must not be empty".into()));
                }
                if runtime.trim().is_empty() {
                    return Err(Error::InvalidInput("app runtime must not be empty".into()));
                }
                if state_ref::<AppState>(ctx.state, "app")?
                    .apps
                    .contains_key(&id)
                {
                    return Err(Error::AppExists(id));
                }
                Ok(Decision::Commit(vec![encode_event(
                    "app.added",
                    &Added {
                        id,
                        name: app_name,
                        source,
                        runtime,
                    },
                )?]))
            }
            "app.remove" => {
                let id = arg(args, 0, "app id")?;
                if !state_ref::<AppState>(ctx.state, "app")?
                    .apps
                    .contains_key(&id)
                {
                    return Err(Error::AppNotFound(id));
                }
                Ok(Decision::Commit(vec![encode_event(
                    "app.removed",
                    &Removed { id },
                )?]))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "exists" => {
                let app = arg(args, 0, "app")?;
                Ok(QueryValue::Bool(
                    state_ref::<AppState>(ctx.state, "app")?
                        .apps
                        .contains_key(&app),
                ))
            }
            other => Err(Error::InvalidInput(format!("unknown query: app.{other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "app.added" => {
                let e: Added = decode_event(record)?;
                state_mut::<AppState>(state, "app")?.apps.insert(
                    e.id.clone(),
                    AppRecord {
                        id: e.id,
                        name: e.name,
                        source: e.source,
                        runtime: e.runtime,
                    },
                );
            }
            "app.removed" => {
                let e: Removed = decode_event(record)?;
                state_mut::<AppState>(state, "app")?.apps.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "app.added" => {
                let e: Added = decode_event(record).ok()?;
                Some(match e.source {
                    Some(src) => format!(
                        "app.added {} \"{}\" runtime={} [{}]",
                        e.id, e.name, e.runtime, src
                    ),
                    None => format!("app.added {} \"{}\" runtime={}", e.id, e.name, e.runtime),
                })
            }
            "app.removed" => {
                let e: Removed = decode_event(record).ok()?;
                Some(format!("app.removed {}", e.id))
            }
            _ => None,
        }
    }
}

pub fn added_event(
    id: impl Into<String>,
    name: impl Into<String>,
    source: Option<String>,
    runtime: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "app.added",
        &Added {
            id: id.into(),
            name: name.into(),
            source,
            runtime: runtime.into(),
        },
    )
}

/// Parse `add` args: `<id> <name…> [--source <path>] [--runtime <name>]`.
fn parse_add(args: &[String]) -> Result<(String, String, Option<String>, String)> {
    let id = arg(args, 0, "app id")?;
    let mut name_parts: Vec<&str> = Vec::new();
    let mut source = None;
    let mut runtime = "js".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                let path = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--source` needs a path".into()))?;
                source = Some(path.clone());
                i += 2;
            }
            "--runtime" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--runtime` needs a name".into()))?;
                runtime = value.clone();
                i += 2;
            }
            word => {
                name_parts.push(word);
                i += 1;
            }
        }
    }
    if name_parts.is_empty() {
        return Err(Error::InvalidInput(
            "usage: app add <id> <name…> [--source <path>] [--runtime <name>]".into(),
        ));
    }
    Ok((id, name_parts.join(" "), source, runtime))
}

/// Parse `import` args: `<bundle> [--storage <backend>] [--path <path>]`.
fn parse_import(args: &[String]) -> Result<(String, Option<String>, Option<String>)> {
    let source = arg(args, 0, "bundle path")?;
    let mut storage_backend = None;
    let mut storage_path = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--storage" | "--backend" => {
                let backend = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--storage` needs a backend".into()))?;
                storage_backend = Some(backend.clone());
                i += 2;
            }
            "--path" | "--storage-path" => {
                let path = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--path` needs a path".into()))?;
                storage_path = Some(path.clone());
                i += 2;
            }
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown app.import option: {other}"
                )))
            }
        }
    }
    Ok((source, storage_backend, storage_path))
}

#[cfg(test)]
mod tests;
