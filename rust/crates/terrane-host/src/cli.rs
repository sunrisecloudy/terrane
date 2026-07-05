//! Command-line adapter for Terrane.
//!
//! This crate turns `terrane <ns> <verb> [args…]` into calls against
//! the host spine, then formats the result for humans.
//!
//! Catalog lives at `$TERRANE_HOME/log.bin` (default `./.terrane/`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

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
        ["open", target] => run_open(target),
        ["migrate", "status", app] => run_migrate_status(app),
        ["migrate", app] => run_migrate(app),
        ["migrate-log"] => run_migrate_log(),
        ["cap", "list", rest @ ..] => run_cap_list(rest),
        ["cap", "info", namespace, rest @ ..] => run_cap_info(namespace, rest),
        ["cap", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane cap (list | info <namespace>) [--format json|markdown|skill] [--include-internal]".into())
        }
        ["app", "install", path] => run_install(path),
        ["app", "install-kv", path, rest @ ..] => run_install_kv(path, rest),
        ["app", "upgrade", app, rest @ ..] => run_app_upgrade(app, rest),
        ["app", "build", dir] => run_app_build(dir),
        ["app", "remove", app] => run_app_remove(app),
        ["logs", app, rest @ ..] => run_logs(app, rest),
        ["logs", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane logs <app> [--level warn] [--tail 200] [--follow]".into())
        }
        ["contract", "export"] => run_contract_export(),
        ["connection", "set", name, rest @ ..] => run_connection_set(name, rest),
        ["connection", "rm", name] | ["connection", "remove", name] => run_connection_rm(name),
        ["connection", "ls"] | ["connection", "list"] => run_connection_ls(),
        ["connection", "stat", name] => run_connection_stat(name),
        ["connection", "authorize", name] => run_connection_authorize(name),
        ["connection", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane connection (set <name> [--kind apiKey|oauth2|smtp] [--field key] [--config json] | rm <name> | ls | stat <name> | authorize <name>)".into())
        }
        ["mcp", "connect", name, transport_json] => run_mcp_connect(name, transport_json),
        ["mcp", "rm", name] | ["mcp", "disconnect", name] => run_mcp_disconnect(name),
        ["mcp", "ls"] | ["mcp", "list"] => run_mcp_ls(),
        ["mcp", "call", app, connection, tool, args_json] => {
            run_mcp_call(app, connection, tool, args_json)
        }
        ["mcp", "tools", app, connection] => run_mcp_tools(app, connection),
        ["mcp", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane mcp (connect <name> <transport-json> | rm <name> | ls | call <app> <connection> <tool> <args-json> | tools <app> <connection>)".into())
        }
        ["kv", "storage", "set", rest @ ..] => run_kv_storage_set(rest),
        ["kv", "storage", "clear", rest @ ..] => run_kv_storage_clear(rest),
        ["kv", "storage", "status"] => run_kv_storage_status(),
        ["history", app, rest @ ..] => run_history(app, rest),
        ["revert", app, rest @ ..] => run_revert(app, rest),
        ["scheduler", "tick"] => run_scheduler_tick(None),
        ["scheduler", "tick", "--now-ms", now_ms] => run_scheduler_tick(Some(now_ms)),
        ["scheduler", "ls", app] | ["scheduler", "list", app] => run_scheduler_ls(app),
        ["scheduler", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane scheduler (tick [--now-ms <epoch-ms>] | ls <app>)".into())
        }
        ["job", "submit", app, verb, rest @ ..] => run_job_submit(app, verb, rest),
        ["job", "stat", app, job_id] => run_job_stat(app, job_id),
        ["job", "ls", app] | ["job", "list", app] => run_job_ls(app, None),
        ["job", "ls", app, status] | ["job", "list", app, status] => run_job_ls(app, Some(status)),
        ["job", "cancel", app, job_id] => run_job_cancel(app, job_id),
        ["job", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane job (submit <app> <verb> [--job-id id] [--now-ms ms] [--retry json] [--args-json json | args…] | stat <app> <job-id> | ls <app> [status] | cancel <app> <job-id>)".into())
        }
        ["automation", "tick"] => run_automation_tick(None),
        ["automation", "tick", "--now-ms", now_ms] => run_automation_tick(Some(now_ms)),
        ["automation", "ls", app] | ["automation", "list", app] => run_automation_ls(app),
        ["automation", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane automation (tick [--now-ms <epoch-ms>] | ls <app>)".into())
        }
        ["blob", "put", app, name, mime, path] => run_blob_put(app, name, mime, path),
        ["blob", "get", app, name, path] => run_blob_get(app, name, path),
        ["blob", "stat", app, name] => run_blob_stat(app, name),
        ["blob", "ls", app, rest @ ..] => run_blob_ls(app, rest),
        ["blob", "rm", app, name] => run_blob_rm(app, name),
        ["blob", "verify", rest @ ..] => run_blob_verify(rest),
        ["blob", "gc", rest @ ..] => run_blob_gc(rest),
        ["webhook", "register", app, name, verb] => run_webhook_register(app, name, verb),
        ["webhook", "rotate", app, name] => run_webhook_rotate(app, name),
        ["webhook", "unregister", app, name] | ["webhook", "rm", app, name] => {
            run_webhook_unregister(app, name)
        }
        ["webhook", "ls", app] | ["webhook", "list", app] => run_webhook_list(app),
        ["webhook", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane webhook (register <app> <name> <verb> | rotate <app> <name> | unregister <app> <name> | ls <app>)".into())
        }
        ["tts", "speak", app, rest @ ..] => run_tts_speak(app, rest),
        ["tts", "render", app, rest @ ..] => run_tts_render(app, rest),
        ["tts", "voices"] => run_tts_voices(),
        ["tts", "renders", app] => run_tts_renders(app),
        ["tts", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane tts (speak <app> [--voice v] [--rate r] <text…> | render <app> [--voice v] [--rate r] <text…> | voices | renders <app>)".into())
        }
        ["media", "info", app, name] => run_media_info(app, name),
        ["media", "transform", app, source, ops_json, dest] => {
            run_media_transform(app, source, ops_json, dest)
        }
        ["media", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane media (info <app> <name> | transform <app> <source> <ops-json> <dest>)".into())
        }
        ["document", "ls", app] => run_document_ls(app),
        ["document", "get", app, id] => run_document_get(app, id),
        ["document", "rm", app, id] => run_document_rm(app, id),
        ["document", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane document (ls <app> | get <app> <id> | rm <app> <id>)".into())
        }
        ["query", "jmespath", app, source_json, rest @ ..] => {
            run_query_jmespath(app, source_json, rest)
        }
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
        ["stream", "open", app, name, verb, request_json] => {
            run_stream_open(app, name, verb, request_json)
        }
        ["stream", "close", app, name] => run_stream_close(app, name),
        ["stream", "ingest-text", app, name, rest @ ..] => run_stream_ingest_text(app, name, rest),
        ["stream", "reopened", app, name, attempt] => run_stream_reopened(app, name, attempt),
        ["stream", "list", app] => run_stream_list(app),
        ["stream", rest @ ..] => {
            let _ = rest;
            Err("usage: terrane stream (open <app> <name> <verb> <request-json> | close <app> <name> | ingest-text <app> <name> [--received-at ts] <text> | reopened <app> <name> <attempt> | list <app>)".into())
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

pub fn run_query_jmespath(app: &str, source_json: &str, rest: &[&str]) -> Result<(), String> {
    if rest.is_empty() {
        return Err("usage: terrane query jmespath <app> <sourceJson> <expression>".into());
    }
    let mut args = vec![app.to_string(), source_json.to_string(), rest.join(" ")];
    let value = crate::query_on_core(&crate::open()?, "query", "jmespath", &args)?;
    args.clear();
    match value {
        terrane_core::QueryValue::Json(json) => println!("{json}"),
        other => {
            return Err(format!(
                "query.jmespath returned unexpected value: {other:?}"
            ))
        }
    }
    Ok(())
}

pub fn run_install(path: &str) -> Result<(), String> {
    println!("{}", crate::install_app(path)?.message());
    Ok(())
}

pub fn run_app_upgrade(app: &str, rest: &[&str]) -> Result<(), String> {
    if rest.is_empty() {
        return Err("usage: terrane app upgrade <id> <bundle|--to-version v|--from-draft d>".into());
    }
    let mut args = vec![app.to_string()];
    args.extend(rest.iter().map(|part| (*part).to_string()));
    print_command_outcome(crate::dispatch("app.upgrade", &args)?);
    Ok(())
}

fn run_connection_set(name: &str, rest: &[&str]) -> Result<(), String> {
    let mut kind = "apiKey";
    let mut field = "key";
    let mut config = "{}";
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--kind" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--kind requires a value".into());
                };
                kind = value;
                i += 2;
            }
            "--field" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--field requires a value".into());
                };
                field = value;
                i += 2;
            }
            "--config" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--config requires a value".into());
                };
                config = value;
                i += 2;
            }
            other => return Err(format!("unknown connection set option: {other}")),
        }
    }
    let name = terrane_cap_connection::validate_name(name).map_err(|e| e.to_string())?;
    let field = terrane_cap_connection::validate_field(field).map_err(|e| e.to_string())?;
    let secret = prompt_secret("Secret: ")?;
    crate::secret_store::set_secret(&crate::home_dir(), &name, &field, &secret)
        .map_err(|e| e.to_string())?;
    let args = vec![name, kind.to_string(), config.to_string()];
    print_command_outcome(crate::dispatch("connection.define", &args)?);
    Ok(())
}

