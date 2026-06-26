//! `forge` — the Terrane CLI (cli-plan/07 Phase 3).
//!
//! Subcommands: `commands`, `describe`, `run`, `trace`, `demo`, `help`.
//! The process exits non-zero when a command returns `ok:false` or on transport
//! errors, so CI can gate on the M0a `demo` spine staying green.

use forge_cli::{
    actor_context, describe_catalog, find_command_descriptor, format_command_describe,
    format_commands_list, format_events, format_help_from_catalog, open_core, parse_payload,
    parse_role, run_command, trace_run, DescribeFilter, RunOptions, WorkspaceOpenOptions,
    DEFAULT_WORKSPACE_ID, FORGE_SERVER_TOKEN_ENV,
};
use forge_domain::CoreError;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_and_run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_and_run(args: &[String]) -> forge_domain::Result<()> {
    let Some(cmd) = args.first().map(String::as_str) else {
        print_help()?;
        return Ok(());
    };

    match cmd {
        "demo" => run_demo(),
        "help" | "--help" | "-h" => {
            print_help()?;
            Ok(())
        }
        "commands" => run_commands(&args[1..]),
        "describe" => run_describe(&args[1..]),
        "run" => run_run(&args[1..]),
        "trace" => run_trace(&args[1..]),
        other => {
            eprintln!("forge: unknown subcommand {other:?}\n");
            print_help()?;
            Err(CoreError::ValidationError(format!("unknown subcommand {other:?}")))
        }
    }
}

fn run_demo() -> forge_domain::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match forge_cli::demo(&mut out) {
        Ok(outcome) if outcome.run_ok && outcome.replay_identical => Ok(()),
        Ok(outcome) => Err(CoreError::RuntimeError(format!(
            "forge demo: spine assertion failed (run_ok={}, replay_identical={})",
            outcome.run_ok, outcome.replay_identical
        ))),
        Err(e) => Err(e),
    }
}

fn run_commands(args: &[String]) -> forge_domain::Result<()> {
    let parsed = CommonFlags::parse(args)?;
    let actor = parsed.actor();
    let mut core = open_core(&parsed.workspace)?;
    let filter = DescribeFilter {
        tier: parsed.tier,
        namespace: parsed.namespace,
        include_inner: parsed.include_inner,
        for_role: parsed.for_role,
        ..DescribeFilter::default()
    };
    let catalog = describe_catalog(
        &mut core,
        &parsed.workspace.workspace_id,
        &filter,
        actor,
    )?;

    if parsed.json {
        println!("{}", serde_json::to_string_pretty(&catalog).unwrap());
    } else {
        println!("{}", format_commands_list(&catalog));
    }
    Ok(())
}

fn run_describe(args: &[String]) -> forge_domain::Result<()> {
    let (name, parsed) = CommandNameArgs::parse(args)?;
    let actor = parsed.actor();
    let mut core = open_core(&parsed.workspace)?;
    let filter = DescribeFilter {
        tier: Some("debug".into()),
        names: Some(vec![name.clone()]),
        include_inner: true,
        for_role: parsed.for_role,
        ..DescribeFilter::default()
    };
    let catalog = describe_catalog(
        &mut core,
        &parsed.workspace.workspace_id,
        &filter,
        actor,
    )?;

    let entry = find_command_descriptor(&catalog, &name).ok_or_else(|| {
        CoreError::ValidationError(format!("command {name:?} not found in catalog"))
    })?;

    if parsed.json {
        println!("{}", serde_json::to_string_pretty(entry).unwrap());
    } else {
        println!("{}", format_command_describe(entry));
    }
    Ok(())
}

fn run_run(args: &[String]) -> forge_domain::Result<()> {
    let (name, parsed) = CommandNameArgs::parse(args)?;
    let payload = parse_payload(parsed.payload.as_deref(), parsed.file.as_deref())?;
    let opts = RunOptions {
        workspace: parsed.workspace,
        actor_id: parsed.actor_id,
        role: parsed.role,
        applet_id: parsed.applet_id,
        server_url: parsed.server,
        token: parsed.token,
        dry_run: parsed.dry_run,
        emit_events: parsed.emit_events,
    };

    let outcome = run_command(&name, payload, &opts)?;
    if parsed.json {
        println!("{}", serde_json::to_string_pretty(&outcome.response).unwrap());
        if opts.emit_events && !outcome.events.is_empty() {
            println!("{}", serde_json::to_string_pretty(&outcome.events).unwrap());
        }
    } else if opts.dry_run {
        println!("dry-run ok");
        println!("{}", serde_json::to_string_pretty(&outcome.envelope).unwrap());
    } else if outcome.response.ok {
        println!("{}", serde_json::to_string_pretty(&outcome.response.payload).unwrap());
        if opts.emit_events && !outcome.events.is_empty() {
            println!("{}", format_events(&outcome.events));
        }
    } else {
        let err = outcome
            .response
            .error
            .unwrap_or_else(|| CoreError::RuntimeError(format!("{name} failed")));
        return Err(err);
    }
    Ok(())
}

