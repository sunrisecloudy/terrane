//! forge-cli: the M0a spine harness library.
//!
//! prd-merged/06 PS-5 (the CLI harness shell) + prd-merged/09 M0a exit (the
//! executable spine + its acceptance proof). The `forge` binary is a thin arg
//! parser over this library; the heavy lifting (drive the whole jewel end to
//! end, then assert deterministic replay) lives here so integration tests can
//! call the same code path the binary does.
//!
//! Phase 3 extends the library with catalog-driven `commands` / `describe` /
//! `run` / `trace` helpers over [`WorkspaceCore`] (cli-plan/07).

use forge_core::WorkspaceCore;
use forge_domain::{
    catalog::CommandSurface, ActorContext, CoreCommand, CoreError, CoreEvent, CoreResponse,
    RequestId, Result, Role, WorkspaceId,
};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

/// The applet id the demo installs under.
const DEMO_APPLET_ID: &str = "notes-lite";

/// Default workspace id for generic CLI invocations (distinct from the demo spine).
pub const DEFAULT_WORKSPACE_ID: &str = "ws-cli";

/// Environment variable for bearer token against `--server`.
pub const FORGE_SERVER_TOKEN_ENV: &str = "FORGE_SERVER_TOKEN";

/// Environment variable overriding the default on-disk CLI workspace path.
pub const FORGE_WORKSPACE_ENV: &str = "FORGE_WORKSPACE";

/// The notes-lite demo source + manifest, embedded so `forge demo` needs no
/// filesystem layout at runtime (the binary is self-contained). Kept in lockstep
/// with `examples/notes-lite/` — the e2e test loads those files from disk and
/// asserts they match these embeds, so they cannot drift silently.
pub const NOTES_LITE_MAIN_TS: &str =
    include_str!("../../../examples/notes-lite/src/main.ts");
pub const NOTES_LITE_MANIFEST_JSON: &str =
    include_str!("../../../examples/notes-lite/manifest.json");

/// Options for opening a local [`WorkspaceCore`].
#[derive(Debug, Clone)]
pub struct WorkspaceOpenOptions {
    pub workspace_id: String,
    pub path: Option<PathBuf>,
    pub in_memory: bool,
}

impl Default for WorkspaceOpenOptions {
    fn default() -> Self {
        WorkspaceOpenOptions {
            workspace_id: DEFAULT_WORKSPACE_ID.to_string(),
            path: None,
            // File-backed by default so successive CLI invocations (`forge run`,
            // `forge trace`, …) share one workspace. Tests opt into `--in-memory`.
            in_memory: false,
        }
    }
}

impl WorkspaceOpenOptions {
    /// Ephemeral in-memory workspace (integration tests and `--in-memory`).
    pub fn in_memory() -> Self {
        WorkspaceOpenOptions {
            in_memory: true,
            ..Self::default()
        }
    }
}

/// Default on-disk workspace path for CLI invocations without `--workspace`.
pub fn default_cli_workspace_path() -> PathBuf {
    std::env::var(FORGE_WORKSPACE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("terrane-forge-cli"))
}

/// Filters forwarded to `system.describe`.
#[derive(Debug, Clone, Default)]
pub struct DescribeFilter {
    pub tier: Option<String>,
    pub namespace: Option<String>,
    pub names: Option<Vec<String>>,
    pub include_inner: bool,
    pub for_role: Option<String>,
}

/// Transport + identity options for `run`.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub workspace: WorkspaceOpenOptions,
    pub actor_id: Option<String>,
    pub role: Option<Role>,
    pub applet_id: Option<String>,
    pub server_url: Option<String>,
    pub token: Option<String>,
    pub dry_run: bool,
    pub emit_events: bool,
}

/// Outcome of a `run` invocation (local or remote).
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub response: CoreResponse,
    pub envelope: CoreCommand,
    pub events: Vec<CoreEvent>,
}

