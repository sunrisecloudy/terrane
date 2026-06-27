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
use std::path::{Path, PathBuf};

use terrane_core::Core;
use terrane_domain::{Error, Request};

pub mod edge;
pub mod sync;
pub use edge::EdgeRunner;
pub use sync::{serve_conn, sync_conn};

/// Default address `terrane serve` binds when none is given.
const DEFAULT_SERVE_ADDR: &str = "127.0.0.1:7777";

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
        ["serve"] => sync::run_serve(DEFAULT_SERVE_ADDR),
        ["serve", "--addr", addr] => sync::run_serve(addr),
        ["sync", app, "--from", home] => run_sync(app, home),
        ["sync", app, "--peer", addr] => sync::run_sync_peer(app, addr),
        ["sync", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane sync <app> (--from <home> | --peer <addr>)".into())
        }
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
    ensure_identity(&mut core)?;
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

/// `app install <path>`: copy a bundle into this home's `apps/<id>/` and catalog
/// it from there, so the home OWNS the app — no dependence on an external path or
/// the working directory the install ran from (the footgun of bare `app add
/// --source`). The recorded source is the absolute path inside the home.
pub fn run_install(path: &str) -> Result<(), String> {
    let src = Path::new(path);
    let manifest = terrane_core::cap::host::read_manifest(src).map_err(|e| e.to_string())?;
    let id = manifest.id.trim().to_string();
    if id.is_empty() {
        return Err(format!("{path}/manifest.json has no \"id\""));
    }
    let name = match manifest.name.trim() {
        "" => id.clone(),
        name => name.to_string(),
    };

    let dest = home_dir().join("apps").join(&id);
    // Copy the bundle in, unless it's already sitting at the destination.
    let in_place = matches!(
        (src.canonicalize(), dest.canonicalize()),
        (Ok(s), Ok(d)) if s == d
    );
    if !in_place {
        copy_dir(src, &dest).map_err(|e| format!("copy bundle into home: {e}"))?;
    }
    let dest_abs = dest
        .canonicalize()
        .map_err(|e| format!("resolve {}: {e}", dest.display()))?;
    let source = dest_abs
        .to_str()
        .ok_or("install path is not valid UTF-8")?
        .to_string();

    let mut core = open()?;
    ensure_identity(&mut core)?;
    match core.dispatch(Request::new(
        "app.add",
        vec![id.clone(), name, "--source".into(), source.clone()],
    )) {
        Ok(_) => {
            println!("installed {id} → {source}");
            Ok(())
        }
        // The bundle files were refreshed; the catalog already points somewhere.
        Err(Error::AppExists(_)) => {
            println!("refreshed {id} (already installed; `terrane app remove {id}` to re-catalog)");
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Recursively copy a bundle directory, replacing any existing destination.
fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Ensure this home has minted its replica identity before it authors anything.
/// Idempotent — `replica.init` is a no-op once the id exists.
pub(crate) fn ensure_identity(core: &mut Core<EdgeRunner>) -> Result<(), String> {
    if core.state().replica.peer.is_none() {
        core.dispatch(Request::new("replica.init", Vec::new()))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `sync <app> --from <home>`: pull another replica's edits for one app and merge
/// them into this one. Reads the other home's log read-only, exports the delta
/// this home is missing, and folds it in as a `crdt.update`. Conflict-free and
/// idempotent — re-running once converged is a no-op.
pub fn run_sync(app: &str, from_home: &str) -> Result<(), String> {
    let src_log = PathBuf::from(from_home).join("log.bin");
    let source = Core::open(&src_log).map_err(|e| format!("open --from {from_home}: {e}"))?;

    let mut local = open()?;
    ensure_identity(&mut local)?;

    let hex = terrane_core::cap::crdt::crdt_export_hex(source.state(), app, local.state())
        .map_err(|e| e.to_string())?;
    let Some(hex) = hex else {
        println!("(nothing to sync: {from_home} has no '{app}' data)");
        return Ok(());
    };

    let records = local
        .dispatch(Request::new("crdt.merge", vec![app.to_string(), hex]))
        .map_err(|e| e.to_string())?;
    if records.is_empty() {
        println!("(already up to date with {from_home})");
    } else {
        println!("synced '{app}' from {from_home}");
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

/// The public API surface (the Rust-introspectable core of `public-contract.json`),
/// assembled from the `terrane-api` and `terrane-core` declarations — so it can't
/// drift from the running system.
pub fn contract_surface() -> terrane_api::PublicSurface {
    let resources = terrane_core::resource_surface()
        .into_iter()
        .map(|ns| terrane_api::ResourceNamespace {
            namespace: ns.namespace.to_string(),
            methods: ns
                .methods
                .into_iter()
                .map(|m| terrane_api::ResourceMethodInfo {
                    name: m.name.to_string(),
                    kind: m.kind.to_string(),
                    params: m.params.iter().map(|p| p.to_string()).collect(),
                })
                .collect(),
        })
        .collect();
    let capabilities = terrane_core::capability_namespaces()
        .into_iter()
        .map(str::to_string)
        .collect();
    terrane_api::public_surface(capabilities, resources)
}

/// `contract export` — print [`contract_surface`] as JSON. The
/// `tools/export-public-contract.mjs` wrapper adds provenance/hashes/conformance.
pub fn run_contract_export() -> Result<(), String> {
    use nanoserde::SerJson;
    println!("{}", contract_surface().serialize_json());
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

/// The home directory: `$TERRANE_HOME` (default `./.terrane/`). Holds the event
/// log and the installed app bundles (`apps/<id>/`).
pub fn home_dir() -> PathBuf {
    env::var("TERRANE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".terrane"))
}

/// The on-disk log path: `$TERRANE_HOME/log.bin`.
pub fn log_path() -> PathBuf {
    home_dir().join("log.bin")
}

pub fn print_help() {
    println!(
        "terrane — your local app catalog\n\n\
         Commands are <namespace> <verb> [args…], routed to the capability that\n\
         owns the namespace. Built-in capabilities:\n\n\
         \x20 terrane app install <path>                       copy a bundle into the home & catalog it\n\
         \x20 terrane app add <id> <name…> [--source <path>]   catalog an app by path (dev)\n\
         \x20 terrane app remove <id>                          remove an app\n\
         \x20 terrane kv set <app> <key> <value…>              store a value\n\
         \x20 terrane kv rm <app> <key>                        delete a value\n\
         \x20 terrane net fetch <app> <url>                    GET a url; record it\n\
         \x20 terrane model ask <app> <claude|codex> <prompt…> ask an agent; record it\n\
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