fn run_trace(args: &[String]) -> forge_domain::Result<()> {
    let (run_id, parsed) = RunIdArgs::parse(args)?;
    let mut core = open_core(&parsed.workspace)?;
    let payload = trace_run(
        &mut core,
        &run_id,
        parsed.since_seq,
        parsed.method.as_deref(),
        parsed.actor(),
        &parsed.workspace.workspace_id,
    )?;

    if parsed.json {
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        println!("run_id: {run_id}");
        if let Some(calls) = payload.get("calls").and_then(|v| v.as_array()) {
            for call in calls {
                let seq = call.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
                let method = call.get("method").and_then(|v| v.as_str()).unwrap_or("?");
                println!("  [{seq}] {method}");
            }
        }
        if payload
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            println!("(truncated)");
        }
    }
    Ok(())
}

fn print_help() -> forge_domain::Result<()> {
    let mut core = open_core(&WorkspaceOpenOptions::default())?;
    let catalog = describe_catalog(
        &mut core,
        DEFAULT_WORKSPACE_ID,
        &DescribeFilter {
            tier: Some("operator".into()),
            ..DescribeFilter::default()
        },
        actor_context(None, None),
    )?;
    println!("{}", format_help_from_catalog(&catalog));
    Ok(())
}

// ---------------------------------------------------------------------------
// Minimal arg parser (no clap — workspace has no clap dep)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CommonFlags {
    workspace: WorkspaceOpenOptions,
    tier: Option<String>,
    namespace: Option<String>,
    include_inner: bool,
    for_role: Option<String>,
    actor_id: Option<String>,
    role: Option<forge_domain::Role>,
    json: bool,
}

impl CommonFlags {
    fn parse(args: &[String]) -> forge_domain::Result<Self> {
        let mut out = CommonFlags {
            workspace: WorkspaceOpenOptions::default(),
            tier: None,
            namespace: None,
            include_inner: false,
            for_role: None,
            actor_id: None,
            role: None,
            json: false,
        };
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--workspace" => {
                    let path = require_value(args, &mut i, "--workspace")?;
                    out.workspace.path = Some(PathBuf::from(path));
                    out.workspace.in_memory = false;
                }
                "--in-memory" => out.workspace.in_memory = true,
                "--tier" => out.tier = Some(require_value(args, &mut i, "--tier")?.into()),
                "--namespace" => {
                    out.namespace = Some(require_value(args, &mut i, "--namespace")?.into())
                }
                "--include-inner" => out.include_inner = true,
                "--for-role" => {
                    out.for_role = Some(require_value(args, &mut i, "--for-role")?.into())
                }
                "--actor" => out.actor_id = Some(require_value(args, &mut i, "--actor")?.into()),
                "--role" => {
                    let role = require_value(args, &mut i, "--role")?;
                    out.role = Some(parse_role(role)?);
                }
                "--json" => out.json = true,
                other => {
                    return Err(CoreError::ValidationError(format!(
                        "unknown flag {other:?}"
                    )));
                }
            }
            i += 1;
        }
        Ok(out)
    }

    fn actor(&self) -> forge_domain::ActorContext {
        actor_context(self.actor_id.as_deref(), self.role)
    }
}

#[derive(Debug, Clone)]
struct CommandNameArgs {
    workspace: WorkspaceOpenOptions,
    payload: Option<String>,
    file: Option<PathBuf>,
    actor_id: Option<String>,
    role: Option<forge_domain::Role>,
    applet_id: Option<String>,
    server: Option<String>,
    token: Option<String>,
    dry_run: bool,
    emit_events: bool,
    json: bool,
    for_role: Option<String>,
}