fn run_connection_rm(name: &str) -> Result<(), String> {
    let name = terrane_cap_connection::validate_name(name).map_err(|e| e.to_string())?;
    let args = vec![name.clone()];
    let outcome = crate::dispatch("connection.remove", &args)?;
    crate::secret_store::remove_connection(&crate::home_dir(), &name).map_err(|e| e.to_string())?;
    print_command_outcome(outcome);
    Ok(())
}

fn run_connection_ls() -> Result<(), String> {
    let core = crate::open()?;
    for status in terrane_cap_connection::all_statuses(core.state()).map_err(|e| e.to_string())? {
        let expiry = status.expires_at.unwrap_or_else(|| "-".to_string());
        println!(
            "{}\t{}\tauthorized={}\texpires={}",
            status.name, status.kind, status.authorized, expiry
        );
    }
    Ok(())
}

fn run_connection_stat(name: &str) -> Result<(), String> {
    let core = crate::open()?;
    match terrane_cap_connection::status(core.state(), name).map_err(|e| e.to_string())? {
        Some(status) => println!(
            "{}",
            serde_json::json!({
                "name": status.name,
                "kind": status.kind,
                "authorized": status.authorized,
                "scopes": status.scopes,
                "expires_at": status.expires_at,
            })
        ),
        None => return Err(format!("unknown connection: {name}")),
    }
    Ok(())
}

fn run_connection_authorize(name: &str) -> Result<(), String> {
    let _ = terrane_cap_connection::validate_name(name).map_err(|e| e.to_string())?;
    Err("OAuth browser authorization is not wired in this host yet; use connection.mark_authorized after a trusted edge exchange".into())
}

fn run_mcp_connect(name: &str, transport_json: &str) -> Result<(), String> {
    let name = terrane_cap_mcp_client::validate_name(name).map_err(|e| e.to_string())?;
    let args = vec![name, transport_json.to_string()];
    print_command_outcome(crate::dispatch("mcp.connect", &args)?);
    Ok(())
}

fn run_mcp_disconnect(name: &str) -> Result<(), String> {
    let name = terrane_cap_mcp_client::validate_name(name).map_err(|e| e.to_string())?;
    let args = vec![name];
    print_command_outcome(crate::dispatch("mcp.disconnect", &args)?);
    Ok(())
}

fn run_mcp_ls() -> Result<(), String> {
    let core = crate::open()?;
    for (name, transport) in &core.state().mcp.connections {
        println!("{name}\t{transport}");
    }
    Ok(())
}

fn run_mcp_call(app: &str, connection: &str, tool: &str, args_json: &str) -> Result<(), String> {
    print_command_outcome(crate::dispatch(
        "mcp.call",
        &[
            app.to_string(),
            connection.to_string(),
            tool.to_string(),
            args_json.to_string(),
        ],
    )?);
    Ok(())
}

fn run_mcp_tools(app: &str, connection: &str) -> Result<(), String> {
    let core = crate::open()?;
    let records = crate::mcp_client::list_tools(
        &crate::home_dir(),
        core.state(),
        app,
        connection,
    )
    .map_err(|e| e.to_string())?;
    let record = records
        .iter()
        .find(|record| record.kind == "mcp.called")
        .ok_or_else(|| "mcp tools produced no response".to_string())?;
    let (_, _, call) = terrane_cap_mcp_client::decode_called(record).map_err(|e| e.to_string())?;
    println!("{}", call.result);
    Ok(())
}

pub fn run_open(target: &str) -> Result<(), String> {
    println!("{}", crate::deep_links::open_target(target)?.message());
    Ok(())
}

/// `terrane app remove <id>` — dispatches the core command and, on success,
/// prunes the app's per-app log buffer directory. The fold already cleared the
/// telemetry state slice; the buffer is a host-edge artifact, deleted here.
pub fn run_app_remove(app: &str) -> Result<(), String> {
    let outcome = crate::dispatch("app.remove", &[app.to_string()])?;
    print_command_outcome(outcome);
    let _ = crate::app_log::delete_app_logs(&crate::home_dir(), app);
    Ok(())
}

