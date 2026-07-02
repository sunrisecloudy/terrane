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
mod args;
mod dev_apps;
mod home;
mod http;
mod live_reload;
mod routes;
mod shell;
mod shim;
mod static_files;

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

    let mut core = match terrane_host::open() {
        Ok(core) => core,
        Err(e) => {
            eprintln!("terrane-web: {e}");
            std::process::exit(1);
        }
    };

    let server = match Server::http(&args.addr) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("terrane-web: cannot bind {}: {e}", args.addr);
            std::process::exit(1);
        }
    };
    let admin_base_url = format!("http://{}", server.server_addr());
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
    for mut request in server.incoming_requests() {
        let response = routes::route(
            &mut core,
            routes::RouteState {
                previews: &mut previews,
                admin_session: &mut admin_session,
            },
            &mut request,
            routes::RouteConfig {
                require_auth,
                token: token.as_deref(),
                live_reload: args.live_reload,
                admin_base_url: &admin_base_url,
                dev_apps: &dev_apps,
            },
        );
        let _ = request.respond(response);
    }
}
