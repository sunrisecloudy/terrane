//! Command-line adapter for Terrane.
//!
//! This crate turns `terrane <ns> <verb> [args…]` into calls against
//! the host spine, then formats the result for humans.
//!
//! Catalog lives at `$TERRANE_HOME/log.bin` (default `./.terrane/`).

use std::path::PathBuf;

pub use crate::{serve_conn, sync_conn, EdgeRunner};

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
        ["cap", "list", rest @ ..] => run_cap_list(rest),
        ["cap", "info", namespace, rest @ ..] => run_cap_info(namespace, rest),
        ["cap", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane cap (list | info <namespace>) [--format json|markdown|skill] [--include-internal]".into())
        }
        ["app", "install", path] => run_install(path),
        ["app", "install-kv", path, rest @ ..] => run_install_kv(path, rest),
        ["app", "build", dir] => run_app_build(dir),
        ["contract", "export"] => run_contract_export(),
        ["kv", "storage", "set", rest @ ..] => run_kv_storage_set(rest),
        ["kv", "storage", "clear", rest @ ..] => run_kv_storage_clear(rest),
        ["kv", "storage", "status"] => run_kv_storage_status(),
        ["i18n", "import", path] => run_i18n_import(path),
        ["i18n", "negotiate", header] => run_i18n_negotiate(header),
        // Host verbs for the local-model edge (runtime + resident server) —
        // machine plumbing, not capability commands: nothing is recorded.
        ["local-model", "setup", "mlx"] => run_local_model_setup_mlx(),
        ["local-model", "server", "status"] => run_local_model_server_status(),
        ["local-model", "server", "stop"] => run_local_model_server_stop(),
        ["local-model", "setup" | "server", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane local-model (setup mlx | server status | server stop)".into())
        }
        ["native", "observe-default"] => run_native_observe_default(),
        ["native", "drain-once"] => run_native_drain_once(),
        // Host-edge verbs for ambient speech-to-text. Capture/ASR run at the
        // edge; these dispatch the trusted `stt.*` commands that record facts.
        ["stt", "open", rest @ ..] => run_stt_dispatch("stt.session.open", rest),
        ["stt", "append", rest @ ..] => run_stt_dispatch("stt.segment.append", rest),
        ["stt", "close", rest @ ..] => run_stt_dispatch("stt.session.close-host", rest),
        ["stt", "trim", rest @ ..] => run_stt_dispatch("stt.retention.trim", rest),
        ["stt", "purge", rest @ ..] => run_stt_dispatch("stt.session.purge", rest),
        ["stt", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane stt (open | append | close | trim | purge) …".into())
        }
        ["serve"] => crate::sync::run_serve(crate::DEFAULT_SERVE_ADDR),
        ["serve", "--addr", addr] => crate::sync::run_serve(addr),
        ["sync", app, "--from", home] => run_sync(app, home),
        ["sync", app, "--peer", addr] => crate::sync::run_sync_peer(app, addr),
        ["sync", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane sync <app> (--from <home> | --peer <addr>)".into())
        }
        // Run an app backend. `--ask` prompts (hidden) for the first verb
        // argument — the password manager's `auth` — so a master password never
        // lands on argv.
        ["run", app, verb, rest @ ..] => run_app_backend(app, verb, rest),
        ["run", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane run <app> <verb> [args… | --ask]".into())
        }
        [ns, verb, rest @ ..] => run_command(ns, verb, rest),
        [other] => Err(format!("unknown command {other:?} (try `terrane help`)")),
    }
}

