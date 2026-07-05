//! The CLI's real [`EffectRunner`] — where the engine's effects meet the world.
//!
//! It performs each [`Effect`] at the edge and hands the result back as the
//! owning capability's recorded event. Replay never calls this. Effects so far:
//! a minimal `http://` GET (`net`), an agent-CLI call (`model`), and minting this
//! home's replica id from OS entropy (`replica`).

use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use terrane_cap_builder as builder;
use terrane_cap_harness as harness;
use terrane_cap_js_runtime::{run_js_bundle, JsRuntimeBundle};
use terrane_cap_kv::{
    app_bundle_app_id, app_bundle_files, app_bundle_key, app_bundle_source, set_event,
    storage_configured_event, KvStorageBackend,
};
use terrane_cap_model::responded_event;
use terrane_cap_net::fetched_event;
use terrane_cap_net::request::{RedirectPolicy, RequestBody, RequestValue, ResponseBodyMode};
use terrane_cap_browser::request::RenderOutput;
use terrane_cap_replica::initialized_event;
use terrane_core::{Effect, EffectRunner, LiveHost};
use terrane_core::{Error, EventRecord, Result};
use terrane_core::{ExecutionPrincipal, RuntimeHostHandle, RuntimeResourceHost};

/// Results a host computed on its own worker thread, waiting to be committed.
///
/// A blocking host can't run a minutes-long harness inside its request loop;
/// it runs [`generate_app_records`] on a worker, stages the records here, and
/// re-dispatches `harness.generate-app`. The runner returns the staged records
/// instead of re-running the CLI, so the commit still flows through the
/// ordinary dispatch → decide → effect → record path and replay is untouched.
#[derive(Clone, Default)]
pub struct HarnessStaging {
    generated:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<EventRecord>>>>,
}

impl HarnessStaging {
    pub fn stage_generated(&self, draft_id: &str, records: Vec<EventRecord>) {
        self.generated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(draft_id.to_string(), records);
    }

    fn take_generated(&self, draft_id: &str) -> Option<Vec<EventRecord>> {
        self.generated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(draft_id)
    }
}

#[derive(Clone, Default)]
pub struct EdgeRunner {
    staging: HarnessStaging,
    home: Option<PathBuf>,
}

impl EdgeRunner {
    pub fn with_staging(staging: HarnessStaging) -> Self {
        Self {
            staging,
            home: None,
        }
    }

    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    fn home(&self) -> Result<&Path> {
        self.home
            .as_deref()
            .ok_or_else(|| Error::Storage("blob CAS requires a Terrane home".into()))
    }

    fn clone_for_nested(&self) -> Self {
        self.clone()
    }
}

/// Run the app-generation harness effect standalone (no core needed). Hosts
/// that background generation call this on a worker thread, then stage the
/// records via [`HarnessStaging`] and re-dispatch `harness.generate-app` to
/// commit them.
pub fn generate_app_records(
    draft_id: &str,
    app_id: &str,
    name: &str,
    harness: &str,
    prompt: &str,
) -> std::result::Result<Vec<EventRecord>, String> {
    generate_app_with_harness(draft_id, app_id, name, harness, prompt).map_err(|e| e.to_string())
}

const DEFAULT_EDGE_TIMEOUT: Duration = Duration::from_secs(30);

impl EffectRunner for EdgeRunner {
    fn run(&self, effect: &Effect, state: &terrane_core::State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                let (status, body) = http_get(url)?;
                Ok(vec![fetched_event(app, url, status, body)?])
            }
            Effect::HttpRequest { app, request } => http_request(self.home()?, state, app, request),
            Effect::BrowserRender { app, request } => {
                browser_render(self.home()?, app, request)
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
            } => match self.staging.take_generated(draft_id) {
                Some(records) => Ok(records),
                None => generate_app_with_harness(draft_id, app_id, name, harness, prompt),
            },
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
            Effect::BlobStore {
                app,
                name,
                mime,
                hash,
                bytes,
            } => {
                crate::blob_store::insert_if_absent(self.home()?, hash, bytes)?;
                Ok(vec![terrane_cap_blob::stored_event(
                    app,
                    name,
                    hash,
                    u64::try_from(bytes.len())
                        .map_err(|_| Error::Storage("blob byte length overflow".into()))?,
                    mime,
                )?])
            }
            Effect::MediaTransform {
                app,
                source_hash,
                source_mime,
                ops_json,
                dest_name,
            } => crate::media_edge::transform_with_home(
                self.home()?,
                app,
                source_hash,
                source_mime,
                ops_json,
                dest_name,
            ),
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
                embed_preset,
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
                embed_preset.as_deref(),
            ),
