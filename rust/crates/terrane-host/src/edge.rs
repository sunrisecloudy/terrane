//! The CLI's real [`EffectRunner`] — where the engine's effects meet the world.
//!
//! It performs each [`Effect`] at the edge and hands the result back as the
//! owning capability's recorded event. Replay never calls this. Effects so far:
//! a minimal `http://` GET (`net`), an agent-CLI call (`model`), and minting this
//! home's replica id from OS entropy (`replica`).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use terrane_cap_builder as builder;
use terrane_cap_harness as harness;
use terrane_cap_js_runtime::{run_js_bundle, JsRuntimeBundle};
use terrane_cap_kv::{
    app_bundle_key, app_bundle_source, set_event, storage_configured_event, KvStorageBackend,
};
use terrane_cap_model::responded_event;
use terrane_cap_net::fetched_event;
use terrane_cap_replica::initialized_event;
use terrane_core::{Effect, EffectRunner};
use terrane_core::{Error, EventRecord, Result};
use terrane_core::{ExecutionPrincipal, RuntimeHostHandle, RuntimeResourceHost};

pub struct EdgeRunner;

const DEFAULT_EDGE_TIMEOUT: Duration = Duration::from_secs(30);

impl EffectRunner for EdgeRunner {
    fn run(&self, effect: &Effect, state: &terrane_core::State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                let (status, body) = http_get(url)?;
                Ok(vec![fetched_event(app, url, status, body)?])
            }
            Effect::ModelCall { app, agent, prompt } => {
                let (response, exit_code) = run_agent(agent, prompt)?;
                Ok(vec![responded_event(
                    app, agent, prompt, response, exit_code,
                )?])
            }
            Effect::GenerateAppWithHarness {
                draft_id,
                app_id,
                name,
                harness,
                prompt,
            } => generate_app_with_harness(draft_id, app_id, name, harness, prompt),
            Effect::RunHarnessJs {
                run_id,
                app_id,
                harness,
                prompt,
            } => run_harness_js(run_id, app_id, harness, prompt, state),
            Effect::ImportAppBundle {
                source,
                storage_backend,
                storage_path,
            } => import_app_bundle(source, storage_backend, storage_path, state),
            Effect::NewReplicaId => Ok(vec![initialized_event(new_peer_id()?)?]),
            Effect::LocalModelCall {
                app,
                model,
                prompt,
                system,
                history,
                schema,
                grammar,
            } => crate::local_llm::call(
                app,
                model,
                prompt,
                system.as_deref(),
                history,
                schema.as_deref(),
                grammar.as_deref(),
                state,
            ),
            Effect::LocalModelPull {
                id,
                repo,
                backend,
                file,
                context_length,
                chat_template,
                max_tokens,
                temperature_milli,
                draft_model,
            } => crate::local_llm::pull(
                id,
                repo,
                backend,
                file.as_deref(),
                *context_length,
                chat_template.clone(),
                *max_tokens,
                *temperature_milli,
                draft_model.clone(),
            ),
        }
    }
}

/// Mint a fresh replica PeerID from OS entropy. Masked to 53 bits and forced
/// nonzero — a valid, JS-safe (`Number`-representable) Loro PeerID.
fn new_peer_id() -> Result<u64> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Storage(format!("failed to read OS entropy for replica id: {e}")))?;
    Ok((u64::from_le_bytes(bytes) & ((1u64 << 53) - 1)) | 1)
}

