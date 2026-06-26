use forge_domain::CoreError;
use forge_server::{serve_blocking, ForgeServer};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CoreError> {
    let mut bind = "127.0.0.1:8787".to_string();
    let mut workspace_id = "default".to_string();
    let mut workspace_path: Option<PathBuf> = None;
    let mut auth_token = std::env::var("TERRANE_FORGE_SERVER_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty());
    let mut console_enabled = true;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                bind = args.next().ok_or_else(|| {
                    CoreError::ValidationError("--bind requires an address".into())
                })?;
            }
            "--workspace-id" => {
                workspace_id = args.next().ok_or_else(|| {
                    CoreError::ValidationError("--workspace-id requires a value".into())
                })?;
            }
            "--workspace" => {
                let path = args.next().ok_or_else(|| {
                    CoreError::ValidationError("--workspace requires a path".into())
                })?;
                workspace_path = Some(PathBuf::from(path));
            }
            "--auth-token" => {
                auth_token = Some(args.next().ok_or_else(|| {
                    CoreError::ValidationError("--auth-token requires a value".into())
                })?);
            }
            "--no-console" => {
                console_enabled = false;
            }
            "--help" | "-h" => {
                println!(
                    "usage: forge-server [--bind 127.0.0.1:8787] [--workspace path] [--workspace-id id] [--auth-token token] [--no-console]"
                );
                return Ok(());
            }
            other => {
                return Err(CoreError::ValidationError(format!(
                    "unknown forge-server argument {other:?}"
                )))
            }
        }
    }

    if bind_requires_auth(&bind) && auth_token.is_none() {
        return Err(CoreError::PermissionDenied(
            "non-loopback forge-server binds require --auth-token or TERRANE_FORGE_SERVER_TOKEN"
                .into(),
        ));
    }

    let server = match workspace_path {
        Some(path) => ForgeServer::open(path, workspace_id)?,
        None => ForgeServer::in_memory(workspace_id)?,
    };
    let server = server.serve_console(console_enabled);
    let server = match auth_token {
        Some(token) => server.require_auth_token(token)?,
        None => server,
    };
    eprintln!(
        "forge-server listening on http://{bind} ({}{})",
        if bind_requires_auth(&bind) {
            "auth required"
        } else {
            "loopback"
        },
        if console_enabled {
            ", console at /console"
        } else {
            ""
        }
    );
    serve_blocking(&bind, &server)
}

fn bind_requires_auth(bind: &str) -> bool {
    let host = bind
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(bind)
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');

    !matches!(host, "127.0.0.1" | "localhost" | "::1")
}

#[cfg(test)]
mod tests {
    use super::bind_requires_auth;

    #[test]
    fn loopback_binds_do_not_require_auth_token() {
        assert!(!bind_requires_auth("127.0.0.1:8787"));
        assert!(!bind_requires_auth("localhost:8787"));
        assert!(!bind_requires_auth("[::1]:8787"));
    }

    #[test]
    fn non_loopback_binds_require_auth_token() {
        assert!(bind_requires_auth("0.0.0.0:8787"));
        assert!(bind_requires_auth("192.168.1.10:8787"));
        assert!(bind_requires_auth("[::]:8787"));
    }
}
