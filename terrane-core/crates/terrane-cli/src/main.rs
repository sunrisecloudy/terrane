//! terrane — the CLI front door.
//!
//! A thin, capability-agnostic client: it turns `terrane <ns> <verb> [args…]`
//! into a [`Request`] and hands it to the core, which routes it to whatever
//! capability owns that namespace. Adding a new capability needs no change here.
//! Reads (`state`, `log`) and meta (`replay`) are the only non-generic verbs.
//!
//! Catalog lives at `$TERRANE_HOME/log.bin` (default `./.terrane/`).

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use terrane_core::Core;
use terrane_domain::Request;

mod net;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("terrane: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(argv: &[&str]) -> Result<(), String> {
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

/// Generic write path: any `<ns> <verb> [args…]` becomes a Request.
fn run_command(ns: &str, verb: &str, rest: &[&str]) -> Result<(), String> {
    let mut core = open()?;
    let request = Request::new(
        format!("{ns}.{verb}"),
        rest.iter().map(|s| s.to_string()).collect(),
    );
    let records = core.dispatch(request).map_err(|e| e.to_string())?;
    if records.is_empty() {
        println!("(no change)");
    } else {
        for record in &records {
            println!("→ {}", record.kind);
        }
    }
    Ok(())
}

/// Read the whole world (no network).
fn run_state() -> Result<(), String> {
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
    Ok(())
}

/// Decode and print the event log, capability-described.
fn run_log() -> Result<(), String> {
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

fn run_replay() -> Result<(), String> {
    let core = open()?;
    if core.replay_matches().map_err(|e| e.to_string())? {
        println!("replay ok: state reproduced identically from the log");
        Ok(())
    } else {
        Err("replay mismatch: log does not reproduce current state".into())
    }
}

fn open() -> Result<Core<net::HttpGetRunner>, String> {
    Core::open_with(log_path(), net::HttpGetRunner).map_err(|e| e.to_string())
}

fn log_path() -> PathBuf {
    env::var("TERRANE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".terrane"))
        .join("log.bin")
}

fn print_help() {
    println!(
        "terrane — your local app catalog\n\n\
         Commands are <namespace> <verb> [args…], routed to the capability that\n\
         owns the namespace. Built-in capabilities:\n\n\
         \x20 terrane app add <id> <name…> [--source <path>]   save an app\n\
         \x20 terrane app remove <id>                          remove an app\n\
         \x20 terrane kv set <app> <key> <value…>              store a value\n\
         \x20 terrane kv rm <app> <key>                        delete a value\n\
         \x20 terrane net fetch <app> <url>                    GET a url; record it\n\n\
         Reads & meta:\n\
         \x20 terrane state                  print the whole world\n\
         \x20 terrane log                    print the event log (decoded)\n\
         \x20 terrane replay                 rebuild state from the log and verify it\n\
         \x20 terrane help                   this message\n\n\
         Catalog: $TERRANE_HOME/log.bin (binary event log; default ./.terrane/)"
    );
}