/// Run an agent CLI non-interactively and capture its output.
/// `claude -p "<prompt>"` (Claude Code print mode) or `codex exec "<prompt>"`.
fn run_agent(agent: &str, prompt: &str) -> Result<(String, i32)> {
    let mut command = match agent {
        "claude" => {
            let mut c = Command::new("claude");
            c.arg("-p").arg(prompt);
            c
        }
        "codex" => {
            let mut c = Command::new("codex");
            c.arg("exec").arg(prompt);
            c
        }
        other => return Err(Error::InvalidInput(format!("unknown agent: {other}"))),
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| {
        Error::Storage(format!(
            "failed to run `{agent}` (is it installed and on PATH?): {e}"
        ))
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Storage(format!("failed to capture `{agent}` stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Storage(format!("failed to capture `{agent}` stderr")))?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let timeout = edge_timeout();
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(Error::Storage(format!(
                    "`{agent}` timed out after {timeout:?}"
                )));
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| Error::Storage(format!("failed to join `{agent}` stdout reader")))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| Error::Storage(format!("failed to join `{agent}` stderr reader")))??;

    let exit_code = status.code().unwrap_or(-1);
    let mut response = String::from_utf8_lossy(&stdout).into_owned();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.trim().is_empty() {
            response.push_str("\n[stderr] ");
            response.push_str(stderr.trim_end());
        }
    }
    Ok((response, exit_code))
}

fn generate_app_with_harness(
    draft_id: &str,
    app_id: &str,
    name: &str,
    harness: &str,
    prompt: &str,
) -> Result<Vec<EventRecord>> {
    let mut records = vec![builder::requested_event(
        draft_id, app_id, name, prompt, harness,
    )?];
    let result = (|| -> Result<Vec<builder::BuilderFile>> {
        let prompt = harness::app_bundle_prompt(app_id, name, prompt);
        let (response, exit_code) = run_harness_command(
            harness,
            &prompt,
            harness::APP_BUNDLE_OUTPUT_SCHEMA,
            "harness-app-bundle.schema.json",
            "harness-app-bundle-last-message.txt",
        )?;
        if exit_code != 0 {
            return Err(Error::Storage(format!(
                "`{harness}` exited with {exit_code}: {}",
                response.trim()
            )));
        }
        let allowed_resources = terrane_core::grant_resource_namespaces();
        builder::parse_generated_files(&response, app_id, name, &allowed_resources)
    })();

    match result {
        Ok(files) => records.push(builder::generated_event(draft_id, files)?),
        Err(e) => records.push(builder::failed_event(draft_id, e.to_string())?),
    }
    Ok(records)
}

fn run_harness_js(
    run_id: &str,
    app_id: &str,
    harness: &str,
    prompt: &str,
    state: &terrane_core::State,
) -> Result<Vec<EventRecord>> {
    let mut records = vec![harness::js_requested_event(
        run_id, app_id, prompt, harness,
    )?];
    let result = (|| -> Result<(String, String, Vec<EventRecord>)> {
        let generated_prompt = harness::run_js_prompt(app_id, prompt);
        let (response, exit_code) = run_harness_command(
            harness,
            &generated_prompt,
            harness::RUN_JS_OUTPUT_SCHEMA,
            "harness-run-js.schema.json",
            "harness-run-js-last-message.txt",
        )?;
        if exit_code != 0 {
            return Err(Error::Storage(format!(
                "`{harness}` exited with {exit_code}: {}",
                response.trim()
            )));
        }
        let js = harness::parse_run_js_output(&response)?;
        let name = state
            .app
            .apps
            .get(app_id)
            .map(|app| app.name.clone())
            .ok_or_else(|| Error::AppNotFound(app_id.to_string()))?;
        let bundle = JsRuntimeBundle {
            source: js.clone(),
            name,
            resources: vec!["kv".to_string(), "build".to_string()],
        };
        let host = RuntimeHostHandle::new(Box::new(
            RuntimeResourceHost::new_with_temporary_resource_grants(
                app_id.to_string(),
                state.clone(),
                ExecutionPrincipal::local_owner(),
                bundle.resources.clone(),
            )
            .with_runner(std::sync::Arc::new(EdgeRunner)),
        ));
        let output = run_js_bundle(app_id, &[], &bundle, host.clone())?;
        Ok((js, output, host.take_records()))
    })();

    match result {
        Ok((js, output, writes)) => {
            records.push(harness::js_generated_event(run_id, &js)?);
            records.extend(writes);
            records.push(harness::js_completed_event(run_id, &output)?);
        }
        Err(e) => records.push(harness::js_failed_event(run_id, e.to_string())?),
    }
    Ok(records)
}

fn import_app_bundle(
    source: &str,
    storage_backend: &Option<String>,
    storage_path: &Option<String>,
    state: &terrane_core::State,
) -> Result<Vec<EventRecord>> {
    let src = Path::new(source);
    let manifest = crate::read_manifest(src)?;
    let id = manifest.id.trim().to_string();
    crate::validate_bundle_id(source, &id).map_err(Error::InvalidInput)?;
    crate::validate_runtime(source, &manifest.runtime).map_err(Error::InvalidInput)?;
    if manifest.runtime != "js" {
        return Err(Error::InvalidInput(
            "app.import currently supports js text bundles only".into(),
        ));
    }
    if state.app.apps.contains_key(&id) {
        return Err(Error::AppExists(id));
    }
    let name = match manifest.name.trim() {
        "" => id.clone(),
        name => name.to_string(),
    };
    let files = read_text_bundle_files(src)?;

    let mut records = Vec::new();
    if let Some(raw_backend) = storage_backend {
        let backend: KvStorageBackend = raw_backend.parse()?;
        backend.ensure_available()?;
        records.push(storage_configured_event(
            Some(id.clone()),
            backend,
            storage_path.clone(),
        )?);
    } else if storage_path.is_some() {
        return Err(Error::InvalidInput(
            "--path requires --storage for app.import".into(),
        ));
    }

    records.push(terrane_cap_app::added_event(
        id.clone(),
        name,
        Some(app_bundle_source(&id)),
        manifest.runtime,
    )?);
    for (path, value) in files {
        records.push(set_event(id.clone(), app_bundle_key(&path)?, value)?);
    }
    Ok(records)
}

fn read_text_bundle_files(root: &Path) -> Result<BTreeMap<String, String>> {
    if !root.is_dir() {
        return Err(Error::InvalidInput(format!(
            "app.import source must be a bundle directory: {}",
            root.display()
        )));
    }
    let mut files = BTreeMap::new();
    collect_text_bundle_files(root, root, &mut files)?;
    if !files.contains_key("manifest.json") {
        return Err(Error::InvalidInput(
            "app.import bundle must contain manifest.json".into(),
        ));
    }
    Ok(files)
}

fn collect_text_bundle_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| Error::Storage(e.to_string()))? {
        let entry = entry.map_err(|e| Error::Storage(e.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|e| Error::Storage(e.to_string()))?;
        let path = entry.path();
        if file_type.is_symlink() {
            return Err(Error::InvalidInput(format!(
                "app.import rejects symlinks: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_text_bundle_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let rel = bundle_relative_path(root, &path)?;
        app_bundle_key(&rel)?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::Runtime(format!("read bundle file {}: {e}", path.display())))?;
        files.insert(rel, text);
    }
    Ok(())
}

fn bundle_relative_path(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|e| Error::Storage(e.to_string()))?;
    let mut parts = Vec::new();
    for component in rel.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(Error::InvalidInput(format!(
                "unsafe bundle path: {}",
                path.display()
            )));
        };
        let part = part.to_str().ok_or_else(|| {
            Error::InvalidInput(format!("bundle path is not UTF-8: {}", path.display()))
        })?;
        parts.push(part.to_string());
    }
    Ok(parts.join("/"))
}

