//! terrane — the CLI front door.
//!
//! A thin arg parser: it builds a [`Command`], hands it to [`terrane_core::Core`],
//! and renders the result. It never touches the event log or State directly —
//! every mutation goes through the engine, every read goes through `state()`.
//!
//! Catalog lives at `$TERRANE_HOME/log.bin` (default `./.terrane/`).

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use terrane_core::Core;
use terrane_domain::{Command, Event};

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
        ["app", rest @ ..] => run_app(rest),
        ["kv", rest @ ..] => run_kv(rest),
        ["fetch", app, url] => run_fetch(app, url),
        ["fetched", app] => run_fetched(app),
        ["log"] => run_log(),
        ["replay"] => run_replay(),
        [other, ..] => Err(format!("unknown command {other:?} (try `terrane help`)")),
    }
}

fn run_app(argv: &[&str]) -> Result<(), String> {
    match argv {
        ["add", id, rest @ ..] => {
            let (name, source) = parse_add(rest)?;
            let mut core = open()?;
            core.execute(Command::AddApp {
                id: (*id).to_string(),
                name,
                source,
            })
            .map_err(|e| e.to_string())?;
            println!("added {id}");
            Ok(())
        }
        ["add", ..] => Err("usage: terrane app add <id> <name…> [--source <path>]".into()),
        ["list"] => {
            let core = open()?;
            let apps = &core.state().apps;
            if apps.is_empty() {
                println!("(no apps yet — `terrane app add <id> <name>`)");
            } else {
                for app in apps.values() {
                    println!("{}\t{}", app.id, app.name);
                }
            }
            Ok(())
        }
        ["show", id] => {
            let core = open()?;
            match core.state().apps.get(*id) {
                Some(app) => {
                    println!("id:     {}", app.id);
                    println!("name:   {}", app.name);
                    println!("source: {}", app.source.as_deref().unwrap_or("(none)"));
                    Ok(())
                }
                None => Err(format!("app not found: {id}")),
            }
        }
        ["rm", id] => {
            let mut core = open()?;
            core.execute(Command::RemoveApp {
                id: (*id).to_string(),
            })
            .map_err(|e| e.to_string())?;
            println!("removed {id}");
            Ok(())
        }
        _ => Err("usage: terrane app <add|list|show|rm> …".into()),
    }
}

fn run_kv(argv: &[&str]) -> Result<(), String> {
    match argv {
        ["set", app, key, value @ ..] if !value.is_empty() => {
            let mut core = open()?;
            core.execute(Command::KvSet {
                app: (*app).to_string(),
                key: (*key).to_string(),
                value: value.join(" "),
            })
            .map_err(|e| e.to_string())?;
            println!("set {app}/{key}");
            Ok(())
        }
        ["set", ..] => Err("usage: terrane kv set <app> <key> <value…>".into()),
        ["get", app, key] => {
            let core = open()?;
            match core.state().data.get(*app).and_then(|kv| kv.get(*key)) {
                Some(value) => {
                    println!("{value}");
                    Ok(())
                }
                None => Err(format!("key not found: {app}/{key}")),
            }
        }
        ["list", app] => {
            let core = open()?;
            match core.state().data.get(*app) {
                Some(kv) if !kv.is_empty() => {
                    for (key, value) in kv {
                        println!("{key}\t{value}");
                    }
                    Ok(())
                }
                _ => {
                    println!("(no data for {app})");
                    Ok(())
                }
            }
        }
        ["rm", app, key] => {
            let mut core = open()?;
            core.execute(Command::KvDelete {
                app: (*app).to_string(),
                key: (*key).to_string(),
            })
            .map_err(|e| e.to_string())?;
            println!("removed {app}/{key}");
            Ok(())
        }
        _ => Err("usage: terrane kv <set|get|list|rm> …".into()),
    }
}

/// Perform a network fetch for an app. The HTTP GET runs here, at the edge; its
/// result is recorded as an event, so `replay` reproduces it without the network.
fn run_fetch(app: &str, url: &str) -> Result<(), String> {
    let mut core = open()?;
    let events = core
        .execute(Command::Fetch {
            app: app.to_string(),
            url: url.to_string(),
        })
        .map_err(|e| e.to_string())?;
    if let Some(Event::Fetched { status, body, .. }) = events.first() {
        println!("fetched {url} → {status} ({} bytes)", body.len());
    }
    Ok(())
}

/// List an app's recorded network responses (read from State, no network).
fn run_fetched(app: &str) -> Result<(), String> {
    let core = open()?;
    match core.state().fetches.get(app) {
        Some(responses) if !responses.is_empty() => {
            for (url, resp) in responses {
                println!("{url}\t{} ({} bytes)", resp.status, resp.body.len());
            }
            Ok(())
        }
        _ => {
            println!("(no fetches for {app})");
            Ok(())
        }
    }
}

/// Decode the binary event log and print it for humans — the inspectability we
/// kept when moving the log from JSON to borsh.
fn run_log() -> Result<(), String> {
    let events = terrane_core::read_log(&log_path()).map_err(|e| e.to_string())?;
    if events.is_empty() {
        println!("(empty log)");
        return Ok(());
    }
    for (i, event) in events.iter().enumerate() {
        println!("{:>4}  {event:?}", i + 1);
    }
    Ok(())
}

fn run_replay() -> Result<(), String> {
    let core = open()?;
    let ok = core.replay_matches().map_err(|e| e.to_string())?;
    let n = core.state().apps.len();
    if ok {
        println!("replay ok: {n} app(s), state reproduced identically from the log");
        Ok(())
    } else {
        Err("replay mismatch: log does not reproduce current state".into())
    }
}

/// Parse the tail of `app add <id> …` into `(name, source)`, pulling out an
/// optional `--source <path>` flag from among the name words.
fn parse_add(rest: &[&str]) -> Result<(String, Option<String>), String> {
    let mut name_parts: Vec<&str> = Vec::new();
    let mut source = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--source" => {
                let path = rest
                    .get(i + 1)
                    .ok_or("`--source` needs a path")?;
                source = Some((*path).to_string());
                i += 2;
            }
            word => {
                name_parts.push(word);
                i += 1;
            }
        }
    }
    if name_parts.is_empty() {
        return Err("usage: terrane app add <id> <name…> [--source <path>]".into());
    }
    Ok((name_parts.join(" "), source))
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
         USAGE:\n\
         \x20 terrane app add <id> <name…> [--source <path>]  save an app\n\
         \x20 terrane app list               list saved apps\n\
         \x20 terrane app show <id>          show one app\n\
         \x20 terrane app rm <id>            remove an app (and its data)\n\
         \x20 terrane kv set <app> <key> <value…>   store a value for an app\n\
         \x20 terrane kv get <app> <key>     read a value\n\
         \x20 terrane kv list <app>          list an app's stored data\n\
         \x20 terrane kv rm <app> <key>      delete a value\n\
         \x20 terrane fetch <app> <url>      GET a url; record the response\n\
         \x20 terrane fetched <app>          list an app's recorded responses\n\
         \x20 terrane log                    print the event log (decoded)\n\
         \x20 terrane replay                 rebuild state from the log and verify it\n\
         \x20 terrane help                   this message\n\n\
         Catalog: $TERRANE_HOME/log.bin (binary event log; default ./.terrane/)"
    );
}
