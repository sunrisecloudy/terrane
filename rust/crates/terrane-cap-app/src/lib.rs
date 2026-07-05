//! The `app` capability — the catalog of saved apps.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_event, encode_event, restore_state, snapshot_state, state_mut, state_ref, AppId,
    CapManifest, CommandCtx, CommandSpec, Decision, Effect, Error, EventRecord, EventSpec,
    QueryCtx, QuerySpec, QueryValue, Result, StateStore,
};
use terrane_cap_kv::RESERVED_PREFIX;

mod doc;

pub const MAX_LINK_PAYLOAD_BYTES: usize = 64 * 1024;

/// A saved app, as the user sees it in their catalog. `source` is where the
/// app's body lives — a path to its bundle (UI + backend).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AppRecord {
    pub id: AppId,
    pub name: String,
    pub source: Option<String>,
    pub runtime: String,
    pub version: String,
    pub history: Vec<VersionEntry>,
    pub interfaces: Vec<String>,
    pub links: Vec<LinkRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct VersionEntry {
    pub version: String,
    pub bundle_hash: String,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct LinkRegistration {
    pub kind: String,
    pub spec: String,
}

/// This capability's slice of State.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AppState {
    pub apps: BTreeMap<AppId, AppRecord>,
    pub links: BTreeMap<AppId, Vec<LinkRegistration>>,
}

type ParsedAdd = (String, String, Option<String>, String, Vec<String>, Vec<LinkRegistration>);