/// Run an app backend via `js-runtime.run`. A bare `--ask` anywhere in the verb
/// arguments is dropped and replaced by a value read (without echo) from the
/// terminal, spliced in as the FIRST verb argument (the vault app's `auth`). This
/// keeps a master password out of argv, shell history, and the process table.
pub fn run_app_backend(app: &str, verb: &str, rest: &[&str]) -> Result<(), String> {
    let ask = rest.contains(&"--ask");
    let mut args: Vec<String> = Vec::with_capacity(rest.len() + 2);
    args.push(app.to_string());
    args.push(verb.to_string());
    if ask {
        args.push(prompt_secret("Master password: ")?);
    }
    for arg in rest {
        if *arg != "--ask" {
            args.push((*arg).to_string());
        }
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    dispatch("js-runtime.run", &refs)
}

/// Read one secret line. On an interactive terminal it prompts on stderr and
/// reads with echo disabled; when stdin is piped (scripts, tests) it reads the
/// line plainly with no prompt — so the same flag works both ways. rpassword
/// reads the controlling terminal directly, which errors when there is none,
/// hence the explicit `is_terminal()` branch.
pub fn prompt_secret(label: &str) -> Result<String, String> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    if std::io::stdin().is_terminal() {
        eprint!("{label}");
        let _ = std::io::stderr().flush();
        return rpassword::read_password().map_err(|e| format!("could not read secret: {e}"));
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("could not read secret: {e}"))?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Generic write path: `<ns> <verb> [args…]` -> `dispatch("<ns>.<verb>", args)`.
pub fn run_command(ns: &str, verb: &str, rest: &[&str]) -> Result<(), String> {
    dispatch(&format!("{ns}.{verb}"), rest)
}

/// Dispatch a dotted command name + args and print the human CLI result.
pub fn dispatch(command: &str, args: &[&str]) -> Result<(), String> {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    print_command_outcome(crate::dispatch(command, &args)?);
    Ok(())
}

pub fn run_install(path: &str) -> Result<(), String> {
    println!("{}", crate::install_app(path)?.message());
    Ok(())
}

fn run_local_model_setup_mlx() -> Result<(), String> {
    let summary = crate::local_llm::setup_mlx(&crate::home_dir()).map_err(|e| e.to_string())?;
    println!("{summary}");
    Ok(())
}

fn run_local_model_server_status() -> Result<(), String> {
    println!(
        "{}",
        crate::local_llm::mlx_server_status_json(&crate::home_dir())
    );
    Ok(())
}

fn run_local_model_server_stop() -> Result<(), String> {
    let message =
        crate::local_llm::mlx_server_stop(&crate::home_dir()).map_err(|e| e.to_string())?;
    println!("{message}");
    Ok(())
}

pub fn run_install_kv(path: &str, rest: &[&str]) -> Result<(), String> {
    let (storage_backend, storage_path) = parse_install_kv_options(rest)?;
    println!(
        "{}",
        crate::install_app_to_kv(path, storage_backend, storage_path)?.message()
    );
    Ok(())
}

pub fn run_sync(app: &str, from_home: &str) -> Result<(), String> {
    println!("{}", crate::sync_from_home(app, from_home)?.message());
    Ok(())
}

pub fn run_cap_list(rest: &[&str]) -> Result<(), String> {
    let options = parse_cap_options(rest, "markdown")?;
    match options.format.as_str() {
        "json" => println!(
            "{}",
            crate::cap_doc::capability_list_json(options.include_internal)
        ),
        "markdown" => println!(
            "{}",
            crate::cap_doc::capability_list_markdown(options.include_internal)
        ),
        other => {
            return Err(format!(
                "unknown cap list format: {other} (expected json or markdown)"
            ))
        }
    }
    Ok(())
}

pub fn run_cap_info(namespace: &str, rest: &[&str]) -> Result<(), String> {
    let options = parse_cap_options(rest, "markdown")?;
    println!(
        "{}",
        crate::cap_doc::render_capability_info(
            namespace,
            &options.format,
            options.include_internal
        )?
    );
    Ok(())
}

/// Read the whole world (no network).
pub fn run_state() -> Result<(), String> {
    let core = crate::open()?;
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

    println!("kv storage:");
    print_kv_storage_plan(core.kv_storage_plan());

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

    println!("local models:");
    if state.local_model.specs.is_empty() && state.local_model.turns.is_empty() {
        println!("  (none)");
    }
    for (id, spec) in &state.local_model.specs {
        let default_marker = if state.local_model.default_model.as_deref() == Some(id.as_str()) {
            " [default]"
        } else {
            ""
        };
        println!(
            "  {id} ({}/{}) at {}{default_marker}",
            spec.backend, spec.format, spec.local_path
        );
    }
    for (app, turns) in &state.local_model.turns {
        for turn in turns {
            println!(
                "  {app} [{}] {} {} tokens ({} chars)",
                turn.model,
                if turn.ok { "ok" } else { "failed" },
                turn.token_count,
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

    println!("harness:");
    if state.harness.runs.is_empty() {
        println!("  (none)");
    }
    for run in state.harness.runs.values() {
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
    let core = crate::open()?;
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
    crate::contract_surface()
}

/// `contract export` — print [`contract_surface`] as JSON.
pub fn run_contract_export() -> Result<(), String> {
    use nanoserde::SerJson;
    println!("{}", contract_surface().serialize_json());
    Ok(())
}

pub fn run_replay() -> Result<(), String> {
    let core = crate::open()?;
    if core.replay_matches().map_err(|e| e.to_string())? {
        println!("replay ok: state reproduced identically from the log");
        Ok(())
    } else {
        Err("replay mismatch: log does not reproduce current state".into())
    }
}

pub fn run_kv_storage_set(rest: &[&str]) -> Result<(), String> {
    let args = parse_kv_storage_set(rest)?;
    print_command_outcome(crate::dispatch("kv.storage.set", &args)?);
    Ok(())
}

pub fn run_kv_storage_clear(rest: &[&str]) -> Result<(), String> {
    let args = parse_kv_storage_clear(rest)?;
    print_command_outcome(crate::dispatch("kv.storage.clear", &args)?);
    Ok(())
}

/// Dispatch a trusted `stt.*` host-edge command (open/append/close/trim) with
/// args passed through verbatim. The command name owns the validation.
pub fn run_stt_dispatch(command: &str, rest: &[&str]) -> Result<(), String> {
    let args: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
    print_command_outcome(crate::dispatch(command, &args)?);
    Ok(())
}

pub fn run_kv_storage_status() -> Result<(), String> {
    let core = crate::open()?;
    print_kv_storage_plan(core.kv_storage_plan());
    Ok(())
}

/// `terrane i18n import <path>`: seed the public KV bucket from checked-in
/// catalog files. Idempotent and replay-safe.
pub fn run_i18n_import(path: &str) -> Result<(), String> {
    let root = std::path::Path::new(path);
    let mut core = crate::open()?;
    let outcome = crate::import_i18n_dir(&mut core, root)?;
    println!("{}", outcome.message());
    Ok(())
}

/// `terrane i18n negotiate <header>`: resolve an `Accept-Language` header to
/// the best supported code. Hosts and debug.
pub fn run_i18n_negotiate(header: &str) -> Result<(), String> {
    println!("{}", terrane_i18n::from_accept_language(header));
    Ok(())
}

/// `terrane app build <dir>`: build an app's frontend (terrane-app-build) into
/// its `dist/`. Terminal parity with the `terrane_build_app` C ABI.
pub fn run_app_build(dir: &str) -> Result<(), String> {
    let result = terrane_app_build::build_app(terrane_app_build::BuildOptions {
        app_dir: std::path::PathBuf::from(dir),
        check_only: false,
    })?;
    println!(
        "built {} files -> {}",
        result.files.len(),
        result.dist.display()
    );
    Ok(())
}

pub fn run_native_observe_default() -> Result<(), String> {
    let mut core = crate::open()?;
    let connector = crate::native::default_connector();
    print_command_outcome(crate::native::observe_connector_on_core(
        &mut core, &connector,
    )?);
    Ok(())
}

pub fn run_native_drain_once() -> Result<(), String> {
    let mut core = crate::open()?;
    let connector = crate::native::default_connector();
    match crate::native::drain_once_on_core(&mut core, &connector)? {
        crate::native::NativeDrainOutcome::Idle => println!("native drain idle"),
        crate::native::NativeDrainOutcome::Drained(drained) => println!(
            "native drained {}/{} {}",
            drained.app, drained.request_id, drained.operation_id
        ),
    }
    Ok(())
}

fn parse_kv_storage_set(rest: &[&str]) -> Result<Vec<String>, String> {
    match rest {
        ["--default", backend, tail @ ..] | ["default", backend, tail @ ..] => {
            let mut args = vec!["default".to_string(), (*backend).to_string()];
            if let Some(path) = parse_optional_storage_path(tail)? {
                args.push(path);
            }
            Ok(args)
        }
        ["--app", app, backend, tail @ ..] | ["app", app, backend, tail @ ..] => {
            let mut args = vec![
                "app".to_string(),
                (*app).to_string(),
                (*backend).to_string(),
            ];
            if let Some(path) = parse_optional_storage_path(tail)? {
                args.push(path);
            }
            Ok(args)
        }
        _ => Err(
            "usage: terrane kv storage set (--default <memory|sqlite|rocksdb> | --app <app> <memory|sqlite|rocksdb>) [--path <path>]".into(),
        ),
    }
}

fn parse_kv_storage_clear(rest: &[&str]) -> Result<Vec<String>, String> {
    match rest {
        ["--default"] | ["default"] => Ok(vec!["default".to_string()]),
        ["--app", app] | ["app", app] => Ok(vec!["app".to_string(), (*app).to_string()]),
        _ => Err("usage: terrane kv storage clear (--default | --app <app>)".into()),
    }
}

fn parse_optional_storage_path(tail: &[&str]) -> Result<Option<String>, String> {
    match tail {
        [] => Ok(None),
        ["--path", path] => Ok(Some((*path).to_string())),
        [path] => Ok(Some((*path).to_string())),
        _ => Err("storage path must be passed as [--path <path>]".into()),
    }
}

fn parse_install_kv_options(rest: &[&str]) -> Result<(Option<String>, Option<String>), String> {
    let mut storage_backend = None;
    let mut storage_path = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--storage" | "--backend" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--storage requires a backend".into());
                };
                storage_backend = Some((*value).to_string());
                i += 2;
            }
            "--path" | "--storage-path" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--path requires a storage path".into());
                };
                storage_path = Some((*value).to_string());
                i += 2;
            }
            other => return Err(format!("unknown app install-kv option: {other}")),
        }
    }
    Ok((storage_backend, storage_path))
}

