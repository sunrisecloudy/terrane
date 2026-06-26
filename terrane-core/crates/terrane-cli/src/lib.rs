//! terrane-cli — the reusable CLI host spine.
//!
//! A thin, capability-agnostic client: it turns `terrane <ns> <verb> [args…]`
//! into a [`Request`] and hands it to the core, which routes it to whatever
//! capability owns that namespace. Reads (`state`, `log`) and meta (`replay`)
//! are the only non-generic verbs.
//!
//! This crate is a *library* so other hosts (e.g. `terrane-host`) can reuse the
//! spine — [`run`], [`dispatch`], [`open`], the [`EdgeRunner`] — and add their
//! own front doors. The `terrane` binary in `main.rs` is a thin wrapper.
//!
//! Catalog lives at `$TERRANE_HOME/log.bin` (default `./.terrane/`).

use std::env;
use std::path::PathBuf;

use terrane_core::Core;
use terrane_domain::Request;

pub mod edge;
pub use edge::EdgeRunner;

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
        [ns, verb, rest @ ..] => run_command(ns, verb, rest),
        [other] => Err(format!("unknown command {other:?} (try `terrane help`)")),
    }
}

/// Generic write path: `<ns> <verb> [args…]` → `dispatch("<ns>.<verb>", args)`.
pub fn run_command(ns: &str, verb: &str, rest: &[&str]) -> Result<(), String> {
    dispatch(&format!("{ns}.{verb}"), rest)
}

/// Build a Request from a dotted command name + args, dispatch it, print the
/// resulting event kinds (or `(no change)`), then print any `host.run` backend
/// output. Shared by `run_command` and external hosts (e.g. `terrane-host run`
/// → `dispatch("host.run", …)`).
pub fn dispatch(command: &str, args: &[&str]) -> Result<(), String> {
    let mut core = open()?;
    let request = Request::new(command, args.iter().map(|s| s.to_string()).collect());
    let records = core.dispatch(request).map_err(|e| e.to_string())?;
    // A `host.run` returns a backend string — that IS the result, so print it
    // alone (the kv.* records it produced are still visible via `terrane log`).
    // Every other command reports the event kinds it committed.
    match core.take_last_output() {
        Some(output) => println!("{output}"),
        None if records.is_empty() => println!("(no change)"),
        None => {
            for record in &records {
                println!("→ {}", record.kind);
            }
        }
    }
    Ok(())
}

/// Read the whole world (no network).
pub fn run_state() -> Result<(), String> {
    let core = open()?;
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
            println!("  {app} {url} → {} ({} bytes)", resp.status, resp.body.len());
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
    Ok(())
}

/// Decode and print the event log, capability-described.
pub fn run_log() -> Result<(), String> {
    let core = open()?;
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

pub fn run_replay() -> Result<(), String> {
    let core = open()?;
    if core.replay_matches().map_err(|e| e.to_string())? {
        println!("replay ok: state reproduced identically from the log");
        Ok(())
    } else {
        Err("replay mismatch: log does not reproduce current state".into())
    }
}

/// Open the workspace core at `$TERRANE_HOME/log.bin` with the real edge runner.
pub fn open() -> Result<Core<EdgeRunner>, String> {
    Core::open_with(log_path(), EdgeRunner).map_err(|e| e.to_string())
}

/// The on-disk log path: `$TERRANE_HOME/log.bin` (default `./.terrane/`).
pub fn log_path() -> PathBuf {
    env::var("TERRANE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".terrane"))
        .join("log.bin")
}

pub fn print_help() {
    println!(
        "terrane — your local app catalog\n\n\
         Commands are <namespace> <verb> [args…], routed to the capability that\n\
         owns the namespace. Built-in capabilities:\n\n\
         \x20 terrane app add <id> <name…> [--source <path>]   save an app\n\
         \x20 terrane app remove <id>                          remove an app\n\
         \x20 terrane kv set <app> <key> <value…>              store a value\n\
         \x20 terrane kv rm <app> <key>                        delete a value\n\
         \x20 terrane net fetch <app> <url>                    GET a url; record it\n\
         \x20 terrane model ask <app> <claude|codex> <prompt…> ask an agent; record it\n\
         \x20 terrane host run <app> [input…]                  run an app's JS backend\n\n\
         Reads & meta:\n\
         \x20 terrane state                  print the whole world\n\
         \x20 terrane log                    print the event log (decoded)\n\
         \x20 terrane replay                 rebuild state from the log and verify it\n\
         \x20 terrane help                   this message\n\n\
         Catalog: $TERRANE_HOME/log.bin (binary event log; default ./.terrane/)"
    );
}