#[derive(BorshSerialize, BorshDeserialize)]
struct Added {
    id: String,
    name: String,
    source: Option<String>,
    runtime: String,
    interfaces: Vec<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct AddedV1 {
    id: String,
    name: String,
    source: Option<String>,
    runtime: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Removed {
    id: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Upgraded {
    id: String,
    from_version: String,
    to_version: String,
    bundle_hash: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct LinkRegistered {
    app: String,
    kind: String,
    spec: String,
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
                CommandSpec {
                    name: "app.upgrade",
                },
                CommandSpec {
                    name: "app.link.deliver",
                },
                CommandSpec { name: "app.remove" },
            ],
            events: vec![
                EventSpec { kind: "app.added" },
                EventSpec {
                    kind: "app.upgraded",
                },
                EventSpec {
                    kind: "app.link.registered",
                },
                EventSpec {
                    kind: "app.removed",
                },
            ],
            queries: vec![QuerySpec { name: "app.exists" }],
            resources: Vec::new(),
            grant_resources: Vec::new(),
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
                let (id, app_name, source, runtime, interfaces, links) = parse_add(args)?;
                if id.trim().is_empty() {
                    return Err(Error::InvalidInput("app id must not be empty".into()));
                }
                validate_app_id(&id)?;
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
                let mut events = vec![encode_event(
                    "app.added",
                    &Added {
                        id: id.clone(),
                        name: app_name,
                        source,
                        runtime,
                        interfaces,
                    },
                )?];
                for link in default_scheme_links(&id)
                    .into_iter()
                    .chain(links)
                {
                    validate_link_registration(&link.kind, &link.spec)?;
                    events.push(link_registered_event(&id, &link.kind, &link.spec)?);
                }
                Ok(Decision::Commit(events))
            }
            "app.upgrade" => {
                let (id, source) = parse_upgrade(args)?;
                if !state_ref::<AppState>(ctx.state, "app")?
                    .apps
                    .contains_key(&id)
                {
                    return Err(Error::AppNotFound(id));
                }
                Ok(Decision::Effect(Effect::UpgradeAppBundle { id, source }))
            }
            "app.link.deliver" => {
                let target = arg(args, 0, "target app")?;
                let kind = arg(args, 1, "common.receive kind")?;
                let payload = arg(args, 2, "payload JSON")?;
                if kind != "link" && kind != "blob" {
                    return Err(Error::InvalidInput(format!(
                        "app.link.deliver only supports link or blob payloads, got {kind}"
                    )));
                }
                if payload.len() > MAX_LINK_PAYLOAD_BYTES {
                    return Err(Error::InvalidInput(format!(
                        "app.link.deliver payload exceeds {MAX_LINK_PAYLOAD_BYTES} bytes"
                    )));
                }
                if !state_ref::<AppState>(ctx.state, "app")?
                    .apps
                    .contains_key(&target)
                {
                    return Err(Error::AppNotFound(target));
                }
                Ok(Decision::Effect(Effect::AppCall {
                    chain: vec!["terrane-host".to_string()],
                    target,
                    verb: "common.receive".to_string(),
                    args: vec![kind, payload],
                }))
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
                let e = decode_added(record)?;
                state_mut::<AppState>(state, "app")?.apps.insert(
                    e.id.clone(),
                    AppRecord {
                        id: e.id,
                        name: e.name,
                        source: e.source,
                        runtime: e.runtime,
                        version: DEFAULT_VERSION.to_string(),
                        history: Vec::new(),
                        interfaces: normalize_interfaces(e.interfaces),
                        links: Vec::new(),
                    },
                );
            }
            "app.upgraded" => {
                let e: Upgraded = decode_event(record)?;
                let state = state_mut::<AppState>(state, "app")?;
                let app = state
                    .apps
                    .get_mut(&e.id)
                    .ok_or_else(|| Error::AppNotFound(e.id.clone()))?;
                app.version = e.to_version.clone();
                app.history.push(VersionEntry {
                    version: e.to_version,
                    bundle_hash: e.bundle_hash,
                    seq: app.history.len() as u64 + 1,
                });
                if app.history.len() > MAX_VERSION_HISTORY {
                    let excess = app.history.len() - MAX_VERSION_HISTORY;
                    app.history.drain(0..excess);
                }
            }
            "app.link.registered" => {
                let e: LinkRegistered = decode_event(record)?;
                let link = LinkRegistration {
                    kind: e.kind,
                    spec: e.spec,
                };
                let state = state_mut::<AppState>(state, "app")?;
                let links = state.links.entry(e.app.clone()).or_default();
                if !links.contains(&link) {
                    links.push(link.clone());
                }
                if let Some(app) = state.apps.get_mut(&e.app) {
                    if !app.links.contains(&link) {
                        app.links.push(link);
                    }
                }
            }
            "app.removed" => {
                let e: Removed = decode_event(record)?;
                let state = state_mut::<AppState>(state, "app")?;
                state.apps.remove(&e.id);
                state.links.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<AppState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<AppState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "app.added" => {
                let e = decode_added(record).ok()?;
                Some(match e.source {
                    Some(src) => format!(
                        "app.added {} \"{}\" runtime={} [{}]",
                        e.id, e.name, e.runtime, src
                    ),
                    None => format!("app.added {} \"{}\" runtime={}", e.id, e.name, e.runtime),
                })
            }
            "app.upgraded" => {
                let e: Upgraded = decode_event(record).ok()?;
                Some(format!(
                    "app.upgraded {} {} -> {} ({})",
                    e.id, e.from_version, e.to_version, e.bundle_hash
                ))
            }
            "app.link.registered" => {
                let e: LinkRegistered = decode_event(record).ok()?;
                Some(format!("app.link.registered {} {} {}", e.app, e.kind, e.spec))
            }
            "app.removed" => {
                let e: Removed = decode_event(record).ok()?;
                Some(format!("app.removed {}", e.id))
            }
            _ => None,
        }
    }

    fn app_of(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "app.added" => decode_added(record).ok().map(|e| e.id),
            "app.upgraded" => decode_event::<Upgraded>(record).ok().map(|e| e.id),
            "app.link.registered" => decode_event::<LinkRegistered>(record).ok().map(|e| e.app),
            "app.removed" => decode_event::<Removed>(record).ok().map(|e| e.id),
            _ => None,
        }
    }
}

pub const DEFAULT_VERSION: &str = "0.0.0";
pub const MAX_VERSION_LEN: usize = 64;
pub const MAX_VERSION_HISTORY: usize = 100;