/// `terrane logs <app> [--level warn] [--tail 200] [--follow]` — read this
/// app's per-app ring buffer back, newest last. `--follow` (best-effort here;
/// the CLI does not long-poll) currently behaves like a tail of the last 1000.
pub fn run_logs(app: &str, rest: &[&str]) -> Result<(), String> {
    let mut level = String::new();
    let mut tail = 200usize;
    let mut follow = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--level" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--level requires a value".into());
                };
                level = (*value).to_string();
                i += 2;
            }
            "--tail" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--tail requires a value".into());
                };
                tail = value
                    .parse::<usize>()
                    .map_err(|_| format!("--tail must be a non-negative integer, got {value:?}"))?;
                i += 2;
            }
            "--follow" => {
                follow = true;
                i += 1;
            }
            other => return Err(format!("unknown logs option: {other}")),
        }
    }
    if follow {
        tail = tail.max(1000);
    }
    let json = crate::app_log::read_tail(&crate::home_dir(), app, &level, tail)
        .map_err(|e| e.to_string())?;
    println!("{json}");
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
    if state.net.fetches.is_empty() && state.net.requests.is_empty() {
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
    for (app, responses) in &state.net.requests {
        for (request_key, resp) in responses {
            println!(
                "  {app} request {request_key} -> {} {} ({} bytes)",
                resp.status, resp.body_kind, resp.body_size
            );
        }
    }

    println!("browser renders:");
    if state.browser.renders.is_empty() {
        println!("  (none)");
    }
    for (app, renders) in &state.browser.renders {
        for (request_key, render) in renders {
            println!(
                "  {app} render {request_key} -> {} {} {} ({} bytes)",
                render.status, render.output, render.body_kind, render.size
            );
        }
    }

    println!("mcp connections:");
    if state.mcp.connections.is_empty() {
        println!("  (none)");
    }
    for (name, transport) in &state.mcp.connections {
        println!("  {name} {transport}");
    }

    println!("mcp calls:");
    if state.mcp.calls.is_empty() {
        println!("  (none)");
    }
    for (app, calls) in &state.mcp.calls {
        for (call_key, call) in calls {
            println!(
                "  {app} {call_key} {}.{} {} ({} bytes) error={}",
                call.connection,
                call.tool,
                call.result_kind,
                call.result_size,
                call.is_error
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

    println!("documents:");
    if state.document.docs.is_empty() {
        println!("  (none)");
    }
    for (app, docs) in &state.document.docs {
        for doc in docs.values() {
            println!(
                "  {app}/{} — {} ({} bytes)",
                doc.id,
                doc.title,
                doc.body.len()
            );
        }
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

pub fn run_migrate_log() -> Result<(), String> {
    let count = terrane_core::migrate_log(&crate::log_path()).map_err(|e| e.to_string())?;
    println!("migrated {count} events; backup kept at log.bin.pre-actor");
    Ok(())
}

pub fn run_migrate_status(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    let state_version = terrane_cap_migration::version(core.state(), app).map_err(|e| e.to_string())?;
    let bundle = migration_bundle(core.state(), app)?;
    let manifest_version = crate::manifest_data_version(&bundle.manifest);
    let pending: Vec<u64> = bundle
        .manifest
        .migrations
        .iter()
        .filter(|step| step.to > state_version)
        .map(|step| step.to)
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "app": app,
            "stateVersion": state_version,
            "manifestVersion": manifest_version,
            "pending": pending,
        })
    );
    Ok(())
}

pub fn run_migrate(app: &str) -> Result<(), String> {
    let mut core = crate::open()?;
    let bundle = migration_bundle(core.state(), app)?;
    let manifest_version = crate::manifest_data_version(&bundle.manifest);
    let mut current =
        terrane_cap_migration::version(core.state(), app).map_err(|e| e.to_string())?;
    if current > manifest_version {
        return Err(format!(
            "app {app} data version is {current}, but this bundle expects {manifest_version}; restore a newer app bundle or a pre-migration backup"
        ));
    }
    if current == manifest_version {
        println!("{app} already at data version {current}");
        return Ok(());
    }
    while current < manifest_version {
        let next = current + 1;
        let step = bundle
            .manifest
            .migrations
            .iter()
            .find(|step| step.to == next)
            .ok_or_else(|| format!("manifest is missing migration step to {next}"))?;
        let script = bundle.script_source(&step.script)?;
        let outcome = crate::dispatch_on_core(
            &mut core,
            "migration.apply",
            &[app.to_string(), next.to_string(), script],
        )?;
        println!(
            "migrated {app} {current} -> {next} ({} events)",
            outcome.records.len()
        );
        current = terrane_cap_migration::version(core.state(), app).map_err(|e| e.to_string())?;
    }
    Ok(())
}

struct MigrationBundle {
    base: Option<PathBuf>,
    files: Option<std::collections::BTreeMap<String, String>>,
    manifest: crate::BundleManifest,
}

impl MigrationBundle {
    fn script_source(&self, script: &str) -> Result<String, String> {
        if let Some(files) = &self.files {
            return files
                .get(script)
                .cloned()
                .ok_or_else(|| format!("migration script missing from kv bundle: {script}"));
        }
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| "app source is not a bundle directory".to_string())?;
        std::fs::read_to_string(base.join(script))
            .map_err(|e| format!("read migration script {script}: {e}"))
    }
}

fn migration_bundle(state: &terrane_core::State, app: &str) -> Result<MigrationBundle, String> {
    let app_record = state
        .app
        .apps
        .get(app)
        .ok_or_else(|| format!("app not found: {app}"))?;
    let source = app_record
        .source
        .as_deref()
        .ok_or_else(|| format!("app {app} has no --source bundle"))?;
    if let Some(source_app) = terrane_cap_kv::app_bundle_app_id(source) {
        if source_app != app {
            return Err(format!(
                "app {app} points at kv bundle for different app {source_app}"
            ));
        }
        let files = terrane_cap_kv::app_bundle_files(state, app).map_err(|e| e.to_string())?;
        let manifest =
            terrane_cap_js_runtime::read_manifest_from_files(&files).map_err(|e| e.to_string())?;
        return Ok(MigrationBundle {
            base: None,
            files: Some(files),
            manifest: host_manifest_from_runtime(manifest),
        });
    }
    let path = Path::new(source);
    let manifest = crate::read_manifest(path).map_err(|e| e.to_string())?;
    Ok(MigrationBundle {
        base: Some(path.to_path_buf()),
        files: None,
        manifest,
    })
}

fn host_manifest_from_runtime(
    manifest: terrane_cap_js_runtime::BundleManifest,
) -> crate::BundleManifest {
    crate::BundleManifest {
        id: manifest.id,
        name: manifest.name,
        version: crate::default_manifest_version(manifest.version),
        runtime: manifest.runtime,
        backend: manifest.backend,
        module: String::new(),
        entry: String::new(),
        ui: String::new(),
        icon: String::new(),
        resources: manifest.resources,
        interfaces: manifest.interfaces,
        file_types: Vec::new(),
        browser_permissions: Vec::new(),
        data_version: manifest.data_version,
        migrations: manifest
            .migrations
            .into_iter()
            .map(|step| crate::MigrationSpec {
                to: step.to,
                script: step.script,
            })
            .collect(),
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

pub fn run_stream_open(app: &str, name: &str, verb: &str, request_json: &str) -> Result<(), String> {
    let args = vec![
        app.to_string(),
        name.to_string(),
        verb.to_string(),
        request_json.to_string(),
    ];
    print_command_outcome(crate::dispatch("stream.open", &args)?);
    println!("(CLI records desired state only; a long-running host must reconcile sockets)");
    Ok(())
}

pub fn run_stream_close(app: &str, name: &str) -> Result<(), String> {
    let args = vec![app.to_string(), name.to_string()];
    print_command_outcome(crate::dispatch("stream.close", &args)?);
    Ok(())
}

pub fn run_stream_ingest_text(app: &str, name: &str, rest: &[&str]) -> Result<(), String> {
    let mut received_at = None;
    let mut text = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--received-at" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--received-at requires a value".into());
                };
                received_at = Some((*value).to_string());
                i += 2;
            }
            value => {
                text.push(value);
                i += 1;
            }
        }
    }
    if text.is_empty() {
        return Err("usage: terrane stream ingest-text <app> <name> [--received-at ts] <text>".into());
    }
    let received_at = received_at.unwrap_or_else(now_unix_millis);
    let mut core = crate::open()?;
    let delivered = crate::stream_edge::deliver_text(
        &crate::home_dir(),
        &mut core,
        app,
        name,
        &text.join(" "),
        &received_at,
    )?;
    print_command_outcome(crate::CommandOutcome {
        records: delivered.records,
        output: delivered.backend_output,
    });
    Ok(())
}