Effect::LocalModelEmbed {
            app,
            model,
            texts,
            query,
        } => crate::local_llm::embed(app, model, texts, *query, state),
            Effect::ObserveTime { app } => {
                let epoch_ms =
                    terrane_cap_time::system_time_to_epoch_ms(std::time::SystemTime::now())?;
                Ok(vec![terrane_cap_time::observed_event(app, epoch_ms)?])
            }
            Effect::AppLog { app, level, msg, data } => {
                let home = self.home()?;
                crate::app_log::append(home, app, level, msg, data)?;
                if level == "error" {
                    Ok(vec![terrane_cap_telemetry::error_event(
                        app,
                        terrane_cap_telemetry::SOURCE_EXPLICIT,
                        msg,
                        "",
                        data,
                    )?])
                } else {
                    // Transient calls reach here too (the same `Effect::AppLog`
                    // payload); they record nothing per the plan.
                    Ok(Vec::new())
                }
            }
            Effect::AppCall {
                chain,
                target,
                verb,
                args,
            } => run_app_call(self, chain, target, verb, args, state),
            Effect::McpCall {
                app,
                connection,
                tool,
                args,
                args_redacted,
                timeout_ms,
            } => crate::mcp_client::call(
                self.home()?,
                state,
                crate::mcp_client::McpCallRequest {
                    app,
                    connection,
                    tool,
                    args,
                    args_redacted,
                    timeout_ms: *timeout_ms,
                },
            ),
            Effect::McpTools { app, connection } => {
                crate::mcp_client::list_tools(self.home()?, state, app, connection)
            }
        }
    }

    /// The edge samples live system metrics for `ctx.resource.sysinfo` reads.
    /// These observe the host and record nothing, so they are not effects.
    fn live(&self) -> Option<&dyn LiveHost> {
        Some(self)
    }
}

fn run_app_call(
    runner: &EdgeRunner,
    chain: &[String],
    target: &str,
    verb: &str,
    args: &[String],
    state: &terrane_core::State,
) -> Result<Vec<EventRecord>> {
    let target_app = state
        .app
        .apps
        .get(target)
        .ok_or_else(|| Error::AppNotFound(target.to_string()))?;
    if target_app.runtime != "js" {
        return Err(Error::InvalidInput(format!(
            "interop currently supports js targets only, got {}",
            target_app.runtime
        )));
    }
    let source = target_app
        .source
        .as_deref()
        .ok_or_else(|| Error::Runtime(format!("app {target} has no --source bundle")))?;
    let bundle = load_app_call_bundle(target, source, state)?;
    let mut input = Vec::with_capacity(args.len() + 1);
    input.push(verb.to_string());
    input.extend(args.iter().cloned());
    let principal = ExecutionPrincipal::app_caller(
        chain
            .last()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
    );
    let host = RuntimeHostHandle::new(Box::new(
        RuntimeResourceHost::new_with_temporary_resource_grants(
            target.to_string(),
            state.clone(),
            principal,
            bundle.resources.clone(),
        )
            .with_runner(std::sync::Arc::new(runner.clone_for_nested()))
            .with_interop_chain({
                let mut next = chain.to_vec();
                next.push(target.to_string());
                next
            }),
    ));
    let result = run_js_bundle(target, &input, &bundle, host.clone());
    let mut records = host.take_records();
    let caller = chain.last().map(String::as_str).unwrap_or("");
    match result {
        Ok(reply) => {
            records.extend(interop_reply_records(runner, caller, target, verb, args, &reply, true)?);
            Ok(records)
        }
        Err(err) => {
            let reply = err.to_string();
            records.extend(interop_reply_records(
                runner, caller, target, verb, args, &reply, false,
            )?);
            Ok(records)
        }
    }
}