/// The outcome of driving the spine once: enough of the run to print a report
/// and to assert the M0a exit conditions in a test.
#[derive(Debug, Clone)]
pub struct DemoOutcome {
    /// Whether the run's `main` returned `{ ok: true }`.
    pub run_ok: bool,
    /// The `AppResult` the applet returned (the `{ ok, value }` object).
    pub result: serde_json::Value,
    /// The id of the recorded run (replay source).
    pub run_id: String,
    /// The replay fingerprint of the recorded run (its observable digest).
    pub fingerprint: String,
    /// The UI trees the run rendered, in order (canonical catalog JSON).
    pub ui_trees: Vec<serde_json::Value>,
    /// The records stored in the `notes` collection after the run.
    pub notes: Vec<serde_json::Value>,
    /// Whether replay reproduced the run byte-identically (the jewel's last link).
    pub replay_identical: bool,
}

/// Drive the whole M0a spine once against a fresh in-memory workspace: install
/// notes-lite, run it with `input`, capture the rendered UI + stored records +
/// recorded run, then replay and check byte-identity.
///
/// This is the single code path `forge demo` and the e2e acceptance test share,
/// so the test proves exactly what the binary does (prd-merged/09 M0a exit).
pub fn run_demo(input: serde_json::Value) -> Result<DemoOutcome> {
    let mut core = WorkspaceCore::in_memory("ws-demo")?;

    install(&mut core, DEMO_APPLET_ID, NOTES_LITE_MANIFEST_JSON, NOTES_LITE_MAIN_TS)?;

    // ---- runtime.run: the TS → ... → SQLite write → UI patch links ----------
    let run_resp = handle(
        &mut core,
        Some(DEMO_APPLET_ID),
        "runtime.run",
        serde_json::json!({ "input": input }),
    )?;

    let run_id = run_resp
        .get("run_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::RuntimeError("runtime.run returned no run_id".into()))?
        .to_string();
    let run_ok = run_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let result = run_resp.get("result").cloned().unwrap_or(serde_json::Value::Null);
    let ui_trees: Vec<serde_json::Value> = run_resp
        .get("ui_renders")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // ---- the SQLite read-back: the stored notes records ---------------------
    let notes = list_records(&mut core, "notes")?;

    // ---- runtime.replay: the deterministic-replay link ----------------------
    let replay_resp = handle(
        &mut core,
        Some(DEMO_APPLET_ID),
        "runtime.replay",
        serde_json::json!({ "run_id": run_id }),
    )?;
    let replay_identical = replay_resp
        .get("replays_identically")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let fingerprint = replay_resp
        .get("fingerprint")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(DemoOutcome {
        run_ok,
        result,
        run_id,
        fingerprint,
        ui_trees,
        notes,
        replay_identical,
    })
}

/// Run the demo and print a human report to `out`, returning the outcome so the
/// caller can pick an exit code. The `forge demo` subcommand is this plus an
/// exit-code mapping.
pub fn demo(out: &mut dyn std::io::Write) -> Result<DemoOutcome> {
    let input = serde_json::json!({ "title": "Buy milk" });
    let outcome = run_demo(input)?;

    let _ = writeln!(out, "forge demo — M0a executable spine (prd-merged/09 M0a exit)");
    let _ = writeln!(out, "applet: {DEMO_APPLET_ID}");
    let _ = writeln!(out);

    let _ = writeln!(out, "── emitted UI tree(s) ──");
    for (i, tree) in outcome.ui_trees.iter().enumerate() {
        let pretty = serde_json::to_string_pretty(tree)
            .unwrap_or_else(|_| tree.to_string());
        let _ = writeln!(out, "render[{i}]:\n{pretty}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "── stored `notes` records ──");
    for note in &outcome.notes {
        let _ = writeln!(out, "{note}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "── run ──");
    let _ = writeln!(out, "run_id:      {}", outcome.run_id);
    let _ = writeln!(out, "result:      {}", outcome.result);
    let _ = writeln!(out, "fingerprint: {}", outcome.fingerprint);
    let _ = writeln!(out);

    let _ = writeln!(out, "REPLAY IDENTICAL: {}", outcome.replay_identical);

    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Phase 3 — catalog-driven CLI helpers (cli-plan/07)
// ---------------------------------------------------------------------------

/// Root of the JSON Schema tree (`schemas/` at the repo root).
pub fn schema_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schemas")
}

/// Parse a role name from CLI flags.
pub fn parse_role(value: &str) -> Result<Role> {
    match value {
        "owner" => Ok(Role::Owner),
        "maintainer" => Ok(Role::Maintainer),
        "editor" => Ok(Role::Editor),
        "runner" => Ok(Role::Runner),
        "viewer" => Ok(Role::Viewer),
        "auditor" => Ok(Role::Auditor),
        "reviewer" => Ok(Role::Reviewer),
        other => Err(CoreError::ValidationError(format!(
            "unknown role {other:?} (expected owner|maintainer|editor|runner|viewer|auditor|reviewer)"
        ))),
    }
}

/// Build an [`ActorContext`] for local core use (`--actor` / `--role` overrides).
pub fn actor_context(actor_id: Option<&str>, role: Option<Role>) -> ActorContext {
    ActorContext {
        actor: actor_id.unwrap_or("cli").into(),
        role: role.unwrap_or(Role::Owner),
    }
}

/// Open a local [`WorkspaceCore`].
///
/// CLI invocations use a persistent file-backed workspace by default
/// ([`default_cli_workspace_path`] or `FORGE_WORKSPACE`) so commands like
/// `forge run` and `forge trace` share state across process boundaries. Pass
/// `--in-memory` (or [`WorkspaceOpenOptions::in_memory`]) for an isolated
/// ephemeral workspace in tests.
pub fn open_core(opts: &WorkspaceOpenOptions) -> Result<WorkspaceCore> {
    let mut core = if opts.in_memory {
        WorkspaceCore::in_memory(&opts.workspace_id)?
    } else {
        let path = opts
            .path
            .clone()
            .unwrap_or_else(default_cli_workspace_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::ValidationError(format!(
                    "create workspace parent {}: {e}",
                    parent.display()
                ))
            })?;
        }
        WorkspaceCore::open(&path, &opts.workspace_id)?
    };
    wire_cli_host_seams(&mut core);
    Ok(core)
}

/// Dev-harness defaults: canned HTTP + a sandbox filesystem with `workspace_data`.
fn wire_cli_host_seams(core: &mut WorkspaceCore) {
    core.set_http_client_factory(|| Box::new(forge_runtime::MockHttpClient::canned()));
    core.set_file_system_factory(|| {
        Box::new(
            forge_runtime::InMemoryFileSystem::new()
                .with_handle_root("workspace_data", "workspace-root"),
        )
    });
}

/// Call `system.describe` and return the catalog payload.
pub fn describe_catalog(
    core: &mut WorkspaceCore,
    workspace_id: &str,
    filter: &DescribeFilter,
    actor: ActorContext,
) -> Result<serde_json::Value> {
    let mut payload = serde_json::json!({});
    if let Some(tier) = &filter.tier {
        payload["tier"] = serde_json::Value::String(tier.clone());
    }
    if let Some(namespace) = &filter.namespace {
        payload["namespace"] = serde_json::Value::String(namespace.clone());
    }
    if let Some(names) = &filter.names {
        payload["names"] = serde_json::Value::Array(
            names.iter().cloned().map(serde_json::Value::String).collect(),
        );
    }
    if filter.include_inner {
        payload["include_inner"] = serde_json::json!(true);
    }
    if let Some(for_role) = &filter.for_role {
        payload["for_role"] = serde_json::Value::String(for_role.clone());
    }

    dispatch_local(
        core,
        build_core_command("system.describe", payload, workspace_id, actor, None),
    )
    .and_then(|resp| {
        if resp.ok {
            Ok(resp.payload)
        } else {
            Err(resp
                .error
                .unwrap_or_else(|| CoreError::RuntimeError("system.describe failed".into())))
        }
    })
}

/// Find one command entry inside a `system.describe` payload.
pub fn find_command_descriptor<'a>(
    catalog: &'a serde_json::Value,
    name: &str,
) -> Option<&'a serde_json::Value> {
    catalog
        .get("commands")
        .and_then(|v| v.as_array())
        .and_then(|entries| entries.iter().find(|entry| entry.get("name").and_then(|n| n.as_str()) == Some(name)))
}

/// True when `name` is an inner host-call surface entry.
pub fn is_inner_command(name: &str, catalog: &serde_json::Value) -> bool {
    if name.starts_with("ctx.") {
        return true;
    }
    find_command_descriptor(catalog, name)
        .and_then(|entry| entry.get("surface"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .map(|surface: CommandSurface| surface == CommandSurface::Inner)
        .unwrap_or(false)
}

/// Read JSON payload from a string, stdin (`-`), or file path.
pub fn parse_payload(flag: Option<&str>, file: Option<&Path>) -> Result<serde_json::Value> {
    if let Some(path) = file {
        let text = std::fs::read_to_string(path).map_err(|e| {
            CoreError::ValidationError(format!("read payload file {}: {e}", path.display()))
        })?;
        return parse_json_text(&text, &format!("file {}", path.display()));
    }

    match flag {
        None => Ok(serde_json::Value::Object(serde_json::Map::new())),
        Some("-") => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| CoreError::ValidationError(format!("read payload from stdin: {e}")))?;
            parse_json_text(&text, "stdin")
        }
        Some(text) => parse_json_text(text, "--payload"),
    }
}

fn parse_json_text(text: &str, source: &str) -> Result<serde_json::Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(trimmed).map_err(|e| {
        CoreError::ValidationError(format!("{source} is not valid JSON: {e}"))
    })
}