pub fn run_stream_reopened(app: &str, name: &str, attempt: &str) -> Result<(), String> {
    let attempt = attempt
        .parse::<u64>()
        .map_err(|_| format!("attempt must be a non-negative integer: {attempt}"))?;
    let mut core = crate::open()?;
    print_command_outcome(crate::stream_edge::reopen_on_core(
        &mut core, app, name, attempt,
    )?);
    Ok(())
}

pub fn run_stream_list(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    let mut rows = Vec::new();
    if let Some(streams) = core.state().stream.streams.get(app) {
        for (name, stream) in streams {
            rows.push(serde_json::json!({
                "name": name,
                "kind": stream.kind.as_str(),
                "verb": stream.verb,
                "lastSeq": stream.last_seq,
                "status": if stream.status == terrane_cap_stream::StreamStatus::Open { "open" } else { "closed" },
            }));
        }
    }
    println!("{}", serde_json::to_string(&rows).map_err(|e| e.to_string())?);
    Ok(())
}

fn now_unix_millis() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

pub fn run_tts_speak(app: &str, rest: &[&str]) -> Result<(), String> {
    let parsed = parse_tts_args(rest)?;
    let core = crate::open()?;
    if !core.state().app.apps.contains_key(app) {
        return Err(format!("app not found: {app}"));
    }
    crate::tts_edge::speak(&parsed.text, parsed.voice.as_deref(), parsed.rate_milli)
        .map_err(|e| e.to_string())?;
    println!("ok");
    Ok(())
}

pub fn run_tts_render(app: &str, rest: &[&str]) -> Result<(), String> {
    let parsed = parse_tts_args(rest)?;
    let mut args = vec![app.to_string()];
    if let Some(voice) = parsed.voice {
        args.push("--voice".to_string());
        args.push(voice);
    }
    args.push("--rate".to_string());
    args.push(parsed.rate_milli.to_string());
    args.push(parsed.text);
    print_command_outcome(crate::dispatch("tts.render", &args)?);
    Ok(())
}

pub fn run_tts_voices() -> Result<(), String> {
    println!("{}", crate::tts_edge::voices_json().map_err(|e| e.to_string())?);
    Ok(())
}

pub fn run_tts_renders(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    let Some(renders) = core.state().tts.renders.get(app) else {
        println!("[]");
        return Ok(());
    };
    let json = serde_json::to_string(
        &renders
            .values()
            .map(|render| {
                serde_json::json!({
                    "textHash": render.text_hash,
                    "voice": render.voice,
                    "rateMilli": render.rate_milli,
                    "blobHash": render.blob_hash,
                    "size": render.size,
                    "mime": render.mime,
                    "durationMs": render.duration_ms,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| format!("encode tts renders: {e}"))?;
    println!("{json}");
    Ok(())
}

struct ParsedTtsArgs {
    voice: Option<String>,
    rate_milli: u32,
    text: String,
}

fn parse_tts_args(rest: &[&str]) -> Result<ParsedTtsArgs, String> {
    let mut voice = None;
    let mut rate_milli = terrane_cap_tts::DEFAULT_RATE_MILLI;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--voice" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--voice requires a value".into());
                };
                validate_tts_voice(value)?;
                voice = Some((*value).to_string());
                i += 2;
            }
            "--rate" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--rate requires a value".into());
                };
                rate_milli = parse_tts_rate(value)?;
                i += 2;
            }
            _ => break,
        }
    }
    if i >= rest.len() {
        return Err("tts text must not be empty".into());
    }
    let text = rest[i..].join(" ");
    if text.trim().is_empty() {
        return Err("tts text must not be empty".into());
    }
    if text.len() > terrane_cap_tts::MAX_TEXT_BYTES {
        return Err(format!(
            "tts text exceeds {} bytes",
            terrane_cap_tts::MAX_TEXT_BYTES
        ));
    }
    Ok(ParsedTtsArgs {
        voice,
        rate_milli,
        text,
    })
}

fn validate_tts_voice(raw: &str) -> Result<(), String> {
    let valid = !raw.trim().is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "voice must be a non-empty token using [A-Za-z0-9_.:-], got {raw:?}"
        ))
    }
}

fn parse_tts_rate(raw: &str) -> Result<u32, String> {
    let value = if let Some((whole, frac)) = raw.split_once('.') {
        if whole.is_empty() || frac.is_empty() || frac.len() > 3 {
            return Err(format!(
                "rate must be 500-2000 milli or 0.5-2.0, got {raw:?}"
            ));
        }
        let whole = whole
            .parse::<u32>()
            .map_err(|_| format!("rate must be 500-2000 milli or 0.5-2.0, got {raw:?}"))?;
        let frac_len = frac.len();
        let frac = frac
            .parse::<u32>()
            .map_err(|_| format!("rate must be 500-2000 milli or 0.5-2.0, got {raw:?}"))?;
        let scale = 10u32.pow(u32::try_from(frac_len).unwrap_or(3));
        whole * 1000 + (frac * 1000) / scale
    } else {
        raw.parse::<u32>()
            .map_err(|_| format!("rate must be 500-2000 milli or 0.5-2.0, got {raw:?}"))?
    };
    if !(terrane_cap_tts::MIN_RATE_MILLI..=terrane_cap_tts::MAX_RATE_MILLI).contains(&value) {
        return Err(format!(
            "rate_milli must be {}-{}, got {value}",
            terrane_cap_tts::MIN_RATE_MILLI,
            terrane_cap_tts::MAX_RATE_MILLI
        ));
    }
    Ok(value)
}

