//! terrane-web — serve a terrane home's apps over HTTP.
//!
//! A thin host over the `terrane-host` spine, like the CLI and MCP
//! hosts. It implements the [`terrane_api`] HTTP contract with `tiny_http`
//! (blocking, single-threaded — one `Core`, one request at a time, which suits
//! the non-`Send` `Core`). It serves app UIs and accepts invokes, injecting a
//! `window.terrane.invoke` shim so an app runs unchanged on the web that runs in
//! the macOS webview.
//!
//! Usage: `terrane-web [--addr 127.0.0.1:8780] [--no-live-reload]`. Loopback
//! binds need no auth; a non-loopback bind requires `TERRANE_WEB_TOKEN` and an
//! `Authorization: Bearer <token>` header on every request. Live reload is on
//! by default and injects a small polling hook into served HTML.

mod admin;
mod agent_jobs;
mod agents;
mod args;
mod builder_jobs;
mod dev_apps;
mod home;
mod http;
mod live_reload;
mod routes;
mod shell;
mod shim;
mod static_files;
mod stt;

use tiny_http::Server;

fn main() {
    let args = args::parse_args();
    let require_auth = !args::is_loopback(&args.addr);
    let token = std::env::var("TERRANE_WEB_TOKEN").ok();
    if require_auth && token.as_deref().map(str::is_empty).unwrap_or(true) {
        eprintln!(
            "terrane-web: a non-loopback bind ({}) requires TERRANE_WEB_TOKEN",
            args.addr
        );
        std::process::exit(1);
    }

    let staging = terrane_host::HarnessStaging::default();
    let mut core = match terrane_host::open_with_staging(staging.clone()) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("terrane-web: {e}");
            std::process::exit(1);
        }
    };
    agents::seed_defaults(&mut core);

    // Seed the shared i18n catalog into the public KV bucket so apps localize
    // out of the box. The catalog root holds `i18n/system` and `apps/<id>/i18n`
    // — prefer $TERRANE_I18N_DIR, else the dev-apps dir's parent, else the CWD.
    // Idempotent (diff-based) and best-effort: a missing catalog is a silent
    // skip and apps just fall back to English.
    let i18n_root = std::env::var_os("TERRANE_I18N_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            args.apps_dir
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(std::path::Path::to_path_buf)
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    match terrane_host::seed_public_i18n(&mut core, &i18n_root) {
        Ok(outcome) if outcome.entries > 0 => eprintln!("terrane-web: {}", outcome.message()),
        Ok(_) => {}
        Err(e) => eprintln!("terrane-web: i18n seed skipped: {e}"),
    }

    let server = match Server::http(&args.addr) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("terrane-web: cannot bind {}: {e}", args.addr);
            std::process::exit(1);
        }
    };
    let listen_addr = server.server_addr().to_string();
    let admin_base_url = format!("http://{listen_addr}");
    if let Err(e) = stt::init(&listen_addr) {
        eprintln!("terrane-web: stt init failed: {e}");
        std::process::exit(1);
    }
    let dev_apps = dev_apps::DevApps::new(args.apps_dir.clone());
    eprintln!(
        "terrane-web: serving {} on http://{} (auth: {}, live reload: {}{})",
        terrane_host::log_path().display(),
        server.server_addr(),
        if require_auth {
            "bearer token"
        } else {
            "off (loopback)"
        },
        if args.live_reload { "on" } else { "off" },
        if dev_apps.enabled() {
            format!(", dev apps: {}", dev_apps.dir_display())
        } else {
            String::new()
        }
    );

    let mut previews = terrane_host::PreviewStore::new();
    let mut admin_session = admin::AdminSessionState::default();
    let mut builder_jobs = builder_jobs::BuilderJobs::new(staging);
    let mut agent_jobs = agent_jobs::AgentJobs::new();
    for mut request in server.incoming_requests() {
        let response = routes::route(
            &mut core,
            routes::RouteState {
                previews: &mut previews,
                admin_session: &mut admin_session,
                builder_jobs: &mut builder_jobs,
                agent_jobs: &mut agent_jobs,
            },
            &mut request,
            routes::RouteConfig {
                require_auth,
                token: token.as_deref(),
                live_reload: args.live_reload,
                admin_base_url: &admin_base_url,
                dev_apps: &dev_apps,
                premium_url: args.premium_url.as_deref(),
            },
        );
        let _ = request.respond(response);
    }
}
