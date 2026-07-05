use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use nanoserde::{DeJson, SerJson};
use terrane_api::{HealthResponse, InvokeRequest, InvokeResponse, CONTRACT_VERSION};
use terrane_host::{PreviewFile, PreviewStore};
use tiny_http::{Method, Request, Response};

use crate::http::{admin_authorized, authorized, header, json_error, json_ok, Resp};
use crate::shim::{inject_app_shim, inject_preview_shim};
use crate::static_files::{content_type, safe_within};

#[derive(DeJson)]
struct CreatePreviewRequest {
    files: Vec<PreviewFile>,
}

#[derive(DeJson)]
struct BuilderGenerateRequest {
    id: String,
    name: String,
    prompt: String,
    #[nserde(default)]
    harness: String,
    #[nserde(default)]
    agent: String,
}

#[derive(DeJson)]
struct BuilderStatusRequest {
    id: String,
}

#[derive(SerJson)]
struct BuilderJobStatus {
    id: String,
    status: String,
}

#[derive(DeJson)]
struct PreviewDecisionRequest {
    #[nserde(default)]
    reason: String,
    #[nserde(default)]
    app: String,
}

type PreviewDecisionFn =
    fn(
        &mut PreviewStore,
        &str,
        &str,
        &str,
    ) -> Result<Option<terrane_host::permission::PermissionRequestView>, String>;

type InstalledDecisionFn = fn(
    &mut terrane_host::HostCore,
    &crate::admin::AdminSessionState,
    &str,
    &mut Request,
    &str,
) -> Resp;

struct DecisionContext<'a> {
    core: &'a mut terrane_host::HostCore,
    previews: &'a mut PreviewStore,
    admin_session: &'a crate::admin::AdminSessionState,
    request_id: &'a str,
    request: &'a mut Request,
    admin_base_url: &'a str,
}

pub struct RouteState<'a> {
    pub previews: &'a mut PreviewStore,
    pub admin_session: &'a mut crate::admin::AdminSessionState,
    pub builder_jobs: &'a mut crate::builder_jobs::BuilderJobs,
    pub agent_jobs: &'a mut crate::agent_jobs::AgentJobs,
}

#[derive(Clone, Copy)]
pub struct RouteConfig<'a> {
    pub require_auth: bool,
    pub token: Option<&'a str>,
    pub live_reload: bool,
    pub admin_base_url: &'a str,
    pub dev_apps: &'a crate::dev_apps::DevApps,
    pub premium_url: Option<&'a str>,
}

