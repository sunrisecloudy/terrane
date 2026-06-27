//! terrane-cli — the command-line adapter for Terrane.
//!
//! This crate turns `terrane <ns> <verb> [args…]` into calls against
//! `terrane-host`, then formats the result for humans. The reusable host spine
//! lives in `terrane-host`; this library exists so the `terrane` binary and the
//! native-host CLI wrapper can share the same argv routing.
//!
//! Catalog lives at `$TERRANE_HOME/log.bin` (default `./.terrane/`).

use std::path::PathBuf;

pub use terrane_host::{serve_conn, sync_conn, EdgeRunner};

/// Route an argv slice: `<ns> <verb> [args…]`, or one of the read/meta verbs.
pub fn run(argv: &[&str]) -> Result<(), String> {
    match argv {
        [] | ["help"] | ["--help"] | ["-h"] => {
            print_help();
            Ok(())
        }
        ["state"] => run_state(),
        ["log"] => run_log(),
        ["replay"] => run_replay(),
        ["app", "install", path] => run_install(path),
        ["contract", "export"] => run_contract_export(),
        ["serve"] => terrane_host::sync::run_serve(terrane_host::DEFAULT_SERVE_ADDR),
        ["serve", "--addr", addr] => terrane_host::sync::run_serve(addr),
        ["sync", app, "--from", home] => run_sync(app, home),
        ["sync", app, "--peer", addr] => terrane_host::sync::run_sync_peer(app, addr),
        ["sync", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane sync <app> (--from <home> | --peer <addr>)".into())
        }
        [ns, verb, rest @ ..] => run_command(ns, verb, rest),
        [other] => Err(format!("unknown command {other:?} (try `terrane help`)")),
    }
}

/// Generic write path: `<ns> <verb> [args…]` -> `dispatch("<ns>.<verb>", args)`.
pub fn run_command(ns: &str, verb: &str, rest: &[&str]) -> Result<(), String> {
    dispatch(&format!("{ns}.{verb}"), rest)
}

/// Dispatch a dotted command name + args and print the human CLI result.
pub fn dispatch(command: &str, args: &[&str]) -> Result<(), String> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    print_command_outcome(terrane_host::dispatch(command, &args)?);
    Ok(())
}

pub fn run_install(path: &str) -> Result<(), String> {
    println!("{}", terrane_host::install_app(path)?.message());
    Ok(())
}

pub fn run_sync(app: &str, from_home: &str) -> Result<(), String> {
    println!(
        "{}",
        terrane_host::sync_from_home(app, from_home)?.message()
    );
    Ok(())
}

/// Read the whole world (no network).
pub fn run_state() -> Result<(), String> {
    let core = terrane_host::open()?;
    let state = core.state();

    println!("apps:");
    if state.app.apps.is_empty() {
        println!("  (none)");
    }
    for app in state.app.apps.values() {
        match &app.source {
            Some(src) => println!("  {} — {}  [{}]", app.id, app.name, src),
            None => println!("  {} — {}", app.id, app.name),
        }
    }

    println!("kv:");
    if state.kv.data.is_empty() {
        println!("  (none)");
    }
    for (app, kv) in &state.kv.data {
        for (key, value) in kv {
            println!("  {app}/{key} = {value}");
        }
    }

    println!("fetches:");
    if state.net.fetches.is_empty() {
        println!("  (none)");
    }
    for (app, responses) in &state.net.fetches {
        for (url, resp) in responses {
            println!(
                "  {app} {url} -> {} ({} bytes)",
                resp.status,
                resp.body.len()
            );
        }
    }

    println!("model:");
    if state.model.turns.is_empty() {
        println!("  (none)");
    }
    for (app, turns) in &state.model.turns {
        for turn in turns {
            println!(
                "  {app} [{}] exit {} ({} chars)",
                turn.agent,
                turn.exit_code,
                turn.response.len()
            );
        }
    }

    println!("builder:");
    if state.builder.drafts.is_empty() {
        println!("  (none)");
    }
    for draft in state.builder.drafts.values() {
        let status = if draft.error.is_some() {
            "failed"
        } else if draft.files.is_empty() {
            "requested"
        } else {
            "generated"
        };
        println!(
            "  {} — {} [{}] {} files",
            draft.app_id,
            draft.name,
            status,
            draft.files.len()
        );
    }

    println!("codex:");
    if state.codex.runs.is_empty() {
        println!("  (none)");
    }
    for run in state.codex.runs.values() {
        let status = if run.error.is_some() {
            "failed"
        } else if run.output.is_some() {
            "completed"
        } else if run.js.is_some() {
            "generated"
        } else {
            "requested"
        };
        println!("  {} — {} [{}]", run.id, run.app_id, status);
    }
    Ok(())
}