pub fn run_kv_storage_status() -> Result<(), String> {
    let core = crate::open()?;
    print_kv_storage_plan(core.kv_storage_plan());
    Ok(())
}

pub fn run_scheduler_tick(now_ms: Option<&str>) -> Result<(), String> {
    let mut core = crate::open()?;
    crate::ensure_identity(&mut core)?;
    let parsed_now = match now_ms {
        Some(raw) => {
            Some(raw
                .parse::<u64>()
                .map_err(|_| format!("--now-ms must be an unsigned integer, got {raw:?}"))?)
        }
        None => None,
    };
    let outcomes = match parsed_now {
        Some(now) => crate::scheduler::run_due_at(&mut core, now)?,
        None => crate::scheduler::run_due(&mut core)?,
    };
    let job_outcomes = match parsed_now {
        Some(now) => crate::job::run_due_at(&mut core, now)?,
        None => crate::job::run_due(&mut core)?,
    };
    if outcomes.is_empty() && job_outcomes.is_empty() {
        println!("scheduler tick: no due schedules");
    } else {
        for outcome in outcomes {
            let status = if outcome.error.is_some() {
                "run_failed"
            } else {
                "ran"
            };
            println!(
                "{} {}/{} scheduled_for={} skipped={} verb={}",
                status,
                outcome.app,
                outcome.name,
                outcome.scheduled_for,
                outcome.skipped,
                outcome.verb
            );
        }
        for outcome in job_outcomes {
            let status = if outcome.error.is_some() {
                "job_failed"
            } else {
                "job_completed"
            };
            println!(
                "{} {}/{} attempt={} verb={}",
                status, outcome.app, outcome.job_id, outcome.attempt, outcome.verb
            );
        }
    }
    Ok(())
}

pub fn run_job_submit(app: &str, verb: &str, rest: &[&str]) -> Result<(), String> {
    let mut job_id = None::<String>;
    let mut retry_json = String::new();
    let mut now_ms = None::<u64>;
    let mut args_json = None::<String>;
    let mut tail = Vec::<String>::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--job-id" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--job-id requires a value".into());
                };
                job_id = Some((*value).to_string());
                i += 2;
            }
            "--retry" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--retry requires a value".into());
                };
                retry_json = (*value).to_string();
                i += 2;
            }
            "--now-ms" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--now-ms requires a value".into());
                };
                now_ms = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("--now-ms must be an unsigned integer, got {value:?}"))?,
                );
                i += 2;
            }
            "--args-json" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--args-json requires a value".into());
                };
                args_json = Some((*value).to_string());
                i += 2;
            }
            other => {
                tail.push(other.to_string());
                i += 1;
            }
        }
    }
    let args_json = match args_json {
        Some(value) => value,
        None => serde_json::to_string(&tail).map_err(|e| e.to_string())?,
    };
    let submitted_at = match now_ms {
        Some(value) => value,
        None => crate::job::now_epoch_ms()?,
    };
    let mut core = crate::open()?;
    crate::ensure_identity(&mut core)?;
    let id = crate::job::submit(
        &mut core,
        app,
        verb,
        &args_json,
        &retry_json,
        submitted_at,
        job_id.as_deref(),
    )?;
    println!("{id}");
    Ok(())
}

pub fn run_job_stat(app: &str, job_id: &str) -> Result<(), String> {
    let core = crate::open()?;
    match core
        .state()
        .job
        .jobs
        .get(app)
        .and_then(|jobs| jobs.get(job_id))
    {
        Some(job) => println!("{}", job_state_json(job)),
        None => println!("null"),
    }
    Ok(())
}