/// Build a [`CoreCommand`] envelope (F5) for local or remote dispatch.
pub fn build_core_command(
    name: &str,
    payload: serde_json::Value,
    workspace_id: &str,
    actor: ActorContext,
    applet_id: Option<&str>,
) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new(format!("req-{name}-{}", uuid_like_counter())),
        actor,
        workspace_id: WorkspaceId::new(workspace_id),
        applet_id: applet_id.map(Into::into),
        name: name.to_string(),
        payload,
    }
}

fn uuid_like_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Dispatch a command against a local core and return the full [`CoreResponse`].
pub fn dispatch_local(core: &mut WorkspaceCore, cmd: CoreCommand) -> Result<CoreResponse> {
    Ok(core.handle(cmd))
}

/// POST a [`CoreCommand`] to a server's `/bridge` endpoint.
pub fn post_bridge(
    server_url: &str,
    token: Option<&str>,
    cmd: &CoreCommand,
) -> Result<CoreResponse> {
    let body = serde_json::to_vec(cmd)
        .map_err(|e| CoreError::ValidationError(format!("serialize CoreCommand: {e}")))?;
    let response = http_post_json(server_url, token, &body)?;
    serde_json::from_slice(&response.body).map_err(|e| {
        CoreError::ValidationError(format!(
            "bridge response is not a CoreResponse (HTTP {}): {e}",
            response.status
        ))
    })
}