fn load_app_call_bundle(
    target: &str,
    source: &str,
    state: &terrane_core::State,
) -> Result<JsRuntimeBundle> {
    if let Some(source_app) = app_bundle_app_id(source) {
        if source_app != target {
            return Err(Error::Runtime(format!(
                "app {target} points at kv bundle for different app {source_app}"
            )));
        }
        return terrane_cap_js_runtime::bundle_from_files(&app_bundle_files(state, target)?);
    }
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = terrane_cap_js_runtime::read_manifest(path)?;
        let js_path = path.join(&manifest.backend);
        let source = std::fs::read_to_string(&js_path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", js_path.display())))?;
        return Ok(JsRuntimeBundle {
            source,
            name: manifest.name,
            resources: manifest.resources,
        });
    }
    let source = std::fs::read_to_string(path)
        .map_err(|e| Error::Runtime(format!("read backend {}: {e}", path.display())))?;
    Ok(JsRuntimeBundle {
        source,
        name: String::new(),
        resources: vec!["kv".to_string()],
    })
}

fn interop_reply_records(
    runner: &EdgeRunner,
    caller: &str,
    target: &str,
    verb: &str,
    args: &[String],
    reply: &str,
    ok: bool,
) -> Result<Vec<EventRecord>> {
    let bytes = reply.as_bytes();
    if bytes.len() > terrane_cap_interop::BLOB_REPLY_LIMIT {
        return Err(Error::InvalidInput(format!(
            "interop reply exceeds {} bytes",
            terrane_cap_interop::BLOB_REPLY_LIMIT
        )));
    }
    let hash = terrane_cap_interop::sha256_hex(bytes);
    if bytes.len() <= terrane_cap_interop::INLINE_REPLY_LIMIT {
        return Ok(vec![terrane_cap_interop::called_event(
            terrane_cap_interop::CalledEvent {
                caller,
                target,
                verb,
                args,
                reply_kind: "inline",
                reply,
                reply_hash: &hash,
                ok,
            },
        )?]);
    }
    crate::blob_store::insert_if_absent(runner.home()?, &hash, bytes)?;
    Ok(vec![
        terrane_cap_blob::stored_event(
            caller,
            format!("__interop__/{target}/{hash}"),
            &hash,
            u64::try_from(bytes.len())
                .map_err(|_| Error::Storage("interop reply length overflow".into()))?,
            "text/plain",
        )?,
        terrane_cap_interop::called_event(terrane_cap_interop::CalledEvent {
            caller,
            target,
            verb,
            args,
            reply_kind: "blob",
            reply: "",
            reply_hash: &hash,
            ok,
        })?,
    ])
}

impl LiveHost for EdgeRunner {
    fn sample(&self, domain: &str, args: &[String]) -> Result<String> {
        match domain {
            "blob.get" => {
                let hash = args
                    .get(2)
                    .ok_or_else(|| Error::InvalidInput("blob.get missing hash".into()))?;
                crate::blob_store::read_verified_base64(self.home()?, hash)
            }
            "media.info" => {
                let hash = args
                    .get(2)
                    .ok_or_else(|| Error::InvalidInput("media.info missing hash".into()))?;
                let mime = args
                    .get(4)
                    .ok_or_else(|| Error::InvalidInput("media.info missing mime".into()))?;
                let bytes = crate::blob_store::read_verified(self.home()?, hash)?;
                crate::media_edge::info(&bytes, mime)
            }
            "telemetry.read" => {
                let app = args
                    .first()
                    .ok_or_else(|| Error::InvalidInput("telemetry.read missing app".into()))?;
                let level = args.get(1).cloned().unwrap_or_default();
                let tail = args
                    .get(2)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(200);
                crate::app_log::read_tail(self.home()?, app, &level, tail)
            }
            _ => crate::metrics::sample(domain, args),
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

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut command);
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
                kill_process_tree(&mut child);
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
            .with_runner(std::sync::Arc::new(EdgeRunner::default())),
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
    crate::validate_common_api_bundle(src).map_err(Error::InvalidInput)?;
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

    records.push(terrane_cap_app::added_event_with_interfaces(
        id.clone(),
        name,
        Some(app_bundle_source(&id)),
        manifest.runtime,
        terrane_cap_app::normalize_interfaces(manifest.interfaces),
    )?);
    for link in terrane_cap_app::default_scheme_links(&id) {
        records.push(terrane_cap_app::link_registered_event(
            &id, &link.kind, &link.spec,
        )?);
    }
    let file_types = crate::manifest_file_types_arg(&manifest.file_types)
        .map_err(Error::InvalidInput)?;
    for spec in file_types.split(',').filter(|spec| !spec.is_empty()) {
        records.push(terrane_cap_app::link_registered_event(
            &id, "filetype", spec,
        )?);
    }
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

/// Give a CLI child its own process group so a timeout kill reaps its whole
/// tree. The npm `codex` wrapper hands the real work to a native child;
/// killing only the wrapper leaves that grandchild running — and holding our
/// stdout/stderr pipes, which blocks the reader threads forever.
fn isolate_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGKILL);
    }
    let _ = child.kill();
}