fn run_harness_command(
    harness: &str,
    prompt: &str,
    schema_text: &str,
    schema_name: &str,
    output_name: &str,
) -> Result<(String, i32)> {
    match harness {
        "codex" => {
            let work_dir = harness_work_dir()?;
            let output = work_dir.join(output_name);
            let schema = work_dir.join(schema_name);
            std::fs::write(&schema, schema_text).map_err(|e| {
                Error::Storage(format!(
                    "failed to write harness output schema {}: {e}",
                    schema.display()
                ))
            })?;
            let mut c = Command::new("codex");
            c.args([
                "exec",
                "-c",
                "service_tier=\"fast\"",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "--ignore-rules",
                "--skip-git-repo-check",
                "--color",
                "never",
            ]);
            c.arg("--cd").arg(&work_dir);
            c.arg("--output-schema").arg(&schema);
            c.arg("--output-last-message").arg(&output);
            c.arg(prompt);
            let (stdout, exit_code) = run_capture(&mut c, harness, harness_timeout())?;
            if exit_code != 0 {
                return Ok((stdout, exit_code));
            }
            let response = std::fs::read_to_string(&output).map_err(|e| {
                Error::Storage(format!(
                    "failed to read harness output {}: {e}",
                    output.display()
                ))
            })?;
            Ok((response, exit_code))
        }
        "claude" | "claude-code" => {
            let mut c = Command::new("claude");
            c.args([
                "-p",
                "--no-session-persistence",
                "--tools",
                "",
                "--output-format",
                "json",
                "--json-schema",
                schema_text,
            ]);
            c.arg(prompt);
            let (stdout, exit_code) = run_capture(&mut c, harness, harness_timeout())?;
            if exit_code == 0 {
                Ok((extract_structured_output(&stdout)?, exit_code))
            } else {
                Ok((stdout, exit_code))
            }
        }
        "opencode" => {
            let work_dir = harness_work_dir()?;
            let mut c = Command::new("opencode");
            c.args(["run", "--pure"]);
            c.arg("--dir").arg(&work_dir);
            c.arg(prompt);
            run_capture(&mut c, harness, harness_timeout())
        }
        other => Err(Error::InvalidInput(format!("unknown harness: {other}"))),
    }
}