pub fn route(
    core: &mut terrane_host::HostCore,
    state: RouteState<'_>,
    request: &mut Request,
    config: RouteConfig<'_>,
) -> Resp {
    let RouteState {
        previews,
        admin_session,
        builder_jobs,
        agent_jobs,
    } = state;
    let RouteConfig {
        require_auth,
        token,
        live_reload,
        admin_base_url,
        dev_apps,
        premium_url,
    } = config;
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if let (Method::Post, ["hook", app, name, token]) = (&method, segs.as_slice()) {
        return webhook(core, app, name, token, request, admin_base_url);
    }

    if require_auth && !authorized(request, token) {
        return json_error(401, "unauthorized");
    }

    if is_admin_control_route(&method, segs.as_slice()) && !admin_authorized(request) {
        return json_error(403, "admin header required");
    }
    match (&method, segs.as_slice()) {
        (Method::Get, []) => {
            let locale = negotiate_locale(request);
            let (system, _) = shell_i18n_data(core, &locale, None);
            crate::home::page(live_reload, &locale, &system)
        }
        (Method::Get, ["healthz"]) => json_ok(&HealthResponse {
            status: "ok".into(),
            version: CONTRACT_VERSION.into(),
        }),
        (Method::Get, ["__terrane", "admin"]) => {
            let locale = negotiate_locale(request);
            let (system, app) = shell_i18n_data(core, &locale, None);
            crate::shell::admin_response(
                live_reload,
                premium_url,
                &crate::shell::ShellI18n {
                    locale: &locale,
                    dir: terrane_host::i18n::dir_for(&locale),
                    system_messages: &system,
                    app_messages: &app,
                },
            )
        }
        (Method::Get, ["__terrane", "admin", "session"]) => crate::admin::session(admin_session),
        (Method::Post, ["__terrane", "admin", "local", "lock"]) => {
            crate::admin::lock(admin_session)
        }
        (Method::Post, ["__terrane", "admin", "local", "unlock"]) => {
            crate::admin::unlock(admin_session)
        }
        (Method::Get, ["__terrane", "admin", "apps"]) => crate::admin::apps(core),
        (Method::Get, ["__terrane", "admin", "grants"]) => crate::admin::grants(core),
        (Method::Get, ["__terrane", "admin", "agents"]) => crate::admin::agents(core),
        (Method::Get, ["__terrane", "admin", "audit"]) => crate::admin::audit(core),
        (Method::Post, ["__terrane", "admin", "agents"]) => {
            crate::admin::register_agent(core, admin_session, request)
        }
        (Method::Post, ["__terrane", "admin", "agents", agent, "delegate"]) => {
            crate::admin::delegate_agent(core, admin_session, agent, request)
        }
        (Method::Delete, ["__terrane", "admin", "agents", agent]) => {
            crate::admin::revoke_agent(core, admin_session, agent)
        }
        (Method::Get, ["__terrane", "admin", "requests"]) => {
            admin_requests(core, previews, admin_base_url)
        }
        (Method::Post, ["__terrane", "admin", "requests", request_id, "approve"]) => {
            approve_request(
                core,
                previews,
                admin_session,
                request_id,
                request,
                admin_base_url,
            )
        }
        (Method::Post, ["__terrane", "admin", "requests", request_id, "deny"]) => deny_request(
            core,
            previews,
            admin_session,
            request_id,
            request,
            admin_base_url,
        ),
        (Method::Post, ["__terrane", "admin", "requests", request_id, "cancel"]) => cancel_request(
            core,
            previews,
            admin_session,
            request_id,
            request,
            admin_base_url,
        ),
        (Method::Post, ["__terrane", "admin", "requests", request_id, "promote"]) => {
            promote_request(
                core,
                previews,
                admin_session,
                request_id,
                request,
                admin_base_url,
            )
        }
        (Method::Post, ["__terrane", "admin", "grants"]) => {
            crate::admin::grant(core, admin_session, request)
        }
        (Method::Delete, ["__terrane", "admin", "grants"]) => {
            crate::admin::revoke(core, admin_session, request)
        }
        (Method::Post, ["__terrane", "admin", "stt", "open"]) => {
            crate::stt::admin_open_route(core, request)
        }
        (Method::Post, ["__terrane", "admin", "stt", "segment"]) => {
            crate::stt::admin_segment_route(core, request)
        }
        (Method::Post, ["__terrane", "admin", "stt", "close"]) => {
            crate::stt::admin_close_route(core, request)
        }
        (Method::Get, ["__terrane", "stt", "worklet.js"]) => crate::stt::worklet_response(),
        (Method::Get, ["__terrane", "stt", "config"]) => crate::stt::config_response(),
        (Method::Get, ["__terrane", "admin", "requests", _request_id]) => {
            let locale = negotiate_locale(request);
            let (system, app) = shell_i18n_data(core, &locale, None);
            crate::shell::admin_response(
                live_reload,
                premium_url,
                &crate::shell::ShellI18n {
                    locale: &locale,
                    dir: terrane_host::i18n::dir_for(&locale),
                    system_messages: &system,
                    app_messages: &app,
                },
            )
        }
        (Method::Get, ["__terrane", "agents"]) => crate::agents::list(core),
        (Method::Post, ["__terrane", "agents"]) => crate::agents::create(core, request),
        (Method::Post, ["__terrane", "agents", "assist", "status"]) => {
            crate::agents::assist_status(agent_jobs, request)
        }
        (Method::Post, ["__terrane", "agents", id, "assist"]) => {
            crate::agents::assist_start(core, agent_jobs, id, request, admin_base_url)
        }
        (Method::Post, ["__terrane", "agents", id]) => crate::agents::update(core, id, request),
        (Method::Post, ["__terrane", "builder", "generate"]) => {
            builder_generate(core, builder_jobs, request)
        }
        (Method::Post, ["__terrane", "builder", "status"]) => {
            builder_status(core, builder_jobs, request)
        }
        (Method::Post, ["__terrane", "previews"]) => create_preview(core, previews, request),
        (Method::Get, ["__terrane", "previews", id, "frame"]) => serve_preview(previews, id, ""),
        (Method::Get, ["__terrane", "previews", id, "frame", rest @ ..]) => {
            serve_preview(previews, id, &rest.join("/"))
        }
        (Method::Post, ["__terrane", "previews", id, "invoke"]) => {
            invoke_preview(previews, id, request, admin_base_url)
        }
        (Method::Delete, ["__terrane", "previews", id]) => destroy_preview(previews, id),
        (Method::Get | Method::Post, ["__terrane", "previews", ..]) => json_error(404, "not found"),
        (Method::Get, ["apps"]) => with_local_cors(request, json_ok(&merged_apps(core, dev_apps))),
        (Method::Post, ["mcp"]) => mcp(core, request),
        (Method::Get, ["mcp"]) => json_error(405, "method not allowed"),
        (Method::Get, ["apps", id, "__terrane", "live-version"]) if live_reload => {
            crate::live_reload::response(app_source(core, dev_apps, id), id)
        }
        (Method::Get, ["apps", id, "logs"]) => app_logs(id, request),
        (Method::Get, ["apps", id, "__terrane", "frame"]) => {
            serve_ui(core, dev_apps, id, "", live_reload)
        }
        (Method::Get, ["apps", id, "__terrane", "frame", rest @ ..]) => {
            serve_ui(core, dev_apps, id, &rest.join("/"), live_reload)
        }
        (Method::Get, ["apps", _id, "__terrane", ..]) => json_error(404, "not found"),
        (Method::Post, ["apps", id, "invoke"]) => {
            invoke(core, dev_apps, id, request, admin_base_url)
        }
        (Method::Get, ["apps", id, "blob", rest @ ..]) => serve_blob(core, id, &rest.join("/")),
        (Method::Get, ["apps", id]) => {
            let exists = core.state().app.apps.contains_key(*id) || dev_apps.find(id).is_some();
            let locale = negotiate_locale(request);
            let (system, app) = shell_i18n_data(core, &locale, Some(id));
            crate::shell::response(
                exists,
                id,
                live_reload,
                premium_url,
                &app_frame_policy(core, dev_apps, request, id),
                &crate::shell::ShellI18n {
                    locale: &locale,
                    dir: terrane_host::i18n::dir_for(&locale),
                    system_messages: &system,
                    app_messages: &app,
                },
            )
        }
        (Method::Get, ["apps", id, rest @ ..]) => {
            serve_bundle_asset(core, dev_apps, id, &rest.join("/"), live_reload)
        }
        _ => json_error(404, "not found"),
    }
}