impl CommandNameArgs {
    fn parse(args: &[String]) -> forge_domain::Result<(String, Self)> {
        let Some(name) = args.first().cloned() else {
            return Err(CoreError::ValidationError(
                "missing command name (usage: forge run <name> [options])".into(),
            ));
        };

        let mut out = CommandNameArgs {
            workspace: WorkspaceOpenOptions::default(),
            payload: None,
            file: None,
            actor_id: None,
            role: None,
            applet_id: None,
            server: None,
            token: None,
            dry_run: false,
            emit_events: false,
            json: false,
            for_role: None,
        };

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--workspace" => {
                    let path = require_value(args, &mut i, "--workspace")?;
                    out.workspace.path = Some(PathBuf::from(path));
                    out.workspace.in_memory = false;
                }
                "--in-memory" => out.workspace.in_memory = true,
                "--events" => out.emit_events = true,
                "--payload" => {
                    out.payload = Some(require_value(args, &mut i, "--payload")?.into())
                }
                "--file" => {
                    let path = require_value(args, &mut i, "--file")?;
                    out.file = Some(PathBuf::from(path));
                }
                "--actor" => out.actor_id = Some(require_value(args, &mut i, "--actor")?.into()),
                "--role" => {
                    let role = require_value(args, &mut i, "--role")?;
                    out.role = Some(parse_role(role)?);
                }
                "--applet" => {
                    out.applet_id = Some(require_value(args, &mut i, "--applet")?.into())
                }
                "--server" => out.server = Some(require_value(args, &mut i, "--server")?.into()),
                "--token" => out.token = Some(require_value(args, &mut i, "--token")?.into()),
                "--dry-run" => out.dry_run = true,
                "--json" => out.json = true,
                "--for-role" => {
                    out.for_role = Some(require_value(args, &mut i, "--for-role")?.into())
                }
                other => {
                    return Err(CoreError::ValidationError(format!(
                        "unknown flag {other:?}"
                    )));
                }
            }
            i += 1;
        }

        if out.token.is_none() {
            if let Ok(token) = std::env::var(FORGE_SERVER_TOKEN_ENV) {
                out.token = Some(token);
            }
        }

        Ok((name, out))
    }

    fn actor(&self) -> forge_domain::ActorContext {
        actor_context(self.actor_id.as_deref(), self.role)
    }
}

#[derive(Debug, Clone)]
struct RunIdArgs {
    workspace: WorkspaceOpenOptions,
    since_seq: Option<u64>,
    method: Option<String>,
    actor_id: Option<String>,
    role: Option<forge_domain::Role>,
    json: bool,
}

impl RunIdArgs {
    fn parse(args: &[String]) -> forge_domain::Result<(String, Self)> {
        let Some(run_id) = args.first().cloned() else {
            return Err(CoreError::ValidationError(
                "missing run_id (usage: forge trace <run_id> [options])".into(),
            ));
        };

        let mut out = RunIdArgs {
            workspace: WorkspaceOpenOptions::default(),
            since_seq: None,
            method: None,
            actor_id: None,
            role: None,
            json: false,
        };

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--workspace" => {
                    let path = require_value(args, &mut i, "--workspace")?;
                    out.workspace.path = Some(PathBuf::from(path));
                    out.workspace.in_memory = false;
                }
                "--in-memory" => out.workspace.in_memory = true,
                "--since-seq" => {
                    let value = require_value(args, &mut i, "--since-seq")?;
                    out.since_seq = Some(value.parse().map_err(|e| {
                        CoreError::ValidationError(format!("invalid --since-seq: {e}"))
                    })?);
                }
                "--method" => out.method = Some(require_value(args, &mut i, "--method")?.into()),
                "--actor" => out.actor_id = Some(require_value(args, &mut i, "--actor")?.into()),
                "--role" => {
                    let role = require_value(args, &mut i, "--role")?;
                    out.role = Some(parse_role(role)?);
                }
                "--json" => out.json = true,
                other => {
                    return Err(CoreError::ValidationError(format!(
                        "unknown flag {other:?}"
                    )));
                }
            }
            i += 1;
        }
        Ok((run_id, out))
    }

    fn actor(&self) -> forge_domain::ActorContext {
        actor_context(self.actor_id.as_deref(), self.role)
    }
}

fn require_value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> forge_domain::Result<&'a str> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| CoreError::ValidationError(format!("{flag} requires a value")))
}