fn extract_structured_output(raw: &str) -> Result<String> {
    let envelope: serde_json::Value = serde_json::from_str(raw.trim())
        .map_err(|e| Error::InvalidInput(format!("claude output was not valid JSON: {e}")))?;
    let structured = envelope.get("structured_output").ok_or_else(|| {
        Error::InvalidInput("claude output did not contain structured_output".into())
    })?;
    if structured.is_null() {
        return Err(Error::InvalidInput(
            "claude structured_output was null".into(),
        ));
    }
    if !structured.is_object() {
        return Err(Error::InvalidInput(
            "claude structured_output was not a JSON object".into(),
        ));
    }
    Ok(structured.to_string())
}

fn run_capture(command: &mut Command, label: &str, timeout: Duration) -> Result<(String, i32)> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| {
        Error::Storage(format!(
            "failed to run `{label}` (is it installed and on PATH?): {e}"
        ))
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Storage(format!("failed to capture `{label}` stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Storage(format!("failed to capture `{label}` stderr")))?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(Error::Storage(format!(
                    "`{label}` timed out after {timeout:?}"
                )));
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| Error::Storage(format!("failed to join `{label}` stdout reader")))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| Error::Storage(format!("failed to join `{label}` stderr reader")))??;

    let exit_code = status.code().unwrap_or(-1);
    let mut response = String::from_utf8_lossy(&stdout).into_owned();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.trim().is_empty() {
            response.push_str("\n[stderr] ");
            response.push_str(stderr.trim_end());
        }
    }
    Ok((response, exit_code))
}

fn harness_work_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("terrane-harness-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| {
        Error::Storage(format!(
            "failed to create harness work dir {}: {e}",
            dir.display()
        ))
    })?;
    Ok(dir)
}

fn http_get(url: &str) -> Result<(u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        Error::InvalidInput(format!(
            "the built-in runner supports only http:// URLs: {url}"
        ))
    })?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| Error::InvalidInput(format!("bad port in {url}")))?,
        ),
        None => (authority, 80u16),
    };

    let timeout = edge_timeout();
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Storage(e.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::Storage(format!(
            "no socket address resolved for {host}:{port}"
        )));
    }
    let deadline = Instant::now() + timeout;
    let mut last_error = None;
    let mut stream = None;
    for addr in addrs {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&addr, remaining) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        Error::Storage(match last_error {
            Some(e) => format!("HTTP connect to {host}:{port} timed out or failed: {e}"),
            None => format!("HTTP connect to {host}:{port} timed out after {timeout:?}"),
        })
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| Error::Storage(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| Error::Storage(e.to_string()))?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| Error::Storage(e.to_string()))?;
    let text = String::from_utf8_lossy(&raw).into_owned();

    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| Error::Storage("malformed HTTP status line".into()))?;
    Ok((status, body.to_string()))
}

fn read_pipe(mut pipe: impl Read) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    pipe.read_to_end(&mut out)
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(out)
}

fn edge_timeout() -> Duration {
    std::env::var("TERRANE_EDGE_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_EDGE_TIMEOUT)
}

fn harness_timeout() -> Duration {
    std::env::var("TERRANE_HARNESS_TIMEOUT_MS")
        .or_else(|_| std::env::var("TERRANE_BUILDER_TIMEOUT_MS"))
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(180))
}