type WebhookRateMap = BTreeMap<(String, String), (u64, u32)>;

fn webhook_rates() -> &'static Mutex<WebhookRateMap> {
    static RATES: OnceLock<Mutex<WebhookRateMap>> = OnceLock::new();
    RATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn webhook(
    core: &mut terrane_host::HostCore,
    app: &str,
    name: &str,
    token: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    if !rate_allows(app, name) {
        return json_error(429, "rate limit exceeded");
    }
    let mut body = Vec::new();
    if request.as_reader().read_to_end(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    if body.len() > terrane_cap_webhook::BODY_HARD_LIMIT {
        return json_error(413, "webhook body too large");
    }
    let headers = request
        .headers()
        .iter()
        .map(|header| (header.field.to_string(), header.value.as_str().to_string()))
        .collect::<Vec<_>>();
    let body_mime = header_value(request, "Content-Type").map(ToString::to_string);
    let outcome = match terrane_host::ingest_webhook_on_core(
        core,
        terrane_host::WebhookIngestRequest {
            app: app.to_string(),
            name: name.to_string(),
            token: token.to_string(),
            method: request.method().as_str().to_string(),
            headers,
            body,
            body_mime,
        },
    ) {
        Ok(outcome) => outcome,
        Err(e) if e == "not found" => return json_error(404, "not found"),
        Err(e) if e.contains("headers must be <=") => return json_error(431, &e),
        Err(e) if e.contains("body must be <=") => return json_error(413, &e),
        Err(e) => return json_error(400, &e),
    };
    let input = vec![outcome.verb.clone(), outcome.delivery_json];
    let _ = terrane_host::invoke_app_input_checked_with_admin_base_and_source(
        core,
        &outcome.app,
        &input,
        admin_base_url,
        "webhook",
    );
    Response::from_string("").with_status_code(202)
}

fn rate_allows(app: &str, name: &str) -> bool {
    let minute = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() / 60,
        Err(_) => 0,
    };
    let mut rates = webhook_rates().lock().unwrap_or_else(|e| e.into_inner());
    let entry = rates
        .entry((app.to_string(), name.to_string()))
        .or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    if entry.1 >= terrane_cap_webhook::RATE_LIMIT_PER_MINUTE {
        return false;
    }
    entry.1 += 1;
    true
}

/// The catalog plus any dev-scanned apps not yet cataloged (`--apps <dir>`).
fn merged_apps(
    core: &terrane_host::HostCore,
    dev_apps: &crate::dev_apps::DevApps,
) -> terrane_api::AppsResponse {
    let mut response = terrane_host::list_apps(core);
    for summary in dev_apps.summaries() {
        if !response.apps.iter().any(|app| app.id == summary.id) {
            response.apps.push(summary);
        }
    }
    response.apps.sort_by(|a, b| a.id.cmp(&b.id));
    response
}