fn run_capture(command: &mut Command, label: &str, timeout: Duration) -> Result<(String, i32)> {
    // stdin must be closed, not inherited: `codex exec` sees a non-TTY stdin
    // (e.g. the pipe a supervisor gave this host) and blocks reading it until
    // EOF — which never comes, so generation hangs at 0% CPU forever.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(command);
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
                kill_process_tree(&mut child);
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

/// A GET that speaks both `http://` and `https://` (TLS via ureq/rustls) and
/// handles redirects/chunked/gzip — needed for real services like the HIBP
/// range API. A non-2xx status is returned as data (status + body), not an
/// error, so callers can decide what to do.
fn http_get(url: &str) -> Result<(u16, String)> {
    let timeout = edge_timeout();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .build();
    match agent.get(url).call() {
        Ok(resp) => {
            let status = resp.status();
            let body = resp
                .into_string()
                .map_err(|e| Error::Storage(format!("reading HTTP body from {url} failed: {e}")))?;
            Ok((status, body))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Ok((code, body))
        }
        Err(ureq::Error::Transport(transport)) => Err(Error::Storage(format!(
            "HTTP GET {url} failed: {transport}"
        ))),
    }
}

fn http_request(home: &Path, state: &terrane_core::State, app: &str, request: &str) -> Result<Vec<EventRecord>> {
    let prepared = terrane_cap_net::request::prepare_request(request)?;
    let execution_request = if prepared.has_unresolved_secret {
        crate::secret_store::resolve_net_request(home, state, app, request)?
    } else {
        request.to_string()
    };
    let execution_prepared = terrane_cap_net::request::prepare_request(&execution_request)?;
    validate_http_target(&execution_prepared.url)?;

    let timeout = Duration::from_millis(execution_prepared.timeout_ms);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .redirects(0)
        .build();
    let resp = perform_http_request(&agent, &execution_prepared)?;

    let status = resp.status();
    let response_headers = filtered_response_headers(&resp);
    let mime = response_headers
        .get("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = read_response_bytes(resp, terrane_cap_net::request::BODY_HARD_LIMIT)?;
    let body_size = u64::try_from(bytes.len())
        .map_err(|_| Error::Storage("HTTP response body length overflow".into()))?;
    let body_hash = terrane_cap_net::request::sha256_hex(&bytes);
    let recorded_body =
        choose_recorded_body(&prepared.response_body, &bytes, &body_hash, body_size, &mime)?;

    let mut records = Vec::new();
    if recorded_body.kind == "blob" {
        crate::blob_store::insert_if_absent(home, &body_hash, &bytes)?;
        records.push(terrane_cap_blob::stored_event(
            app,
            format!("__net__/{}", prepared.request_key),
            &body_hash,
            body_size,
            &mime,
        )?);
    }
    records.push(terrane_cap_net::responded_event(
        app,
        prepared.request_key,
        prepared.redacted_json,
        status,
        response_headers,
        recorded_body,
    )?);
    Ok(records)
}

fn browser_render(home: &Path, app: &str, request: &str) -> Result<Vec<EventRecord>> {
    let prepared = terrane_cap_browser::request::prepare_render(request)?;
    validate_browser_target(&prepared.url, &prepared.allowed_hosts)?;
    let capture = run_chromium_render(&prepared)?;
    let hash = terrane_cap_browser::request::sha256_hex(&capture.bytes);
    let size = u64::try_from(capture.bytes.len())
        .map_err(|_| Error::Storage("browser capture length overflow".into()))?;
    let body = choose_browser_body(&prepared.output, &capture.bytes, &hash, size)?;

    let mut records = Vec::new();
    if body.kind == "blob" {
        crate::blob_store::insert_if_absent(home, &hash, &capture.bytes)?;
        records.push(terrane_cap_blob::stored_event(
            app,
            format!("__browser__/{}", prepared.request_key),
            &hash,
            size,
            prepared.output.mime(),
        )?);
    }
    records.push(terrane_cap_browser::rendered_event(
        terrane_cap_browser::RenderedEvent {
            app: app.to_string(),
            request_key: prepared.request_key,
            request_json_redacted: prepared.redacted_json,
            url: prepared.url,
            output: prepared.output.as_str().to_string(),
            status: capture.status,
            body,
            title: capture.title,
        },
    )?);
    Ok(records)
}

struct BrowserCapture {
    status: u16,
    title: String,
    bytes: Vec<u8>,
}

struct EphemeralProfile {
    path: PathBuf,
}

impl EphemeralProfile {
    fn new() -> Result<Self> {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "terrane-browser-{}-{}",
            std::process::id(),
            unix_nanos()
        ));
        std::fs::create_dir(&path)
            .map_err(|e| Error::Storage(format!("create browser profile {}: {e}", path.display())))?;
        Ok(Self { path })
    }
}