/// Resolve a catalog `payload_schema` path to an on-disk JSON Schema file.
pub fn resolve_schema_path(schema_rel_path: &str) -> PathBuf {
    let rel = schema_rel_path
        .strip_prefix("schemas/")
        .unwrap_or(schema_rel_path);
    schema_root().join(rel)
}

/// Validate `payload` against a catalog `payload_schema` path (dry-run).
pub fn validate_payload_schema(schema_rel_path: &str, payload: &serde_json::Value) -> Result<()> {
    let schema_path = resolve_schema_path(schema_rel_path);
    let text = std::fs::read_to_string(&schema_path).map_err(|e| {
        CoreError::ValidationError(format!(
            "read payload schema {}: {e}",
            schema_path.display()
        ))
    })?;
    let schema: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        CoreError::ValidationError(format!("payload schema is not valid JSON: {e}"))
    })?;
    validate_json_schema(&schema, payload, "$")
}

/// Issue a command locally or via `--server`, with dry-run and inner-surface guards.
pub fn run_command(
    name: &str,
    payload: serde_json::Value,
    opts: &RunOptions,
) -> Result<CommandOutcome> {
    let actor = actor_context(opts.actor_id.as_deref(), opts.role);

    if opts.server_url.is_none() {
        let mut core = open_core(&opts.workspace)?;
        let catalog = describe_catalog(
            &mut core,
            &opts.workspace.workspace_id,
            &DescribeFilter {
                tier: Some("debug".into()),
                include_inner: true,
                ..DescribeFilter::default()
            },
            actor.clone(),
        )?;

        if is_inner_command(name, &catalog) {
            return Err(CoreError::ValidationError(format!(
                "command {name:?} is an inner host-call (surface: inner); \
                 operators cannot issue ctx.* directly — drive the app via runtime.run \
                 or ui.dispatch_event instead"
            )));
        }

        let descriptor = find_command_descriptor(&catalog, name);
        if let Some(entry) = descriptor {
            if let Some(schema) = entry.get("payload_schema").and_then(|v| v.as_str()) {
                validate_payload_schema(schema, &payload)?;
            }
        }

        let envelope = build_core_command(
            name,
            payload,
            &opts.workspace.workspace_id,
            actor,
            opts.applet_id.as_deref(),
        );

        if opts.dry_run {
            return Ok(CommandOutcome {
                response: CoreResponse::ok(envelope.request_id.clone(), serde_json::json!({
                    "dry_run": true,
                    "envelope": envelope,
                })),
                envelope,
                events: Vec::new(),
            });
        }

        let response = dispatch_local(&mut core, envelope.clone())?;
        let events = if opts.emit_events && response.ok {
            core.events_mut().drain()
        } else {
            Vec::new()
        };
        return Ok(CommandOutcome {
            response,
            envelope,
            events,
        });
    }

    // Remote: identity is injected by the server; still validate envelope shape.
    let envelope = build_core_command(
        name,
        payload,
        &opts.workspace.workspace_id,
        ActorContext::owner("cli"),
        opts.applet_id.as_deref(),
    );

    if opts.dry_run {
        return Ok(CommandOutcome {
            response: CoreResponse::ok(envelope.request_id.clone(), serde_json::json!({
                "dry_run": true,
                "envelope": envelope,
            })),
            envelope,
            events: Vec::new(),
        });
    }

    let server_url = opts.server_url.as_deref().expect("server_url set");
    let env_token = std::env::var(FORGE_SERVER_TOKEN_ENV).ok();
    let token = opts.token.as_deref().or(env_token.as_deref());
    let response = post_bridge(server_url, token, &envelope)?;
    Ok(CommandOutcome {
        response,
        envelope,
        events: Vec::new(),
    })
}