/// An app's bundle source: the catalog entry, else the dev-apps scan.
fn app_source(
    core: &terrane_host::HostCore,
    dev_apps: &crate::dev_apps::DevApps,
    id: &str,
) -> Option<String> {
    core.state()
        .app
        .apps
        .get(id)
        .and_then(|app| app.source.clone())
        .or_else(|| dev_apps.find(id).map(|app| app.source))
}

fn app_frame_policy(
    core: &terrane_host::HostCore,
    dev_apps: &crate::dev_apps::DevApps,
    request: &Request,
    id: &str,
) -> crate::shell::AppFramePolicy {
    let browser_permissions = app_source(core, dev_apps, id)
        .and_then(|source| terrane_host::read_manifest(std::path::Path::new(&source)).ok())
        .map(|manifest| manifest.browser_permissions)
        .unwrap_or_default()
        .into_iter()
        .filter(|permission| matches!(permission.as_str(), "camera" | "microphone"))
        .collect::<Vec<_>>();
    if browser_permissions.is_empty() {
        return crate::shell::AppFramePolicy::default();
    }
    crate::shell::AppFramePolicy {
        frame_origin: alternate_loopback_origin(request),
        browser_permissions,
    }
}

fn alternate_loopback_origin(request: &Request) -> Option<String> {
    let host = header_value(request, "Host")?;
    let current_host = host_without_port(host)?;
    if !is_loopback_host(current_host) {
        return None;
    }
    let port = host
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())?;
    let current_host = current_host.trim_matches(|c| c == '[' || c == ']');
    let frame_host = if current_host.eq_ignore_ascii_case("localhost") {
        "127.0.0.1"
    } else {
        "localhost"
    };
    Some(format!("http://{frame_host}:{port}"))
}

fn admin_requests(
    core: &terrane_host::HostCore,
    previews: &PreviewStore,
    admin_base_url: &str,
) -> Resp {
    match terrane_host::permission::permission_requests(core, admin_base_url) {
        Ok(mut response) => {
            response
                .requests
                .extend(previews.permission_requests(admin_base_url));
            response
                .requests
                .sort_by(|a, b| a.request_id.cmp(&b.request_id));
            json_ok(&response)
        }
        Err(e) => json_error(400, &e),
    }
}

fn approve_request(
    core: &mut terrane_host::HostCore,
    previews: &mut PreviewStore,
    admin_session: &crate::admin::AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    decide_request(
        DecisionContext {
            core,
            previews,
            admin_session,
            request_id,
            request,
            admin_base_url,
        },
        PreviewStore::approve_permission_request,
        crate::admin::approve_request,
    )
}

fn deny_request(
    core: &mut terrane_host::HostCore,
    previews: &mut PreviewStore,
    admin_session: &crate::admin::AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    decide_request(
        DecisionContext {
            core,
            previews,
            admin_session,
            request_id,
            request,
            admin_base_url,
        },
        PreviewStore::deny_permission_request,
        crate::admin::deny_request,
    )
}

fn cancel_request(
    core: &mut terrane_host::HostCore,
    previews: &mut PreviewStore,
    admin_session: &crate::admin::AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    decide_request(
        DecisionContext {
            core,
            previews,
            admin_session,
            request_id,
            request,
            admin_base_url,
        },
        PreviewStore::cancel_permission_request,
        crate::admin::cancel_request,
    )
}

fn promote_request(
    core: &mut terrane_host::HostCore,
    previews: &mut PreviewStore,
    admin_session: &crate::admin::AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    if previews
        .permission_request(request_id, admin_base_url)
        .is_none()
    {
        return json_error(404, "permission request not found");
    }
    if admin_session.locked() {
        return json_error(403, "local admin is locked");
    }
    let decision = match preview_decision_request(request) {
        Ok(decision) => decision,
        Err(resp) => return resp,
    };
    match previews.promote_permission_request(core, request_id, &decision.app, admin_base_url) {
        Ok(Some(view)) => json_ok(&view),
        Ok(None) => json_error(404, "permission request not found"),
        Err(e) => json_error(400, &e),
    }
}