pub fn run_job_ls(app: &str, status: Option<&str>) -> Result<(), String> {
    let core = crate::open()?;
    let jobs = core.state().job.jobs.get(app).cloned().unwrap_or_default();
    for (job_id, job) in jobs {
        if status.is_some_and(|want| want != job.status.as_str()) {
            continue;
        }
        println!(
            "{} status={} attempt={} verb={} next_attempt_at={}",
            job_id,
            job.status.as_str(),
            job.attempt,
            job.verb,
            job.next_attempt_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    Ok(())
}

pub fn run_job_cancel(app: &str, job_id: &str) -> Result<(), String> {
    let at = crate::job::now_epoch_ms()?;
    print_command_outcome(crate::dispatch(
        "job.cancel",
        &[app.to_string(), job_id.to_string(), at.to_string()],
    )?);
    Ok(())
}

fn job_state_json(job: &terrane_cap_job_queue::Job) -> String {
    serde_json::json!({
        "app": job.app,
        "job_id": job.job_id,
        "verb": job.verb,
        "status": job.status.as_str(),
        "attempt": job.attempt,
        "progress_pct": job.progress_pct,
        "submitted_at": job.submitted_at,
        "started_at": job.started_at,
        "finished_at": job.finished_at,
        "cancelled_at": job.cancelled_at,
        "next_attempt_at": job.next_attempt_at,
        "lease_until": job.lease_until,
        "last_error": job.last_error,
        "output": job.output,
    })
    .to_string()
}

pub fn run_scheduler_ls(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    let schedules = core
        .state()
        .scheduler
        .schedules
        .get(app)
        .cloned()
        .unwrap_or_default();
    for (name, schedule) in schedules {
        println!(
            "{} last_scheduled_for={} last_fired_at={} skipped_total={} spec={}",
            name,
            schedule
                .last_scheduled_for
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            schedule
                .last_fired_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            schedule.skipped_total,
            schedule.spec.spec_json
        );
    }
    Ok(())
}

pub fn run_automation_tick(now_ms: Option<&str>) -> Result<(), String> {
    let mut core = crate::open()?;
    crate::ensure_identity(&mut core)?;
    let outcome = match now_ms {
        Some(raw) => {
            let now = raw
                .parse::<u64>()
                .map_err(|_| format!("--now-ms must be an unsigned integer, got {raw:?}"))?;
            crate::automation::run_tick_at(&mut core, now)?
        }
        None => crate::automation::run_tick(&mut core)?,
    };
    if outcome.records.is_empty() && outcome.backend_outputs.is_empty() {
        println!("automation tick: no matching rules");
    } else {
        for item in outcome.backend_outputs {
            let status = if item.error.is_some() {
                "run_failed"
            } else {
                "ran"
            };
            println!("{} {}/{} verb={}", status, item.app, item.name, item.verb);
        }
        let suppressed = outcome
            .records
            .iter()
            .filter(|record| record.kind == "automation.suppressed")
            .count();
        if suppressed > 0 {
            println!("suppressed {suppressed} automation fire(s)");
        }
    }
    Ok(())
}

pub fn run_automation_ls(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    let rules = core
        .state()
        .automation
        .rules
        .get(app)
        .cloned()
        .unwrap_or_default();
    for (name, rule) in rules {
        println!(
            "{} last_fired_at={} fire_count={} suppressed_count={} rule={}",
            name,
            rule.last_fired_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            rule.fire_count,
            rule.suppressed_count,
            rule.rule_json
        );
    }
    Ok(())
}

pub fn run_history(app: &str, rest: &[&str]) -> Result<(), String> {
    let core = crate::open()?;
    let args = parse_history_args(app, rest)?;
    let query = if args.at {
        "at"
    } else if args.key.is_some() {
        "key"
    } else {
        "list"
    };
    let value = crate::query_on_core(&core, "history", query, &args.into_query_args())?;
    match value {
        terrane_core::QueryValue::Json(json) => println!("{json}"),
        other => return Err(format!("history.{query} returned unexpected value: {other:?}")),
    }
    Ok(())
}

pub fn run_revert(app: &str, rest: &[&str]) -> Result<(), String> {
    let parsed = parse_revert_args(app, rest)?;
    let mut core = crate::open()?;
    if parsed.yes {
        print_command_outcome(crate::dispatch_on_core(
            &mut core,
            "history.revert",
            &parsed.command_args,
        )?);
    } else {
        let dry = crate::dry_run_on_core(&core, "history.revert", &parsed.command_args)?;
        println!(
            "would append {} event(s); pass --yes to apply",
            dry.records
        );
    }
    Ok(())
}

pub fn run_blob_put(app: &str, name: &str, mime: &str, path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read blob file {path}: {e}"))?;
    let args = vec![
        app.to_string(),
        name.to_string(),
        mime.to_string(),
        B64.encode(bytes),
    ];
    print_command_outcome(crate::dispatch("blob.put", &args)?);
    Ok(())
}

pub fn run_blob_get(app: &str, name: &str, path: &str) -> Result<(), String> {
    let core = crate::open()?;
    let meta = core
        .state()
        .blob
        .blobs
        .get(app)
        .and_then(|names| names.get(name))
        .ok_or_else(|| format!("key not found: {app}/{name}"))?;
    let bytes = crate::blob_store::read_verified(&crate::home_dir(), &meta.hash)
        .map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("write blob file {path}: {e}"))?;
    println!("wrote {path}");
    Ok(())
}

pub fn run_blob_stat(app: &str, name: &str) -> Result<(), String> {
    let core = crate::open()?;
    let meta = core
        .state()
        .blob
        .blobs
        .get(app)
        .and_then(|names| names.get(name))
        .ok_or_else(|| format!("key not found: {app}/{name}"))?;
    println!(
        "{{\"name\":\"{}\",\"hash\":\"{}\",\"size\":{},\"mime\":\"{}\"}}",
        escape_json(name),
        meta.hash,
        meta.size,
        escape_json(&meta.mime)
    );
    Ok(())
}

pub fn run_blob_ls(app: &str, rest: &[&str]) -> Result<(), String> {
    let prefix = match rest {
        [] => "",
        [prefix] => prefix,
        _ => return Err("usage: terrane blob ls <app> [prefix]".into()),
    };
    let core = crate::open()?;
    let Some(names) = core.state().blob.blobs.get(app) else {
        println!("[]");
        return Ok(());
    };
    let mut out = String::from("[");
    let mut first = true;
    for (name, meta) in names {
        if !name.starts_with(prefix) {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"hash\":\"{}\",\"size\":{},\"mime\":\"{}\"}}",
            escape_json(name),
            meta.hash,
            meta.size,
            escape_json(&meta.mime)
        ));
    }
    out.push(']');
    println!("{out}");
    Ok(())
}

pub fn run_blob_rm(app: &str, name: &str) -> Result<(), String> {
    let args = vec![app.to_string(), name.to_string()];
    print_command_outcome(crate::dispatch("blob.rm", &args)?);
    Ok(())
}

pub fn run_webhook_register(app: &str, name: &str, verb: &str) -> Result<(), String> {
    let mut core = crate::open()?;
    let args = vec![app.to_string(), name.to_string(), verb.to_string()];
    crate::dispatch_on_core(&mut core, "webhook.register", &args)?;
    print_webhook_route(&core, app, name)
}

pub fn run_webhook_rotate(app: &str, name: &str) -> Result<(), String> {
    let mut core = crate::open()?;
    let args = vec![app.to_string(), name.to_string()];
    crate::dispatch_on_core(&mut core, "webhook.rotate", &args)?;
    print_webhook_route(&core, app, name)
}

pub fn run_webhook_unregister(app: &str, name: &str) -> Result<(), String> {
    let args = vec![app.to_string(), name.to_string()];
    print_command_outcome(crate::dispatch("webhook.unregister", &args)?);
    Ok(())
}

pub fn run_webhook_list(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    let Some(routes) = core.state().webhook.routes.get(app) else {
        println!("[]");
        return Ok(());
    };
    let mut out = String::from("[");
    let mut first = true;
    for (name, meta) in routes {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"verb\":\"{}\",\"url_path\":\"/hook/{}/{}/{}\"}}",
            escape_json(name),
            escape_json(&meta.verb),
            escape_json(app),
            escape_json(name),
            escape_json(&meta.token)
        ));
    }
    out.push(']');
    println!("{out}");
    Ok(())
}

fn print_webhook_route(core: &terrane_core::Core<crate::EdgeRunner>, app: &str, name: &str) -> Result<(), String> {
    let meta = core
        .state()
        .webhook
        .routes
        .get(app)
        .and_then(|routes| routes.get(name))
        .ok_or_else(|| format!("webhook route not found after commit: {app}/{name}"))?;
    println!(
        "{{\"name\":\"{}\",\"verb\":\"{}\",\"url_path\":\"/hook/{}/{}/{}\",\"note\":\"deliveries arrive only while a listening Terrane web/mac host is running\"}}",
        escape_json(name),
        escape_json(&meta.verb),
        escape_json(app),
        escape_json(name),
        escape_json(&meta.token)
    );
    Ok(())
}

