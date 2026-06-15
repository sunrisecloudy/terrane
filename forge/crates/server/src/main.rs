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
            "--help" | "-h" => {
                println!(
                    "usage: forge-server [--bind 127.0.0.1:8787] [--workspace path] [--workspace-id id]"
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

    let server = match workspace_path {
        Some(path) => ForgeServer::open(path, workspace_id)?,
        None => ForgeServer::in_memory(workspace_id)?,
    };
    eprintln!("forge-server listening on http://{bind}");
    serve_blocking(&bind, &server)
}