fn print_kv_storage_plan(plan: &terrane_cap_kv::KvStoragePlan) {
    println!("  default -> {}", plan.default.describe());
    if plan.apps.is_empty() {
        println!("  app overrides: (none)");
        return;
    }
    println!("  app overrides:");
    for (app, binding) in &plan.apps {
        println!("    {app} -> {}", binding.describe());
    }
}

/// Open the workspace core at `$TERRANE_HOME/log.bin` with the real edge runner.
pub fn open() -> Result<crate::HostCore, String> {
    crate::open()
}

/// The home directory: `$TERRANE_HOME` (default `./.terrane/`).
pub fn home_dir() -> PathBuf {
    crate::home_dir()
}

/// The on-disk log path: `$TERRANE_HOME/log.bin`.
pub fn log_path() -> PathBuf {
    crate::log_path()
}

fn print_command_outcome(outcome: crate::CommandOutcome) {
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

struct CapOptions {
    format: String,
    include_internal: bool,
}

fn parse_cap_options(rest: &[&str], default_format: &str) -> Result<CapOptions, String> {
    let mut format = default_format.to_string();
    let mut include_internal = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--format" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--format requires a value".into());
                };
                format = (*value).to_string();
                i += 2;
            }
            "--include-internal" => {
                include_internal = true;
                i += 1;
            }
            other => return Err(format!("unknown cap option: {other}")),
        }
    }
    Ok(CapOptions {
        format,
        include_internal,
    })
}