fn decide_request(
    ctx: DecisionContext<'_>,
    preview_decide: PreviewDecisionFn,
    installed_decide: InstalledDecisionFn,
) -> Resp {
    if ctx
        .previews
        .permission_request(ctx.request_id, ctx.admin_base_url)
        .is_none()
    {
        return installed_decide(
            ctx.core,
            ctx.admin_session,
            ctx.request_id,
            ctx.request,
            ctx.admin_base_url,
        );
    }
    if ctx.admin_session.locked() {
        return json_error(403, "local admin is locked");
    }
    let decision = match preview_decision_request(ctx.request) {
        Ok(decision) => decision,
        Err(resp) => return resp,
    };
    match preview_decide(
        ctx.previews,
        ctx.request_id,
        &decision.reason,
        ctx.admin_base_url,
    ) {
        Ok(Some(view)) => json_ok(&view),
        Ok(None) => json_error(404, "permission request not found"),
        Err(e) => json_error(400, &e),
    }
}

fn preview_decision_request(request: &mut Request) -> Result<PreviewDecisionRequest, Resp> {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return Err(json_error(400, "cannot read request body"));
    }
    if body.trim().is_empty() {
        return Ok(PreviewDecisionRequest {
            reason: String::new(),
            app: String::new(),
        });
    }
    PreviewDecisionRequest::deserialize_json(&body)
        .map_err(|e| json_error(400, &format!("bad decision body: {e}")))
}

/// `GET /apps/{id}/logs?level=&tail=` — owner/local host surface for backend
/// telemetry jsonl. The app frame itself does not call this route.
fn app_logs(id: &str, request: &Request) -> Resp {
    let query = request.url().split_once('?').map(|(_, q)| q).unwrap_or("");
    let level = query_value(query, "level").unwrap_or_default();
    let tail = query_value(query, "tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200);
    match terrane_host::app_log::read_tail(&terrane_host::home_dir(), id, &level, tail) {
        Ok(json) => Response::from_data(json.into_bytes())
            .with_header(header("Content-Type", "application/json")),
        Err(e) => json_error(500, &e.to_string()),
    }
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            percent_decode(v).ok()
        } else {
            None
        }
    })
}

fn is_admin_control_route(method: &Method, segs: &[&str]) -> bool {
    if segs.len() < 2 || segs[0] != "__terrane" || segs[1] != "admin" {
        return false;
    }
    matches!(
        (method, segs),
        (Method::Get, ["__terrane", "admin", "session"])
            | (Method::Post, ["__terrane", "admin", "local", "lock"])
            | (Method::Post, ["__terrane", "admin", "local", "unlock"])
            | (Method::Get, ["__terrane", "admin", "apps"])
            | (Method::Get, ["__terrane", "admin", "grants"])
            | (Method::Get, ["__terrane", "admin", "agents"])
            | (Method::Get, ["__terrane", "admin", "audit"])
            | (Method::Post, ["__terrane", "admin", "agents"])
            | (
                Method::Post,
                ["__terrane", "admin", "agents", _, "delegate"]
            )
            | (Method::Delete, ["__terrane", "admin", "agents", _])
            | (Method::Get, ["__terrane", "admin", "requests"])
            | (
                Method::Post,
                ["__terrane", "admin", "requests", _, "approve"]
            )
            | (Method::Post, ["__terrane", "admin", "requests", _, "deny"])
            | (
                Method::Post,
                ["__terrane", "admin", "requests", _, "cancel"]
            )
            | (
                Method::Post,
                ["__terrane", "admin", "requests", _, "promote"]
            )
            | (Method::Post, ["__terrane", "admin", "grants"])
            | (Method::Delete, ["__terrane", "admin", "grants"])
            | (Method::Post, ["__terrane", "admin", "stt", "open"])
            | (Method::Post, ["__terrane", "admin", "stt", "segment"])
            | (Method::Post, ["__terrane", "admin", "stt", "close"])
    )
}

/// `POST /__terrane/builder/generate` - start generating a draft app in the
/// background and return `{id, status: "running"}` immediately. The harness
/// runs minutes; holding the single-threaded request loop for it would stall
/// every other request. Poll `/__terrane/builder/status` for the draft.
fn builder_generate(
    core: &mut terrane_host::HostCore,
    builder_jobs: &mut crate::builder_jobs::BuilderJobs,
    request: &mut Request,
) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: BuilderGenerateRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad builder body: {e}")),
    };
    let draft_id = parsed.id.trim().to_string();
    let harness = selected_harness(&parsed.harness, &parsed.agent).trim();
    let harness = if harness.is_empty() { "codex" } else { harness };

    // Fail fast on invalid requests: decide-level validation without running
    // the effect. A valid effectful command reports "dryRun unsupported";
    // anything else is a real validation error.
    let args = [
        "--harness".to_string(),
        harness.to_string(),
        draft_id.clone(),
        parsed.id.clone(),
        parsed.name.clone(),
        parsed.prompt.clone(),
    ];
    match terrane_host::dry_run_on_core(core, "harness.generate-app", &args) {
        Err(e) if e.contains("dryRun unsupported") => {}
        Err(e) => return json_error(500, &e),
        Ok(_) => {}
    }

    if !builder_jobs.running(&draft_id) {
        builder_jobs.start(&draft_id, &parsed.id, &parsed.name, harness, &parsed.prompt);
    }
    json_ok(&BuilderJobStatus {
        id: draft_id,
        status: "running".into(),
    })
}