pub fn run_blob_verify(rest: &[&str]) -> Result<(), String> {
    let core = crate::open()?;
    let hashes = blob_hashes_for_args(core.state(), rest)?;
    let health =
        crate::blob_store::verify_hashes(&crate::home_dir(), hashes).map_err(|e| e.to_string())?;
    let mut ok = true;
    for item in health {
        match item {
            crate::blob_store::BlobHealth::Ok { hash, size } => {
                println!("ok {hash} {size} bytes");
            }
            crate::blob_store::BlobHealth::Missing { hash } => {
                ok = false;
                println!("missing {hash}");
            }
            crate::blob_store::BlobHealth::Corrupt { hash, reason } => {
                ok = false;
                println!("corrupt {hash}: {reason}");
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err("blob verify failed".into())
    }
}

pub fn run_blob_gc(rest: &[&str]) -> Result<(), String> {
    let dry_run = match rest {
        [] | ["--dry-run"] => true,
        ["--yes"] => false,
        _ => return Err("usage: terrane blob gc [--dry-run|--yes]".into()),
    };
    let core = crate::open()?;
    let live: BTreeSet<String> = core
        .state()
        .blob
        .refs
        .iter()
        .filter_map(|(hash, count)| (*count > 0).then_some(hash.clone()))
        .collect();
    let plan =
        crate::blob_store::gc(&crate::home_dir(), &live, dry_run).map_err(|e| e.to_string())?;
    if dry_run {
        println!("would delete {} blob rows", plan.stale_hashes.len());
    } else {
        println!("deleted {} blob rows", plan.deleted);
    }
    for hash in plan.stale_hashes {
        println!("{hash}");
    }
    Ok(())
}

pub fn run_media_info(app: &str, name: &str) -> Result<(), String> {
    let core = crate::open()?;
    let meta = core
        .state()
        .blob
        .blobs
        .get(app)
        .and_then(|names| names.get(name))
        .ok_or_else(|| format!("key not found: {app}/{name}"))?;
    let bytes = crate::blob_store::read_verified(&crate::home_dir(), &meta.hash)
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        crate::media_edge::info(&bytes, &meta.mime).map_err(|e| e.to_string())?
    );
    Ok(())
}

pub fn run_media_transform(
    app: &str,
    source: &str,
    ops_json: &str,
    dest: &str,
) -> Result<(), String> {
    let args = vec![
        app.to_string(),
        source.to_string(),
        ops_json.to_string(),
        dest.to_string(),
    ];
    print_command_outcome(crate::dispatch("media.transform", &args)?);
    Ok(())
}

pub fn run_document_ls(app: &str) -> Result<(), String> {
    let core = crate::open()?;
    println!(
        "{}",
        terrane_cap_document::document_list_json(core.state(), app).map_err(|e| e.to_string())?
    );
    Ok(())
}

pub fn run_document_get(app: &str, id: &str) -> Result<(), String> {
    let core = crate::open()?;
    match terrane_cap_document::get_document_json(core.state(), app, id)
        .map_err(|e| e.to_string())?
    {
        Some(json) => println!("{json}"),
        None => println!("null"),
    }
    Ok(())
}

pub fn run_document_rm(app: &str, id: &str) -> Result<(), String> {
    let args = vec![app.to_string(), id.to_string()];
    print_command_outcome(crate::dispatch("document.delete", &args)?);
    Ok(())
}

fn blob_hashes_for_args(state: &terrane_core::State, rest: &[&str]) -> Result<Vec<String>, String> {
    match rest {
        [] => Ok(state
            .blob
            .refs
            .iter()
            .filter_map(|(hash, count)| (*count > 0).then_some(hash.clone()))
            .collect()),
        [app] => Ok(terrane_cap_blob::live_hashes_for_app(&state.blob, app)),
        [app, name] => state
            .blob
            .blobs
            .get(*app)
            .and_then(|names| names.get(*name))
            .map(|meta| vec![meta.hash.clone()])
            .ok_or_else(|| format!("key not found: {app}/{name}")),
        _ => Err("usage: terrane blob verify [app [name]]".into()),
    }
}

fn escape_json(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
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

struct HistoryArgs {
    app: String,
    key: Option<String>,
    at_seq: Option<String>,
    filter: Option<String>,
    before: Option<String>,
    limit: Option<String>,
    at: bool,
}

impl HistoryArgs {
    fn into_query_args(self) -> Vec<String> {
        if self.at {
            return vec![
                self.app,
                self.key.unwrap_or_default(),
                self.at_seq.unwrap_or_default(),
            ];
        }
        if let Some(key) = self.key {
            return vec![self.app, key, self.limit.unwrap_or_default()];
        }
        vec![
            self.app,
            self.filter.unwrap_or_default(),
            self.before.unwrap_or_default(),
            self.limit.unwrap_or_default(),
        ]
    }
}

fn parse_history_args(app: &str, rest: &[&str]) -> Result<HistoryArgs, String> {
    let mut args = HistoryArgs {
        app: app.to_string(),
        key: None,
        at_seq: None,
        filter: None,
        before: None,
        limit: None,
        at: false,
    };
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--key" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--key requires a value".into());
                };
                args.key = Some((*value).to_string());
                i += 2;
            }
            "--at" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--at requires a sequence".into());
                };
                args.at = true;
                args.at_seq = Some((*value).to_string());
                i += 2;
            }
            "--filter" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--filter requires a value".into());
                };
                args.filter = Some((*value).to_string());
                i += 2;
            }
            "--before" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--before requires a sequence".into());
                };
                args.before = Some((*value).to_string());
                i += 2;
            }
            "--limit" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--limit requires a value".into());
                };
                args.limit = Some((*value).to_string());
                i += 2;
            }
            other => return Err(format!("unknown history option: {other}")),
        }
    }
    if args.at && args.key.is_none() {
        return Err("usage: terrane history <app> --key <key> --at <seq>".into());
    }
    if args.at && (args.filter.is_some() || args.before.is_some() || args.limit.is_some()) {
        return Err("history --at only accepts --key and --at".into());
    }
    Ok(args)
}

struct RevertArgs {
    command_args: Vec<String>,
    yes: bool,
}