pub fn print_help() {
    println!(
        "terrane — your local app catalog\n\n\
         Commands are <namespace> <verb> [args…], routed to the capability that\n\
         owns that namespace. Built-in capabilities:\n\n\
         \x20 terrane app install <path>                       copy a bundle into the home & catalog it\n\
         \x20 terrane app install-kv <path> [--storage <backend>] [--path <path>]\n\
         \x20                                                  store a JS bundle in reserved cap-kv keys\n\
         \x20 terrane app build <dir>                          build an app frontend (terrane-app-build) into dist/\n\
         \x20 terrane app add <id> <name…> [--source <path>]   catalog an app by path (dev)\n\
         \x20 terrane app remove <id>                          remove an app\n\
         \x20 terrane kv set <app> <key> <value…>              store a value\n\
         \x20 terrane kv rm <app> <key>                        delete a value\n\
         \x20 terrane kv storage set --default <backend> [--path <path>]\n\
         \x20 terrane kv storage set --app <app> <backend> [--path <path>]\n\
         \x20 terrane kv storage clear (--default | --app <app>)\n\
         \x20 terrane kv storage status\n\
         \x20 terrane i18n import <path>                    seed the public KV bucket from i18n/system & apps/*/i18n catalogs\n\
         \x20 terrane i18n negotiate <accept-language>       resolve a header to the best supported code\n\
         \x20 terrane native observe-default                    record default host native support\n\
         \x20 terrane native drain-once                         drain one pending native request\n\
         \x20 terrane net fetch <app> <url>                    GET a url; record it\n\
         \x20 terrane model ask <app> <claude|codex> <prompt…> ask an agent; record it\n\
         \x20 terrane local-model pull [<id> <hf-repo> [<file>]] [--backend gguf|mlx] [options…]  fetch + register (bare = recommended model)\n\
         \x20 terrane local-model register <id> <llama_cpp|mlx> <path-or-repo> [--context N] [--template T] [--max-tokens N] [--temp F]\n\
         \x20 terrane local-model ask <app> [--model <id>] [--system <text>] [--continue] [--schema <json>|--grammar <gbnf>] <prompt…>  local inference\n\
         \x20 terrane local-model default <id>   choose the model asks use when --model is omitted\n\
         \x20 terrane local-model rm <id>        unregister a local model spec\n\
         \x20 terrane local-model setup mlx      install the Apple-Silicon MLX runtime (pinned, self-contained)\n\
         \x20 terrane local-model server status|stop   inspect or stop the resident mlx server\n\
         \x20 terrane harness generate-app [--harness <codex|claude-code|opencode>] <draft> <app> <name> <prompt…>\n\
         \x20 terrane harness run-js [--harness <codex|claude-code|opencode>] <run> <app> <prompt…>\n\
         \x20 terrane run <app> <verb> [--ask] [args…]         run an app backend; --ask prompts (hidden) for the first arg\n\
         \x20 terrane js-runtime run <app> [input…]            run an app's JS backend\n\
         \x20 terrane wasm-runtime run <app> [input…]          run an app's WASM backend\n\n\
         Multi-user:\n\
         \x20 terrane serve [--addr <addr>]      listen for peers (default 127.0.0.1:7777)\n\
         \x20 terrane sync <app> --from <home>   merge another home's edits (local)\n\
         \x20 terrane sync <app> --peer <addr>   merge a serving peer's edits (network)\n\n\
         Reads & meta:\n\
         \x20 terrane state                  print the whole world\n\
         \x20 terrane log                    print the event log (decoded)\n\
         \x20 terrane replay                 rebuild state from the log and verify it\n\
         \x20 terrane cap list               list capability docs\n\
         \x20 terrane cap info <namespace>   show capability docs\n\
         \x20 terrane contract export        print the public API contract (JSON)\n\
         \x20 terrane help                   this message\n\n\
         Catalog: $TERRANE_HOME/log.bin (binary event log; default ./.terrane/)"
    );
}