/// Decode and print the event log, capability-described.
pub fn run_log() -> Result<(), String> {
    let core = terrane_host::open()?;
    let lines = core.log_lines().map_err(|e| e.to_string())?;
    if lines.is_empty() {
        println!("(empty log)");
        return Ok(());
    }
    for (i, line) in lines.iter().enumerate() {
        println!("{:>4}  {line}", i + 1);
    }
    Ok(())
}

/// The public API surface assembled from the running `terrane-api` +
/// `terrane-core` declarations.
pub fn contract_surface() -> terrane_api::PublicSurface {
    terrane_host::contract_surface()
}

/// `contract export` — print [`contract_surface`] as JSON.
pub fn run_contract_export() -> Result<(), String> {
    use nanoserde::SerJson;
    println!("{}", contract_surface().serialize_json());
    Ok(())
}

pub fn run_replay() -> Result<(), String> {
    let core = terrane_host::open()?;
    if core.replay_matches().map_err(|e| e.to_string())? {
        println!("replay ok: state reproduced identically from the log");
        Ok(())
    } else {
        Err("replay mismatch: log does not reproduce current state".into())
    }
}

/// Open the workspace core at `$TERRANE_HOME/log.bin` with the real edge runner.
pub fn open() -> Result<terrane_host::HostCore, String> {
    terrane_host::open()
}

/// The home directory: `$TERRANE_HOME` (default `./.terrane/`).
pub fn home_dir() -> PathBuf {
    terrane_host::home_dir()
}

/// The on-disk log path: `$TERRANE_HOME/log.bin`.
pub fn log_path() -> PathBuf {
    terrane_host::log_path()
}

fn print_command_outcome(outcome: terrane_host::CommandOutcome) {
    match outcome.output {
        Some(output) => println!("{output}"),
        None if outcome.records.is_empty() => println!("(no change)"),
        None => {
            for record in &outcome.records {
                println!("-> {}", record.kind);
            }
        }
    }
}

pub fn print_help() {
    println!(
        "terrane — your local app catalog\n\n\
         Commands are <namespace> <verb> [args…], routed to the capability that\n\
         owns that namespace. Built-in capabilities:\n\n\
         \x20 terrane app install <path>                       copy a bundle into the home & catalog it\n\
         \x20 terrane app add <id> <name…> [--source <path>]   catalog an app by path (dev)\n\
         \x20 terrane app remove <id>                          remove an app\n\
         \x20 terrane kv set <app> <key> <value…>              store a value\n\
         \x20 terrane kv rm <app> <key>                        delete a value\n\
         \x20 terrane net fetch <app> <url>                    GET a url; record it\n\
         \x20 terrane model ask <app> <claude|codex> <prompt…> ask an agent; record it\n\
         \x20 terrane codex generate-app [--harness <codex|claude-code|opencode>] <draft> <app> <name> <prompt…>\n\
         \x20 terrane codex run-js [--harness <codex|claude-code|opencode>] <run> <app> <prompt…>\n\
         \x20 terrane host run <app> [input…]                  run an app's JS backend\n\n\
         Multi-user:\n\
         \x20 terrane serve [--addr <addr>]      listen for peers (default 127.0.0.1:7777)\n\
         \x20 terrane sync <app> --from <home>   merge another home's edits (local)\n\
         \x20 terrane sync <app> --peer <addr>   merge a serving peer's edits (network)\n\n\
         Reads & meta:\n\
         \x20 terrane state                  print the whole world\n\
         \x20 terrane log                    print the event log (decoded)\n\
         \x20 terrane replay                 rebuild state from the log and verify it\n\
         \x20 terrane contract export        print the public API contract (JSON)\n\
         \x20 terrane help                   this message\n\n\
         Catalog: $TERRANE_HOME/log.bin (binary event log; default ./.terrane/)"
    );
}