pub fn added_event(
    id: impl Into<String>,
    name: impl Into<String>,
    source: Option<String>,
    runtime: impl Into<String>,
) -> Result<EventRecord> {
    added_event_with_interfaces(id, name, source, runtime, mandatory_interfaces())
}

pub fn upgraded_event(
    id: impl Into<String>,
    from_version: impl Into<String>,
    to_version: impl Into<String>,
    bundle_hash: impl Into<String>,
) -> Result<EventRecord> {
    let id = id.into();
    let from_version = from_version.into();
    let to_version = to_version.into();
    let bundle_hash = bundle_hash.into();
    validate_version(&from_version)?;
    validate_version(&to_version)?;
    encode_event(
        "app.upgraded",
        &Upgraded {
            id,
            from_version,
            to_version,
            bundle_hash,
        },
    )
}

pub fn added_event_with_interfaces(
    id: impl Into<String>,
    name: impl Into<String>,
    source: Option<String>,
    runtime: impl Into<String>,
    interfaces: Vec<String>,
) -> Result<EventRecord> {
    encode_event(
        "app.added",
        &Added {
            id: id.into(),
            name: name.into(),
            source,
            runtime: runtime.into(),
            interfaces,
        },
    )
}

pub fn link_registered_event(app: &str, kind: &str, spec: &str) -> Result<EventRecord> {
    validate_link_registration(kind, spec)?;
    encode_event(
        "app.link.registered",
        &LinkRegistered {
            app: app.to_string(),
            kind: kind.to_string(),
            spec: spec.to_string(),
        },
    )
}

/// Parse `add` args: `<id> <name…> [--source <path>] [--runtime <name>]`.
fn parse_add(args: &[String]) -> Result<ParsedAdd> {
    let id = arg(args, 0, "app id")?;
    let mut name_parts: Vec<&str> = Vec::new();
    let mut source = None;
    let mut runtime = "js".to_string();
    let mut interfaces = Vec::new();
    let mut links = Vec::new();
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
            "--interfaces" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--interfaces` needs a comma-separated list".into()))?;
                interfaces = value
                    .split(',')
                    .map(str::trim)
                    .filter(|iface| !iface.is_empty())
                    .map(str::to_string)
                    .collect();
                i += 2;
            }
            "--file-types" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--file-types` needs ext:mime entries".into()))?;
                for spec in value.split(',').map(str::trim).filter(|spec| !spec.is_empty()) {
                    links.push(LinkRegistration {
                        kind: "filetype".to_string(),
                        spec: spec.to_string(),
                    });
                }
                i += 2;
            }
            "--link" => {
                let kind = args
                    .get(i + 1)
                    .ok_or_else(|| Error::InvalidInput("`--link` needs a kind".into()))?;
                let spec = args
                    .get(i + 2)
                    .ok_or_else(|| Error::InvalidInput("`--link` needs a spec".into()))?;
                links.push(LinkRegistration {
                    kind: kind.clone(),
                    spec: spec.clone(),
                });
                i += 3;
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
    Ok((
        id,
        name_parts.join(" "),
        source,
        runtime,
        normalize_interfaces(interfaces),
        links,
    ))
}

fn decode_added(record: &EventRecord) -> Result<Added> {
    match decode_event::<Added>(record) {
        Ok(event) => Ok(event),
        Err(_) => {
            let old: AddedV1 = decode_event(record)?;
            Ok(Added {
                id: old.id,
                name: old.name,
                source: old.source,
                runtime: old.runtime,
                interfaces: mandatory_interfaces(),
            })
        }
    }
}

pub fn normalize_interfaces(mut interfaces: Vec<String>) -> Vec<String> {
    interfaces.push("inbox".to_string());
    interfaces.push("items".to_string());
    interfaces.sort();
    interfaces.dedup();
    interfaces
}

pub fn mandatory_interfaces() -> Vec<String> {
    normalize_interfaces(Vec::new())
}

pub fn default_scheme_links(app: &str) -> Vec<LinkRegistration> {
    vec![
        LinkRegistration {
            kind: "scheme-route".to_string(),
            spec: format!("terrane://open/{app}"),
        },
        LinkRegistration {
            kind: "scheme-route".to_string(),
            spec: format!("terrane://send/{app}"),
        },
        LinkRegistration {
            kind: "scheme-route".to_string(),
            spec: format!("terrane://app/{app}/item/*"),
        },
    ]
}