impl Drop for EphemeralProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn run_chromium_render(
    prepared: &terrane_cap_browser::request::PreparedRender,
) -> Result<BrowserCapture> {
    let chrome = find_chromium().ok_or_else(|| {
        Error::Storage(
            "BrowserUnavailable: no system Chrome/Chromium found for browser.render".into(),
        )
    })?;
    let profile = EphemeralProfile::new()?;
    let output_file = match prepared.output {
        RenderOutput::Screenshot | RenderOutput::Pdf => {
            let mut path = profile.path.clone();
            path.push(if prepared.output == RenderOutput::Pdf {
                "capture.pdf"
            } else {
                "capture.png"
            });
            Some(path)
        }
        RenderOutput::Text | RenderOutput::Html => None,
    };
    let mut args = vec![
        "--headless=new".to_string(),
        "--disable-gpu".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--no-sandbox".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        format!("--user-data-dir={}", profile.path.display()),
        format!("--window-size={},{}", prepared.viewport_w, prepared.viewport_h),
        format!("--virtual-time-budget={}", prepared.wait_ms),
    ];
    match (&prepared.output, &output_file) {
        (RenderOutput::Screenshot, Some(path)) => {
            args.push(format!("--screenshot={}", path.display()));
        }
        (RenderOutput::Pdf, Some(path)) => {
            args.push(format!("--print-to-pdf={}", path.display()));
            args.push("--no-pdf-header-footer".to_string());
        }
        (RenderOutput::Text | RenderOutput::Html, None) => {
            args.push("--dump-dom".to_string());
        }
        _ => return Err(Error::Runtime("invalid browser render output path state".into())),
    }
    args.push(prepared.url.clone());

    let mut output = run_command_with_timeout(&chrome, &args, Duration::from_millis(
        terrane_cap_browser::request::TOTAL_TIMEOUT_MS,
    ))?;
    if !output.status.success() {
        let mut legacy_args = args.clone();
        if let Some(headless) = legacy_args.iter_mut().find(|arg| arg.as_str() == "--headless=new") {
            *headless = "--headless".to_string();
            output = run_command_with_timeout(&chrome, &legacy_args, Duration::from_millis(
                terrane_cap_browser::request::TOTAL_TIMEOUT_MS,
            ))?;
        }
    }
    if !output.status.success() {
        return Err(Error::Storage(format!(
            "browser render failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let html = if matches!(prepared.output, RenderOutput::Text | RenderOutput::Html) {
        String::from_utf8(output.stdout)
            .map_err(|e| Error::Storage(format!("browser DOM output was not UTF-8: {e}")))?
    } else {
        String::new()
    };
    let bytes = match (&prepared.output, output_file) {
        (RenderOutput::Text, None) => html_to_text(&html).into_bytes(),
        (RenderOutput::Html, None) => html.into_bytes(),
        (RenderOutput::Screenshot | RenderOutput::Pdf, Some(path)) => std::fs::read(&path)
            .map_err(|e| Error::Storage(format!("read browser capture {}: {e}", path.display())))?,
        _ => return Err(Error::Runtime("invalid browser capture state".into())),
    };
    if bytes.len() > terrane_cap_browser::request::BODY_HARD_LIMIT {
        return Err(Error::Storage(format!(
            "browser capture exceeds {} bytes",
            terrane_cap_browser::request::BODY_HARD_LIMIT
        )));
    }
    Ok(BrowserCapture {
        status: 200,
        title: extract_html_title(&String::from_utf8_lossy(&bytes)),
        bytes,
    })
}

fn find_chromium() -> Option<String> {
    let candidates = [
        std::env::var("TERRANE_CHROME").ok(),
        Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()),
        Some("/Applications/Chromium.app/Contents/MacOS/Chromium".to_string()),
        Some("google-chrome".to_string()),
        Some("google-chrome-stable".to_string()),
        Some("chromium".to_string()),
        Some("chromium-browser".to_string()),
        Some("chrome".to_string()),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.contains('/') {
            if Path::new(&candidate).exists() {
                return Some(candidate);
            }
        } else if command_exists(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_command_with_timeout(
    command: &str,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output> {
    let mut child = Command::new(command)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Storage(format!("spawn browser engine {command}: {e}")))?;
    let start = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|e| Error::Storage(format!("poll browser engine: {e}")))?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|e| Error::Storage(format!("collect browser output: {e}")));
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Storage(format!(
                "browser render exceeded {} ms",
                timeout.as_millis()
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn choose_browser_body(
    output: &RenderOutput,
    bytes: &[u8],
    hash: &str,
    size: u64,
) -> Result<terrane_cap_browser::RecordedBody> {
    let inline = match output {
        RenderOutput::Text | RenderOutput::Html => {
            bytes.len() <= terrane_cap_browser::request::INLINE_AUTO_LIMIT
        }
        RenderOutput::Screenshot | RenderOutput::Pdf => false,
    };
    if inline {
        let body = String::from_utf8(bytes.to_vec())
            .map_err(|e| Error::Storage(format!("browser text/html capture was not UTF-8: {e}")))?;
        return Ok(terrane_cap_browser::RecordedBody {
            kind: "inline".to_string(),
            body,
            hash: hash.to_string(),
            size,
            mime: output.mime().to_string(),
        });
    }
    Ok(terrane_cap_browser::RecordedBody {
        kind: "blob".to_string(),
        body: String::new(),
        hash: hash.to_string(),
        size,
        mime: output.mime().to_string(),
    })
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut last_space = true;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            _ if in_tag => {}
            _ if ch.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            _ => {
                out.push(ch);
                last_space = false;
            }
        }
    }
    out.trim().to_string()
}

fn extract_html_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<title>") else {
        return String::new();
    };
    let body_start = start + "<title>".len();
    let Some(end_rel) = lower[body_start..].find("</title>") else {
        return String::new();
    };
    html[body_start..body_start + end_rel].trim().to_string()
}

fn validate_browser_target(
    url: &str,
    allowed_hosts: &[String],
) -> Result<()> {
    let (scheme, rest) = split_url_scheme(url)?;
    if !matches!(scheme, "http" | "https") {
        return Err(Error::InvalidInput(format!(
            "browser render URL scheme must be http or https: {scheme}"
        )));
    }
    let host_port = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidInput("browser render URL missing host".into()))?;
    let (host, port) = split_host_port(host_port, scheme)?;
    if !allowed_hosts.is_empty() && !allowed_hosts.iter().any(|allowed| allowed == &host) {
        return Err(Error::InvalidInput(format!(
            "browser render host {host} is not in allowedHosts"
        )));
    }
    if host == "169.254.169.254" {
        return Err(Error::InvalidInput(
            "browser render to cloud metadata address 169.254.169.254 is denied".into(),
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        deny_browser_metadata_ip(ip)?;
        return Ok(());
    }
    for addr in (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Error::Storage(format!("resolve {host}: {e}")))?
    {
        deny_browser_metadata_ip(addr.ip())?;
    }
    Ok(())
}

fn deny_browser_metadata_ip(ip: IpAddr) -> Result<()> {
    if ip == IpAddr::from([169, 254, 169, 254]) {
        return Err(Error::InvalidInput(
            "browser render to cloud metadata address 169.254.169.254 is denied".into(),
        ));
    }
    Ok(())
}

fn perform_http_request(
    agent: &ureq::Agent,
    prepared: &terrane_cap_net::request::PreparedRequest,
) -> Result<ureq::Response> {
    let mut url = prepared.url.clone();
    let original_scheme = url_scheme(&url)?.to_string();
    for hop in 0..=5 {
        validate_http_target(&url)?;
        let resp = perform_single_http_request(agent, prepared, &url)?;
        if !(300..400).contains(&resp.status()) {
            return Ok(resp);
        }
        match prepared.redirect {
            RedirectPolicy::Manual => return Ok(resp),
            RedirectPolicy::Deny => {
                return Err(Error::Storage(format!(
                    "HTTP {} {} refused redirect status {}",
                    prepared.method,
                    url,
                    resp.status()
                )))
            }
            RedirectPolicy::Follow => {
                if hop == 5 {
                    return Err(Error::Storage(format!(
                        "HTTP {} {} exceeded 5 redirects",
                        prepared.method, prepared.url
                    )));
                }
                let location = resp.header("location").ok_or_else(|| {
                    Error::Storage(format!(
                        "HTTP {} {} redirect missing location",
                        prepared.method, url
                    ))
                })?;
                let next = resolve_redirect_url(&url, location)?;
                let next_scheme = url_scheme(&next)?;
                if original_scheme == "https" && next_scheme == "http" {
                    return Err(Error::Storage(format!(
                        "HTTP {} {} refused https-to-http redirect",
                        prepared.method, prepared.url
                    )));
                }
                url = next;
            }
        }
    }
    Err(Error::Storage(format!(
        "HTTP {} {} exceeded 5 redirects",
        prepared.method, prepared.url
    )))
}

fn perform_single_http_request(
    agent: &ureq::Agent,
    prepared: &terrane_cap_net::request::PreparedRequest,
    url: &str,
) -> Result<ureq::Response> {
    let mut req = agent.request(&prepared.method, url);
    for header in &prepared.headers {
        let RequestValue::Plain(value) = &header.value else {
            return Err(Error::InvalidInput(
                "net.request contains unresolved {$secret}; secret resolution belongs to cap-oauth-connections"
                    .into(),
            ));
        };
        req = req.set(&header.name, value);
    }
    let resp = match &prepared.body {
        Some(RequestBody::Text(body)) => req.send_string(body),
        Some(RequestBody::Base64(bytes)) => req.send_bytes(bytes),
        Some(RequestBody::Secret(_)) => {
            return Err(Error::InvalidInput(
                "net.request contains unresolved {$secret}; secret resolution belongs to cap-oauth-connections"
                    .into(),
            ))
        }
        None => req.call(),
    };
    match resp {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::Status(_, resp)) => Ok(resp),
        Err(ureq::Error::Transport(transport)) => Err(Error::Storage(format!(
            "HTTP {} {} failed: {transport}",
            prepared.method, url
        ))),
    }
}

fn filtered_response_headers(resp: &ureq::Response) -> BTreeMap<String, String> {
    const ALLOWED: &[&str] = &[
        "content-type",
        "content-length",
        "etag",
        "last-modified",
        "location",
        "cache-control",
    ];
    let mut out = BTreeMap::new();
    for name in ALLOWED {
        if let Some(value) = resp.header(name) {
            out.insert((*name).to_string(), value.to_string());
        }
    }
    out
}

fn read_response_bytes(resp: ureq::Response, hard_limit: usize) -> Result<Vec<u8>> {
    let mut reader = resp.into_reader().take(
        u64::try_from(hard_limit + 1)
            .map_err(|_| Error::Storage("HTTP response limit overflow".into()))?,
    );
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Storage(format!("reading HTTP response body failed: {e}")))?;
    if bytes.len() > hard_limit {
        return Err(Error::Storage(format!(
            "HTTP response body exceeds {hard_limit} bytes"
        )));
    }
    Ok(bytes)
}

fn choose_recorded_body(
    mode: &ResponseBodyMode,
    bytes: &[u8],
    hash: &str,
    size: u64,
    mime: &str,
) -> Result<terrane_cap_net::RecordedBody> {
    let text = is_text_mime(mime);
    let inline = match mode {
        ResponseBodyMode::Inline => {
            if bytes.len() > terrane_cap_net::request::INLINE_FORCED_LIMIT {
                return Err(Error::Storage(format!(
                    "inline HTTP response body exceeds {} bytes",
                    terrane_cap_net::request::INLINE_FORCED_LIMIT
                )));
            }
            true
        }
        ResponseBodyMode::Blob => false,
        ResponseBodyMode::Auto => {
            text && bytes.len() <= terrane_cap_net::request::INLINE_AUTO_LIMIT
        }
    };
    if inline {
        let (body, is_base64) = if text {
            match String::from_utf8(bytes.to_vec()) {
                Ok(body) => (body, false),
                Err(_) => (B64.encode(bytes), true),
            }
        } else {
            (B64.encode(bytes), true)
        };
        return Ok(terrane_cap_net::RecordedBody {
            kind: "inline".to_string(),
            body,
            is_base64,
            hash: hash.to_string(),
            size,
            mime: mime.to_string(),
        });
    }
    Ok(terrane_cap_net::RecordedBody {
        kind: "blob".to_string(),
        body: String::new(),
        is_base64: false,
        hash: hash.to_string(),
        size,
        mime: mime.to_string(),
    })
}

fn is_text_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/javascript"
                | "application/xml"
                | "application/x-www-form-urlencoded"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}