/// Pretty-print drained [`CoreEvent`]s for non-JSON CLI output.
pub fn format_events(events: &[CoreEvent]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "[{}] {} (logical={})",
                event.event_id, event.kind, event.created_at_logical.0
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Call `system.trace` for a recorded run.
pub fn trace_run(
    core: &mut WorkspaceCore,
    run_id: &str,
    since_seq: Option<u64>,
    method: Option<&str>,
    actor: ActorContext,
    workspace_id: &str,
) -> Result<serde_json::Value> {
    let mut payload = serde_json::json!({ "run_id": run_id });
    if let Some(seq) = since_seq {
        payload["since_seq"] = serde_json::json!(seq);
    }
    if let Some(method) = method {
        payload["methods"] = serde_json::json!([method]);
    }

    let response = dispatch_local(
        core,
        build_core_command("system.trace", payload, workspace_id, actor, None),
    )?;
    if response.ok {
        Ok(response.payload)
    } else {
        Err(response
            .error
            .unwrap_or_else(|| CoreError::RuntimeError("system.trace failed".into())))
    }
}

/// Pretty-print the command list grouped by namespace.
pub fn format_commands_list(catalog: &serde_json::Value) -> String {
    let mut lines = Vec::new();
    let entries = catalog
        .get("commands")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut by_ns: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for entry in entries {
        let ns = entry
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        by_ns.entry(ns).or_default().push(entry);
    }

    for (ns, mut cmds) in by_ns {
        lines.push(ns);
        cmds.sort_by(|a, b| {
            a.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        });
        for cmd in cmds {
            lines.push(format!("  {}", format_command_line(&cmd)));
        }
        lines.push(String::new());
    }
    lines.join("\n").trim_end().to_string()
}

fn format_command_line(entry: &serde_json::Value) -> String {
    let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("?");
    let summary = entry.get("summary").and_then(|v| v.as_str()).unwrap_or("");
    let mut flags = String::new();
    if entry.get("mutates").and_then(|v| v.as_bool()).unwrap_or(false) {
        flags.push('✎');
    }
    if entry.get("effectful").and_then(|v| v.as_bool()).unwrap_or(false) {
        flags.push('⚡');
    }
    let visibility = entry
        .get("visibility")
        .and_then(|v| v.as_str())
        .unwrap_or("public");
    format!("{name:<22} {flags:<2} {visibility:<9} {summary}")
}

/// Pretty-print one command descriptor.
pub fn format_command_describe(entry: &serde_json::Value) -> String {
    let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("?");
    let visibility = entry
        .get("visibility")
        .and_then(|v| v.as_str())
        .unwrap_or("public");
    let surface = entry
        .get("surface")
        .and_then(|v| v.as_str())
        .unwrap_or("outer");
    let mutates = entry.get("mutates").and_then(|v| v.as_bool()).unwrap_or(false);
    let effectful = entry.get("effectful").and_then(|v| v.as_bool()).unwrap_or(false);
    let summary = entry.get("summary").and_then(|v| v.as_str()).unwrap_or("");

    let mut roles: Vec<String> = entry
        .get("required_roles")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    roles.sort();

    let mut lines = vec![
        format!("{name}  ({visibility}, {surface}{})", if mutates { ", mutates" } else { "" }),
        format!("  Summary: {summary}"),
    ];
    if effectful {
        lines.push("  Effectful: yes (host effects mediated by policy)".into());
    }
    if !roles.is_empty() {
        lines.push(format!("  Roles: {}", roles.join(", ")));
    }
    if let Some(schema) = entry.get("payload_schema").and_then(|v| v.as_str()) {
        lines.push(format!("  Payload schema: {schema}"));
    }
    if let Some(schema) = entry.get("response_schema").and_then(|v| v.as_str()) {
        lines.push(format!("  Response schema: {schema}"));
    }
    if let Some(events) = entry.get("events").and_then(|v| v.as_array()) {
        if !events.is_empty() {
            let names: Vec<_> = events
                .iter()
                .filter_map(|e| e.as_str())
                .collect();
            lines.push(format!("  Events: {}", names.join(", ")));
        }
    }
    lines.join("\n")
}

/// Generate usage text from the live catalog (plus the demo spine gate).
pub fn format_help_from_catalog(catalog: &serde_json::Value) -> String {
    let mut lines = vec![
        "forge — Terrane unified CLI (cli-plan/07 Phase 3)".into(),
        String::new(),
        "USAGE:".into(),
        "    forge <command> [options]".into(),
        String::new(),
        "COMMANDS:".into(),
        "    commands    List commands from system.describe".into(),
        "    describe    Show one command descriptor + schemas".into(),
        "    run         Issue a command (local core or --server)".into(),
        "    trace       Read inner host-call effects for a run (system.trace)".into(),
        "    demo        Run the notes-lite M0a spine gate (unchanged)".into(),
        "    help        Show this message".into(),
        String::new(),
        "NAMESPACES (from catalog):".into(),
    ];

    let entries = catalog
        .get("commands")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut namespaces: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        let ns = entry
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        *namespaces.entry(ns).or_default() += 1;
    }
    for (ns, count) in namespaces {
        lines.push(format!("    {ns} ({count})"));
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// JSON Schema validation (minimal subset for --dry-run)
// ---------------------------------------------------------------------------

fn validate_json_schema(schema: &serde_json::Value, value: &serde_json::Value, at: &str) -> Result<()> {
    let ty = schema.get("type").and_then(|v| v.as_str());
    if let Some(ty) = ty {
        if !value_matches_type(ty, value) {
            return Err(CoreError::ValidationError(format!(
                "{at}: expected type {ty}, got {}",
                json_type_name(value)
            )));
        }
    }

    if ty == Some("object") {
        let obj = value.as_object().ok_or_else(|| {
            CoreError::ValidationError(format!("{at}: expected object"))
        })?;
        if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
            for key in required {
                let key = key.as_str().ok_or_else(|| {
                    CoreError::ValidationError(format!("{at}: invalid required entry"))
                })?;
                if !obj.contains_key(key) {
                    return Err(CoreError::ValidationError(format!(
                        "{at}: missing required property {key:?}"
                    )));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
            if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
                for key in obj.keys() {
                    if !props.contains_key(key) {
                        return Err(CoreError::ValidationError(format!(
                            "{at}: additional property {key:?} is not allowed"
                        )));
                    }
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
            for (key, subschema) in props {
                if let Some(child) = obj.get(key) {
                    validate_json_schema(subschema, child, &format!("{at}.{key}"))?;
                }
            }
        }
    }

    if ty == Some("array") {
        let arr = value.as_array().ok_or_else(|| {
            CoreError::ValidationError(format!("{at}: expected array"))
        })?;
        if let Some(items) = schema.get("items") {
            for (i, item) in arr.iter().enumerate() {
                validate_json_schema(items, item, &format!("{at}[{i}]"))?;
            }
        }
    }

    if let Some(enum_values) = schema.get("enum").and_then(|v| v.as_array()) {
        if !enum_values.iter().any(|candidate| candidate == value) {
            return Err(CoreError::ValidationError(format!(
                "{at}: value must be one of {enum_values:?}"
            )));
        }
    }

    Ok(())
}

fn value_matches_type(ty: &str, value: &serde_json::Value) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Object(_) => "object",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Null => "null",
    }
}

// ---------------------------------------------------------------------------
// Minimal HTTP client for --server /bridge
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn http_post_json(url: &str, token: Option<&str>, body: &[u8]) -> Result<HttpResponse> {
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|e| {
        CoreError::PlatformUnavailable(format!("connect to {host}:{port}: {e}"))
    })?;

    write!(stream, "POST {path} HTTP/1.1\r\n").map_err(io_err)?;
    write!(stream, "Host: {host}\r\n").map_err(io_err)?;
    write!(stream, "Content-Type: application/json\r\n").map_err(io_err)?;
    write!(stream, "Content-Length: {}\r\n", body.len()).map_err(io_err)?;
    write!(stream, "Connection: close\r\n").map_err(io_err)?;
    if let Some(token) = token {
        write!(stream, "Authorization: Bearer {token}\r\n").map_err(io_err)?;
        write!(stream, "x-forge-server-token: {token}\r\n").map_err(io_err)?;
    }
    stream.write_all(b"\r\n").map_err(io_err)?;
    stream.write_all(body).map_err(io_err)?;

    read_http_response(&mut stream)
}

fn parse_http_url(url: &str) -> Result<(String, u16, String)> {
    let trimmed = url.trim();
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    if trimmed.starts_with("https://") {
        return Err(CoreError::ValidationError(
            "https:// is not supported by the minimal CLI HTTP client; use http://".into(),
        ));
    }

    let (host_port, path) = match without_scheme.split_once('/') {
        Some((hp, rest)) => (hp, format!("/{rest}")),
        None => (without_scheme, "/bridge".into()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port_str)) => {
            let port = port_str.parse::<u16>().map_err(|e| {
                CoreError::ValidationError(format!("invalid port in server URL: {e}"))
            })?;
            (host.to_string(), port)
        }
        None => (host_port.to_string(), 80),
    };
    Ok((host, port, path))
}