/// `POST /__terrane/builder/status` `{id}` — poll a background generation. The
/// poll that finds the worker finished commits its records through an ordinary
/// `harness.generate-app` dispatch, then returns the draft JSON.
fn builder_status(
    core: &mut terrane_host::HostCore,
    builder_jobs: &mut crate::builder_jobs::BuilderJobs,
    request: &mut Request,
) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: BuilderStatusRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad builder status body: {e}")),
    };

    match builder_jobs.poll(core, parsed.id.trim()) {
        crate::builder_jobs::JobPoll::Running => json_ok(&BuilderJobStatus {
            id: parsed.id.trim().to_string(),
            status: "running".into(),
        }),
        crate::builder_jobs::JobPoll::Done(json) => Response::from_data(json.into_bytes())
            .with_header(header("Content-Type", "application/json")),
        crate::builder_jobs::JobPoll::Failed(e) => json_error(500, &e),
        crate::builder_jobs::JobPoll::Unknown => {
            json_error(404, &format!("no such builder job or draft: {}", parsed.id))
        }
    }
}

fn selected_harness<'a>(harness: &'a str, legacy_agent: &'a str) -> &'a str {
    let harness = harness.trim();
    if harness.is_empty() {
        legacy_agent
    } else {
        harness
    }
}

/// `POST /__terrane/previews` - create an in-memory app preview from files.
fn create_preview(
    core: &terrane_host::HostCore,
    previews: &mut PreviewStore,
    request: &mut Request,
) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: CreatePreviewRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad preview body: {e}")),
    };

    match previews.create_preview(parsed.files, core.state()) {
        Ok(mut response) => {
            response.frame_url = format!("/__terrane/previews/{}/frame/", response.id);
            json_ok(&response)
        }
        Err(e) => json_error(400, &e),
    }
}

/// `POST /__terrane/previews/{id}/invoke` - run the generated preview backend.
fn invoke_preview(
    previews: &mut PreviewStore,
    id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: InvokeRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad invoke body: {e}")),
    };

    match previews.permission_required_with_admin_base(id, admin_base_url) {
        Ok(Some(required)) => {
            let body = required.serialize_json();
            return Response::from_data(body.into_bytes())
                .with_status_code(403)
                .with_header(header("Content-Type", "application/json"));
        }
        Ok(None) => {}
        Err(e) if e.starts_with("no such preview:") => return json_error(404, &e),
        Err(e) => return json_error(400, &e),
    }

    match previews.invoke_backend(id, &parsed.verb, &parsed.args) {
        Ok(output) => json_ok(&InvokeResponse { output }),
        Err(e) if e.starts_with("no such preview:") => json_error(404, &e),
        Err(e) => json_error(500, &e),
    }
}

fn destroy_preview(previews: &mut PreviewStore, id: &str) -> Resp {
    match previews.destroy_preview(id) {
        Ok(()) => Response::from_string("").with_status_code(204),
        Err(e) if e.starts_with("no such preview:") => json_error(404, &e),
        Err(e) => json_error(400, &e),
    }
}

/// `GET /__terrane/previews/{id}/frame/...` - serve generated UI/assets.
fn serve_preview(previews: &PreviewStore, id: &str, rel: &str) -> Resp {
    let asset = match previews.read_asset(id, rel) {
        Ok(asset) => asset,
        Err(e) if e.starts_with("no such preview:") => return json_error(404, &e),
        Err(e) if e.contains("absolute paths") || e.contains("parent-dir") => {
            return json_error(403, &e)
        }
        Err(e) => return json_error(404, &e),
    };
    let content_type = asset.content_type;
    let body = if content_type.starts_with("text/html") {
        inject_preview_shim(asset.content.as_bytes(), id)
    } else {
        asset.content.into_bytes()
    };
    Response::from_data(body)
        .with_header(header("Content-Type", &content_type))
        .with_header(assets_cors_header())
}