fn validate_http_target(url: &str) -> Result<()> {
    let (scheme, rest) = split_url_scheme(url)?;
    if !matches!(scheme, "http" | "https") {
        return Err(Error::InvalidInput(format!(
            "net request URL scheme must be http or https: {scheme}"
        )));
    }
    let host_port = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidInput("net request URL missing host".into()))?;
    let (host, port) = split_host_port(host_port, scheme)?;
    if host == "169.254.169.254" {
        return Err(Error::InvalidInput(
            "net request to cloud metadata address 169.254.169.254 is denied".into(),
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        deny_metadata_ip(ip)?;
        return Ok(());
    }
    for addr in (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Error::Storage(format!("resolve {host}: {e}")))?
    {
        deny_metadata_ip(addr.ip())?;
    }
    Ok(())
}

fn resolve_redirect_url(current: &str, location: &str) -> Result<String> {
    if location.contains("://") {
        return Ok(location.to_string());
    }
    let (scheme, rest) = split_url_scheme(current)?;
    let host_port = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidInput("net request URL missing host".into()))?;
    if location.starts_with('/') {
        return Ok(format!("{scheme}://{host_port}{location}"));
    }
    let path = rest
        .split_once('/')
        .map(|(_, path)| path.split(['?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    let base = path.rsplit_once('/').map(|(base, _)| base).unwrap_or("");
    if base.is_empty() {
        Ok(format!("{scheme}://{host_port}/{location}"))
    } else {
        Ok(format!("{scheme}://{host_port}/{base}/{location}"))
    }
}

fn url_scheme(url: &str) -> Result<&str> {
    split_url_scheme(url).map(|(scheme, _)| scheme)
}

fn split_url_scheme(url: &str) -> Result<(&str, &str)> {
    url.split_once("://")
        .ok_or_else(|| Error::InvalidInput("net request URL must include http:// or https://".into()))
}

fn split_host_port(host_port: &str, scheme: &str) -> Result<(String, u16)> {
    let default_port = if scheme == "https" { 443 } else { 80 };
    if let Some(rest) = host_port.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| Error::InvalidInput("invalid bracketed IPv6 URL host".into()))?;
        let port = if let Some(port) = tail.strip_prefix(':') {
            parse_port(port)?
        } else {
            default_port
        };
        return Ok((host.to_string(), port));
    }
    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((host.to_string(), parse_port(port)?)),
        _ => Ok((host_port.to_string(), default_port)),
    }
}

fn parse_port(port: &str) -> Result<u16> {
    port.parse::<u16>()
        .map_err(|_| Error::InvalidInput(format!("invalid URL port: {port}")))
}

fn deny_metadata_ip(ip: IpAddr) -> Result<()> {
    if ip == IpAddr::from([169, 254, 169, 254]) {
        return Err(Error::InvalidInput(
            "net request to cloud metadata address 169.254.169.254 is denied".into(),
        ));
    }
    Ok(())
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
        // Real app generations regularly need minutes; hosts background the
        // harness, so a generous default no longer stalls anything.
        .unwrap_or(Duration::from_secs(600))
}