pub fn validate_link_registration(kind: &str, spec: &str) -> Result<()> {
    match kind {
        "scheme-route" => validate_scheme_route(spec),
        "filetype" => validate_filetype(spec),
        other => Err(Error::InvalidInput(format!(
            "unsupported app link registration kind: {other}"
        ))),
    }
}

fn validate_scheme_route(spec: &str) -> Result<()> {
    if spec.starts_with("terrane://open/")
        || spec.starts_with("terrane://send/")
        || spec.starts_with("terrane://app/")
    {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "scheme-route spec must be a terrane:// route: {spec}"
        )))
    }
}

fn validate_filetype(spec: &str) -> Result<()> {
    let Some((ext, mime)) = spec.split_once(':') else {
        return Err(Error::InvalidInput(format!(
            "filetype spec must be ext:mime, got {spec}"
        )));
    };
    if ext.is_empty()
        || ext.starts_with('.')
        || !ext
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "filetype extension is unsafe: {ext:?}"
        )));
    }
    if mime.is_empty()
        || !mime.contains('/')
        || !mime
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'+' | b'-'))
    {
        return Err(Error::InvalidInput(format!(
            "filetype mime is unsafe: {mime:?}"
        )));
    }
    Ok(())
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

fn parse_upgrade(args: &[String]) -> Result<(String, String)> {
    let id = arg(args, 0, "app id")?;
    validate_app_id(&id)?;
    if args.len() < 2 {
        return Err(Error::InvalidInput(
            "usage: app upgrade <id> <bundle path|--to-version version|--from-draft draftId>"
                .into(),
        ));
    }
    match args[1].as_str() {
        "--to-version" => {
            let version = args
                .get(2)
                .ok_or_else(|| Error::InvalidInput("`--to-version` needs a version".into()))?;
            validate_version(version)?;
            Ok((id, format!("version://{version}")))
        }
        "--from-draft" => {
            let draft = args
                .get(2)
                .ok_or_else(|| Error::InvalidInput("`--from-draft` needs a draft id".into()))?;
            if draft.trim().is_empty() || draft.contains('/') || draft.contains("..") {
                return Err(Error::InvalidInput(format!("unsafe draft id: {draft:?}")));
            }
            Ok((id, format!("draft://{draft}")))
        }
        source => {
            if args.len() != 2 {
                return Err(Error::InvalidInput(format!(
                    "unknown app.upgrade option: {}",
                    args[2]
                )));
            }
            Ok((id, source.to_string()))
        }
    }
}

fn validate_app_id(id: &str) -> Result<()> {
    if id.starts_with(RESERVED_PREFIX) {
        return Err(Error::InvalidInput(format!(
            "app id prefix {RESERVED_PREFIX:?} is reserved for platform data"
        )));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "app id is unsafe: {id:?}; use ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(())
}

pub fn validate_version(version: &str) -> Result<()> {
    if version.is_empty() || version.len() > MAX_VERSION_LEN {
        return Err(Error::InvalidInput(format!(
            "app version must be 1..={MAX_VERSION_LEN} bytes"
        )));
    }
    let (core, prerelease) = match version.split_once('-') {
        Some((core, pre)) => {
            if pre.is_empty()
                || !pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
            {
                return Err(Error::InvalidInput(format!(
                    "app version prerelease is invalid: {version:?}"
                )));
            }
            (core, Some(pre))
        }
        None => (version, None),
    };
    if core.split('.').count() != 3 {
        return Err(Error::InvalidInput(format!(
            "app version must be semver X.Y.Z with optional prerelease, got {version:?}"
        )));
    }
    for part in core.split('.') {
        if part.is_empty()
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(Error::InvalidInput(format!(
                "app version must be semver X.Y.Z with optional prerelease, got {version:?}"
            )));
        }
    }
    let _ = prerelease;
    Ok(())
}

#[cfg(test)]
mod tests;
