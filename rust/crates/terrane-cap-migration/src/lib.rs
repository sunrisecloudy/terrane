//! The `migration` capability — app data-version facts and deterministic
//! forward migration batches.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, QueryCtx, QuerySpec, QueryValue, Result, RuntimeCtx, RuntimeOutput, RuntimeRequest,
    StateStore,
};

mod doc;

pub const DEFAULT_DATA_VERSION: u64 = 1;
pub const MAX_SCRIPT_BYTES: usize = 512 * 1024;
pub const MAX_RECORDED_EVENTS_PER_STEP: usize = 10_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationState {
    pub apps: BTreeMap<String, AppMigrationState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppMigrationState {
    pub version: u64,
    pub history: Vec<MigrationStep>,
}

impl Default for AppMigrationState {
    fn default() -> Self {
        Self {
            version: DEFAULT_DATA_VERSION,
            history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MigrationStep {
    pub from_version: u64,
    pub to_version: u64,
    pub script_hash: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Applied {
    app: String,
    from_version: u64,
    to_version: u64,
    script_hash: String,
}

pub struct MigrationCapability;

impl Capability for MigrationCapability {
    fn namespace(&self) -> &'static str {
        "migration"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "migration.apply",
                },
                CommandSpec {
                    name: "migration.commit",
                },
            ],
            events: vec![EventSpec {
                kind: "migration.applied",
            }],
            queries: vec![QuerySpec {
                name: "migration.status",
            }],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::migration_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "migration.apply" => decide_apply(ctx, args),
            "migration.commit" => decide_commit(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "status" => {
                let app = arg(args, 0, "app")?;
                let status = app_status(ctx.state, &app)?;
                Ok(QueryValue::Json(
                    json!({
                        "app": app,
                        "version": status.version,
                        "history": status.history.iter().map(|step| {
                            json!({
                                "from": step.from_version,
                                "to": step.to_version,
                                "scriptHash": step.script_hash,
                            })
                        }).collect::<Vec<_>>(),
                    })
                    .to_string(),
                ))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown query: migration.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "migration.applied" => {
                let event: Applied = decode_event(record)?;
                let state = state_mut::<MigrationState>(state, "migration")?;
                let entry = state.apps.entry(event.app).or_default();
                entry.version = event.to_version;
                entry.history.push(MigrationStep {
                    from_version: event.from_version,
                    to_version: event.to_version,
                    script_hash: event.script_hash,
                });
            }
            "app.removed" => {
                let removed = decode_app_removed(record)?;
                state_mut::<MigrationState>(state, "migration")?
                    .apps
                    .remove(&removed.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        let event: Applied = decode_event(record).ok()?;
        Some(format!(
            "migration.applied {} {} -> {} ({})",
            event.app, event.from_version, event.to_version, event.script_hash
        ))
    }

    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput> {
        let from = parse_u64_arg(request.input_tail(), 0, "from")?;
        let to = parse_u64_arg(request.input_tail(), 1, "to")?;
        let script_hash = arg(request.input_tail(), 2, "script_hash")?;
        let script_source = arg(request.input_tail(), 3, "script_source")?;
        let resources = migration_resources(&ctx)?;
        let output = terrane_cap_js_runtime::run_js_migration(
            &request.app,
            &script_source,
            &resources,
            ctx.host.clone(),
        )?;
        ctx.host.write_resource(
            "migration",
            "commit",
            &[
                from.to_string(),
                to.to_string(),
                script_hash,
            ],
        )?;
        let count = ctx.host.record_count();
        if count > MAX_RECORDED_EVENTS_PER_STEP {
            return Err(Error::InvalidInput(format!(
                "migration step recorded {count} events; limit is {MAX_RECORDED_EVENTS_PER_STEP}"
            )));
        }
        Ok(RuntimeOutput { output })
    }
}

fn migration_resources(ctx: &RuntimeCtx) -> Result<Vec<String>> {
    match &ctx.source_files {
        Some(files) => Ok(terrane_cap_js_runtime::read_manifest_from_files(files)?.resources),
        None => Ok(terrane_cap_js_runtime::read_manifest(std::path::Path::new(&ctx.source))?
            .resources),
    }
}

fn decide_apply(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let to = parse_u64_arg(args, 1, "to_version")?;
    let script_source = arg(args, 2, "script_source")?;
    ensure_app_exists(ctx.bus, &app)?;
    if script_source.len() > MAX_SCRIPT_BYTES {
        return Err(Error::InvalidInput(format!(
            "migration script exceeds {MAX_SCRIPT_BYTES} bytes"
        )));
    }
    let from = version(ctx.state, &app)?;
    validate_next_step(from, to)?;
    let script_hash = sha256_hex(script_source.as_bytes());
    Ok(Decision::Runtime(RuntimeRequest {
        app,
        input: vec![from.to_string(), to.to_string(), script_hash, script_source],
    }))
}

fn decide_commit(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let from = parse_u64_arg(args, 1, "from")?;
    let to = parse_u64_arg(args, 2, "to")?;
    let script_hash = arg(args, 3, "script_hash")?;
    ensure_app_exists(ctx.bus, &app)?;
    if version(ctx.state, &app)? != from {
        return Err(Error::InvalidInput(format!(
            "migration.commit expected current version {from} for {app}"
        )));
    }
    validate_next_step(from, to)?;
    validate_sha256_hex(&script_hash)?;
    Ok(Decision::Commit(vec![applied_event(
        &app,
        from,
        to,
        &script_hash,
    )?]))
}

pub fn applied_event(
    app: &str,
    from_version: u64,
    to_version: u64,
    script_hash: &str,
) -> Result<EventRecord> {
    encode_event(
        "migration.applied",
        &Applied {
            app: app.to_string(),
            from_version,
            to_version,
            script_hash: script_hash.to_string(),
        },
    )
}

pub fn version(state: &dyn StateStore, app: &str) -> Result<u64> {
    Ok(app_status(state, app)?.version)
}

pub fn app_status(state: &dyn StateStore, app: &str) -> Result<AppMigrationState> {
    Ok(state_ref::<MigrationState>(state, "migration")?
        .apps
        .get(app)
        .cloned()
        .unwrap_or_default())
}

pub fn validate_next_step(from: u64, to: u64) -> Result<()> {
    if to != from + 1 {
        return Err(Error::InvalidInput(format!(
            "migration steps must be consecutive: current version is {from}, target is {to}"
        )));
    }
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(hex(byte >> 4));
        out.push(hex(byte & 0x0f));
    }
    out
}

fn validate_sha256_hex(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::InvalidInput(
            "script_hash must be 64 hex characters".into(),
        ))
    }
}

fn parse_u64_arg(args: &[String], index: usize, name: &str) -> Result<u64> {
    let value = arg(args, index, name)?;
    value.parse::<u64>().map_err(|_| {
        Error::InvalidInput(format!(
            "{name} must be a non-negative integer, got {value:?}"
        ))
    })
}

fn hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}