/// `POST /mcp` — MCP JSON-RPC over HTTP, backed by the shared host MCP module.
fn mcp(core: &mut terrane_host::HostCore, request: &mut Request) -> Resp {
    if !origin_allowed(request) {
        return json_error(403, "forbidden origin");
    }

    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }

    match terrane_host::mcp::handle_json_rpc_with_source(core, &body, "mcp_http") {
        Some(response) => Response::from_data(response.into_bytes())
            .with_header(header("Content-Type", "application/json")),
        None => Response::from_string("").with_status_code(202),
    }
}

/// `POST /apps/{id}/invoke` — run a verb on the app's backend, return its
/// output. A dev-scanned app is cataloged on its first invoke (the same lazy
/// `app.add` the macOS host performs on selection).
fn invoke(
    core: &mut terrane_host::HostCore,
    dev_apps: &crate::dev_apps::DevApps,
    id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed: InvokeRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad invoke body: {e}")),
    };

    if !core.state().app.apps.contains_key(id) {
        if let Some(dev) = dev_apps.find(id) {
            let args = vec![
                dev.id.clone(),
                dev.name.clone(),
                "--source".to_string(),
                dev.source.clone(),
            ];
            if let Err(e) = terrane_host::dispatch_on_core(core, "app.add", &args) {
                return json_error(500, &format!("cannot catalog dev app {id}: {e}"));
            }
        }
    }

    match terrane_host::invoke_app_checked_with_admin_base_and_source(
        core,
        id,
        &parsed.verb,
        &parsed.args,
        admin_base_url,
        "web",
    ) {
        Ok(output) => json_ok(&InvokeResponse { output }),
        Err(terrane_host::InvokeFailure::PermissionRequired(required)) => {
            let body = required.serialize_json();
            Response::from_data(body.into_bytes())
                .with_status_code(403)
                .with_header(header("Content-Type", "application/json"))
        }
        Err(terrane_host::InvokeFailure::Other(e)) if e.starts_with("no such app:") => {
            json_error(404, &e)
        }
        Err(terrane_host::InvokeFailure::Other(e)) => json_error(500, &e),
    }
}

/// `GET /apps/{id}/blob/{name}` — serve verified CAS bytes for `<img src>`.
fn serve_blob(core: &terrane_host::HostCore, id: &str, encoded_name: &str) -> Resp {
    let name = match percent_decode(encoded_name) {
        Ok(name) => name,
        Err(e) => return json_error(400, &e),
    };
    let granted = terrane_cap_auth::namespace_granted(
        core.state(),
        &terrane_core::ExecutionPrincipal::local_owner(),
        id,
        "blob",
    )
    .map_err(|e| e.to_string());
    match granted {
        Ok(true) => {}
        Ok(false) => return json_error(403, "permission required for blob"),
        Err(e) => return json_error(500, &e),
    }
    let Some(meta) = core
        .state()
        .blob
        .blobs
        .get(id)
        .and_then(|names| names.get(&name))
    else {
        return json_error(404, "blob not found");
    };
    let bytes = match terrane_host::blob_store::read_verified(&terrane_host::home_dir(), &meta.hash)
    {
        Ok(bytes) => bytes,
        Err(e) => return json_error(500, &e.to_string()),
    };
    Response::from_data(bytes)
        .with_header(header("Content-Type", &meta.mime))
        .with_header(header("ETag", &meta.hash))
}

/// `GET /apps/{id}/…` — serve the app's UI (with the invoke shim injected) or a
/// bundle asset. `rel` is the path under the bundle dir (empty = the UI entry).
fn serve_ui(
    core: &mut terrane_host::HostCore,
    dev_apps: &crate::dev_apps::DevApps,
    id: &str,
    rel: &str,
    live_reload: bool,
) -> Resp {
    let Some(source) = app_source(core, dev_apps, id) else {
        return json_error(404, &format!("no such app (or no bundle): {id}"));
    };
    let base = Path::new(&source);

    let Some(ui) = terrane_host::read_manifest_ui(&source) else {
        return json_error(404, "app has no UI");
    };
    let entry = base.join(ui);
    let target = if rel.is_empty() {
        entry
    } else {
        entry.parent().unwrap_or(base).join(rel)
    };

    serve_file(id, base, &target, live_reload)
}

/// `GET /apps/{id}/{rel}` — serve a direct bundle asset path rooted at the app
/// source directory. This keeps `/apps/{id}/dist/...` useful for built bundles
/// while the iframe route resolves relative to `manifest.ui`'s directory.
fn serve_bundle_asset(
    core: &mut terrane_host::HostCore,
    dev_apps: &crate::dev_apps::DevApps,
    id: &str,
    rel: &str,
    live_reload: bool,
) -> Resp {
    let Some(source) = app_source(core, dev_apps, id) else {
        return json_error(404, &format!("no such app (or no bundle): {id}"));
    };
    let base = Path::new(&source);
    let target = base.join(rel);

    serve_file(id, base, &target, live_reload)
}