fn read_http_response(stream: &mut TcpStream) -> Result<HttpResponse> {
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(io_err)?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);

    let mut content_length = None;
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().ok();
            }
        }
    }

    let body_start = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    let mut body = buf[body_start..].to_vec();
    if let Some(len) = content_length {
        body.truncate(len);
    }
    Ok(HttpResponse { status, body })
}

fn io_err(e: std::io::Error) -> CoreError {
    CoreError::PlatformUnavailable(e.to_string())
}

// ---------------------------------------------------------------- helpers

/// Install an applet: parse the manifest JSON, derive the entrypoint source key,
/// and issue `applet.install` through the core. A non-`ok` response is surfaced
/// as the underlying [`CoreError`] (so a rejected eval/compile fails here).
pub fn install(
    core: &mut WorkspaceCore,
    applet_id: &str,
    manifest_json: &str,
    entry_ts: &str,
) -> Result<serde_json::Value> {
    let manifest: serde_json::Value = serde_json::from_str(manifest_json).map_err(|e| {
        CoreError::ValidationError(format!("manifest.json is not valid JSON: {e}"))
    })?;
    let entrypoint = manifest
        .get("entrypoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::ValidationError("manifest has no `entrypoint`".into()))?
        .to_string();

    handle(
        core,
        Some(applet_id),
        "applet.install",
        serde_json::json!({
            "manifest": manifest,
            "sources": { entrypoint: entry_ts },
        }),
    )
}

/// Issue a command through the core, mapping a non-`ok` [`forge_domain::CoreResponse`]
/// back into its [`CoreError`] so callers use `?` over the whole spine.
pub fn handle(
    core: &mut WorkspaceCore,
    applet_id: Option<&str>,
    name: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value> {
    let cmd = CoreCommand {
        request_id: RequestId::new(format!("req-{name}")),
        actor: ActorContext::owner("cli"),
        workspace_id: WorkspaceId::new("ws-demo"),
        applet_id: applet_id.map(Into::into),
        name: name.to_string(),
        payload,
    };
    let resp = core.handle(cmd);
    if resp.ok {
        Ok(resp.payload)
    } else {
        Err(resp
            .error
            .unwrap_or_else(|| CoreError::RuntimeError(format!("{name} failed without an error"))))
    }
}

/// List every record in `collection` via `query.execute`, returning the rows
/// (each `{ id, fields }`).
pub fn list_records(core: &mut WorkspaceCore, collection: &str) -> Result<Vec<serde_json::Value>> {
    let resp = handle(
        core,
        None,
        "query.execute",
        serde_json::json!({ "collection": collection }),
    )?;
    Ok(resp
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_demo_assets_match_examples_dir() {
        // The embeds and the on-disk examples/ files are the same bytes, so the
        // binary's self-contained demo cannot drift from the published example.
        let disk_ts = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/notes-lite/src/main.ts"
        ))
        .unwrap();
        let disk_manifest = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/notes-lite/manifest.json"
        ))
        .unwrap();
        assert_eq!(disk_ts, NOTES_LITE_MAIN_TS);
        assert_eq!(disk_manifest, NOTES_LITE_MANIFEST_JSON);
    }

    #[test]
    fn run_demo_drives_the_whole_spine() {
        let outcome = run_demo(serde_json::json!({ "title": "Buy milk" })).unwrap();
        assert!(outcome.run_ok, "demo run must complete ok");
        assert_eq!(outcome.result["value"]["count"], serde_json::json!(1));
        // A note record was stored (the SQLite write link).
        assert_eq!(outcome.notes.len(), 1);
        assert_eq!(outcome.notes[0]["fields"]["title"], serde_json::json!("Buy milk"));
        // A UI tree was produced (the tree-patch link).
        assert!(!outcome.ui_trees.is_empty());
        let tree = outcome.ui_trees[0].to_string();
        assert!(tree.contains("\"Notes\""), "header rendered: {tree}");
        assert!(tree.contains("\"Buy milk\""), "title in list: {tree}");
        // Replay reproduced it byte-identically (the determinism link).
        assert!(outcome.replay_identical, "demo run must replay identically");
    }

    #[test]
    fn dry_run_rejects_missing_required_field() {
        let opts = RunOptions {
            dry_run: true,
            workspace: WorkspaceOpenOptions {
                workspace_id: "ws".into(),
                ..WorkspaceOpenOptions::in_memory()
            },
            ..RunOptions::default()
        };
        let err = run_command(
            "query.execute",
            serde_json::json!({}),
            &opts,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("missing required property"),
            "{err}"
        );
    }

    #[test]
    fn inner_command_is_refused() {
        let opts = RunOptions {
            workspace: WorkspaceOpenOptions {
                workspace_id: "ws".into(),
                ..WorkspaceOpenOptions::in_memory()
            },
            ..RunOptions::default()
        };
        let err = run_command(
            "ctx.db.insert",
            serde_json::json!({}),
            &opts,
        )
        .unwrap_err();
        assert!(err.to_string().contains("inner host-call"), "{err}");
    }
}