fn parse_revert_args(app: &str, rest: &[&str]) -> Result<RevertArgs, String> {
    let mut to_seq = None;
    let mut scope = "app".to_string();
    let mut selector = String::new();
    let mut actor = None;
    let mut yes = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--to" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--to requires a sequence".into());
                };
                to_seq = Some((*value).to_string());
                i += 2;
            }
            "--scope" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--scope requires key, prefix, or app".into());
                };
                scope = (*value).to_string();
                i += 2;
            }
            "--key" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--key requires a value".into());
                };
                scope = "key".to_string();
                selector = (*value).to_string();
                i += 2;
            }
            "--prefix" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--prefix requires a value".into());
                };
                scope = "prefix".to_string();
                selector = (*value).to_string();
                i += 2;
            }
            "--actor" => {
                let Some(value) = rest.get(i + 1) else {
                    return Err("--actor requires a value".into());
                };
                actor = Some((*value).to_string());
                i += 2;
            }
            "--yes" => {
                yes = true;
                i += 1;
            }
            "--dry-run" => {
                yes = false;
                i += 1;
            }
            other => return Err(format!("unknown revert option: {other}")),
        }
    }
    let Some(to_seq) = to_seq else {
        return Err("usage: terrane revert <app> --to <seq> [--key k | --prefix p | --scope app] [--actor actor] [--yes]".into());
    };
    if scope == "app" {
        selector.clear();
    }
    let mut command_args = vec![app.to_string(), to_seq, scope, selector];
    if let Some(actor) = actor {
        command_args.push(actor);
    }
    Ok(RevertArgs { command_args, yes })
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
         \x20 terrane app upgrade <id> <bundle|--to-version v|--from-draft d>\n\
         \x20 terrane app build <dir>                          build an app frontend (terrane-app-build) into dist/\n\
         \x20 terrane app add <id> <name…> [--source <path>]   catalog an app by path (dev)\n\
         \x20 terrane app remove <id>                          remove an app (also prunes its log buffer)\n\
         \x20 terrane kv set <app> <key> <value…>              store a value\n\
         \x20 terrane kv rm <app> <key>                        delete a value\n\
         \x20 terrane kv storage set --default <backend> [--path <path>]\n\
         \x20 terrane kv storage set --app <app> <backend> [--path <path>]\n\
         \x20 terrane kv storage clear (--default | --app <app>)\n\
         \x20 terrane kv storage status\n\
         \x20 terrane migrate status <app>                      show app data migration status\n\
         \x20 terrane migrate <app>                             apply pending manifest migrations\n\
         \x20 terrane scheduler tick [--now-ms <epoch-ms>]     fire due schedules and run backend verbs\n\
         \x20 terrane scheduler ls <app>                       list folded scheduler state\n\
         \x20 terrane job submit <app> <verb> [args…]          queue a background backend job\n\
         \x20 terrane job stat|ls|cancel <app> [job-id]        inspect or cancel durable jobs\n\
         \x20 terrane automation tick [--now-ms <epoch-ms>]    fire matching event rules and run backend verbs\n\
         \x20 terrane automation ls <app>                      list folded automation state\n\
         \x20 terrane history <app> [--key k] [--at seq] [--filter f] [--before seq] [--limit n]\n\
         \x20 terrane revert <app> --to <seq> [--key k | --prefix p | --scope app] [--actor actor] [--yes]\n\
         \x20 terrane blob put <app> <name> <mime> <path>     store file bytes in the blob CAS\n\
         \x20 terrane blob get <app> <name> <path>            verify and write blob bytes to a file\n\
         \x20 terrane blob ls <app> [prefix]                  list blob metadata\n\
         \x20 terrane blob rm <app> <name>                    remove a blob name\n\
         \x20 terrane blob verify [app [name]]                verify live blob hashes against the CAS\n\
         \x20 terrane blob gc [--dry-run|--yes]               report or delete unreferenced CAS rows\n\
         \x20 terrane webhook register <app> <name> <verb>    mint a local-network webhook URL\n\
         \x20 terrane webhook rotate|unregister <app> <name>  rotate or remove a webhook URL\n\
         \x20 terrane webhook ls <app>                        list webhook URL paths\n\
         \x20 terrane tts speak <app> [--voice v] [--rate r] <text…>   speak text now; record nothing\n\
         \x20 terrane tts render <app> [--voice v] [--rate r] <text…>  render speech into blob CAS\n\
         \x20 terrane tts voices|renders <app>                list host voices or folded render metadata\n\
         \x20 terrane media info <app> <name>                 probe media metadata for a stored blob\n\
         \x20 terrane media transform <app> <source> <ops-json> <dest>  transform media into a new blob\n\
         \x20 terrane document ls <app>                       list document summaries as JSON\n\
         \x20 terrane document get <app> <id>                 print one document as JSON or null\n\
         \x20 terrane document rm <app> <id>                  delete one document\n\
         \x20 terrane i18n import <path>                    seed the public KV bucket from i18n/system & apps/*/i18n catalogs\n\
         \x20 terrane i18n negotiate <accept-language>       resolve a header to the best supported code\n\
         \x20 terrane native observe-default                    record default host native support\n\
         \x20 terrane native drain-once                         drain one pending native request\n\
         \x20 terrane connection set <name> [--field key]       read a secret from stdin/prompt and record public metadata\n\
         \x20 terrane connection ls|stat|rm                     inspect or remove non-secret connection metadata\n\
         \x20 terrane mcp connect <name> <transport-json>       record an external MCP server connection\n\
         \x20 terrane mcp call <app> <connection> <tool> <args-json>  call an external MCP tool; record the result\n\
         \x20 terrane mcp tools <app> <connection>              list tools from an external MCP server\n\
         \x20 terrane mcp ls|rm                                  inspect or remove MCP connections\n\
         \x20 terrane net fetch <app> <url>                    GET a url; record it\n\
         \x20 terrane net request <app> <request-json>          full HTTP request; record redacted request + response\n\
         \x20 terrane stream open <app> <name> <verb> <request-json>  record desired SSE/WebSocket state\n\
         \x20 terrane stream ingest-text <app> <name> <text…>   record one observed stream message and invoke its verb\n\
         \x20 terrane stream close|list|reopened …              manage folded stream state from the host edge\n\
         \x20 terrane browser render <app> <request-json>       headless render; record redacted request + result\n\
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
         \x20 terrane open <url-or-file>                       route terrane:// links or registered file types to common.receive\n\
         \x20 terrane js-runtime run <app> [input…]            run an app's JS backend\n\
         \x20 terrane wasm-runtime run <app> [input…]          run an app's WASM backend\n\n\
         Multi-user:\n\
         \x20 terrane serve [--addr <addr>]      listen for peers (default 127.0.0.1:7777)\n\
         \x20 terrane sync <app> --from <home>   merge another home's edits (local)\n\
         \x20 terrane sync <app> --peer <addr>   merge a serving peer's edits (network)\n\n\
         Reads & meta:\n\
         \x20 terrane state                  print the whole world\n\
         \x20 terrane log                    print the event log (decoded)\n\
         \x20 terrane logs <app> [--level warn] [--tail 200]  read an app's local backend log buffer\n\
         \x20 terrane replay                 rebuild state from the log and verify it\n\
         \x20 terrane migrate-log            upgrade a pre-actor log and keep log.bin.pre-actor\n\
         \x20 terrane query jmespath <app> <sourceJson> <expression>  read folded state with JMESPath\n\
         \x20 terrane cap list               list capability docs\n\
         \x20 terrane cap info <namespace>   show capability docs\n\
         \x20 terrane contract export        print the public API contract (JSON)\n\
         \x20 terrane help                   this message\n\n\
         Catalog: $TERRANE_HOME/log.bin (binary event log; default ./.terrane/)"
    );
}