fn serve_file(id: &str, base: &Path, target: &Path, live_reload: bool) -> Resp {
    let Some(safe) = safe_within(base, target) else {
        return json_error(403, "forbidden");
    };
    let Ok(bytes) = std::fs::read(&safe) else {
        return json_error(404, "not found");
    };

    let ctype = content_type(&safe);
    let body = if ctype.starts_with("text/html") {
        inject_app_shim(&bytes, id, live_reload)
    } else {
        bytes
    };
    Response::from_data(body)
        .with_header(header("Content-Type", ctype))
        .with_header(assets_cors_header())
}

/// The app frame is a sandboxed iframe without `allow-same-origin`, so its
/// origin is opaque (`null`) and `<script type="module">` fetches its assets in
/// CORS mode — unlike classic scripts and stylesheets. Without this header the
/// browser blocks every ES-module asset and module-based apps render blank.
fn assets_cors_header() -> tiny_http::Header {
    header("Access-Control-Allow-Origin", "*")
}

/// Loopback-origin pages (e.g. the Terrane Premium dashboard listing this
/// host's apps) may read the public catalog cross-origin; any other origin
/// gets no CORS grant and the browser blocks the read.
fn with_local_cors(request: &Request, resp: Resp) -> Resp {
    let Some(origin) = header_value(request, "Origin") else {
        return resp;
    };
    if origin_host(origin).is_some_and(is_loopback_host) {
        resp.with_header(header("Access-Control-Allow-Origin", origin))
            .with_header(header("Vary", "Origin"))
    } else {
        resp
    }
}

fn origin_allowed(request: &Request) -> bool {
    let Some(origin) = header_value(request, "Origin") else {
        return true;
    };
    let Some(origin_host) = origin_host(origin) else {
        return false;
    };
    if is_loopback_host(origin_host) {
        return true;
    }
    header_value(request, "Host")
        .and_then(host_without_port)
        .is_some_and(|host| host.eq_ignore_ascii_case(origin_host))
}

fn header_value<'a>(request: &'a Request, field: &str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.to_string().eq_ignore_ascii_case(field))
        .map(|h| h.value.as_str())
}

/// The active locale for a shell render: the `terrane_lang` cookie override if
/// it names a supported code (the in-app language picker sets it), else
/// negotiated from `Accept-Language`, else the default (`en`).
fn negotiate_locale(request: &Request) -> String {
    if let Some(choice) = cookie_value(request, "terrane_lang") {
        if let Some(code) = terrane_host::i18n::canonical(choice.trim()) {
            return code.to_string();
        }
    }
    terrane_host::i18n::from_accept_language(header_value(request, "Accept-Language").unwrap_or(""))
        .to_string()
}

/// Read one cookie value from the request's `Cookie` header.
fn cookie_value(request: &Request, name: &str) -> Option<String> {
    for pair in header_value(request, "Cookie")?.split(';') {
        if let Some((key, value)) = pair.trim().split_once('=') {
            if key.trim() == name {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Build the shell's i18n payload for `app_id` (`None` in admin mode → no app
/// frame bundle). Returns the owned locale + bundles; the caller borrows them
/// into a `ShellI18n` for the render.
fn shell_i18n_data(
    core: &terrane_host::HostCore,
    locale: &str,
    app_id: Option<&str>,
) -> (
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
) {
    let system = terrane_host::i18n::system_bundle(core, locale);
    let app = match app_id {
        Some(id) => terrane_host::i18n::app_bundle(core, locale, id),
        None => std::collections::BTreeMap::new(),
    };
    (system, app)
}

fn origin_host(origin: &str) -> Option<&str> {
    let after_scheme = origin.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())?;
    host_without_port(authority)
}

fn host_without_port(authority: &str) -> Option<&str> {
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    authority
        .split_once(':')
        .map(|(host, _)| host)
        .or(Some(authority))
        .filter(|host| !host.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(|c| c == '[' || c == ']');
    matches!(host, "::1" | "localhost") || host.starts_with("127.")
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err("bad percent escape".into());
                }
                let high = hex(bytes[i + 1]).ok_or_else(|| "bad percent escape".to_string())?;
                let low = hex(bytes[i + 2]).ok_or_else(|| "bad percent escape".to_string())?;
                out.push((high << 4) | low);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "blob name must be utf-8".to_string())
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
