//! Stdin/stdout helper for Node reference-host and tooling to invoke Forge commands.

use forge_core::WorkspaceCore;
use forge_domain::{CoreCommand, CoreResponse};
use std::io::{self, Read};
use std::path::PathBuf;

fn workspace_path(workspace_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "forge-core-invoke-{workspace_id}.sqlite"
    ))
}

fn main() {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("core-invoke: failed to read stdin: {e}");
        std::process::exit(1);
    }

    let command: CoreCommand = match serde_json::from_str(input.trim()) {
        Ok(command) => command,
        Err(e) => {
            let response = CoreResponse::err(
                forge_domain::RequestId::new("core-invoke"),
                forge_domain::CoreError::ValidationError(format!(
                    "stdin is not a valid CoreCommand: {e}"
                )),
            );
            println!("{}", serde_json::to_string(&response).expect("serialize"));
            std::process::exit(1);
        }
    };

    let workspace_id = command.workspace_id.as_str().to_string();
    let path = workspace_path(&workspace_id);
    let response: CoreResponse = match WorkspaceCore::open(&path, workspace_id) {
        Ok(mut core) => core.handle(command),
        Err(error) => CoreResponse::err(forge_domain::RequestId::new("core-invoke"), error),
    };

    println!("{}", serde_json::to_string(&response).expect("serialize"));
    if !response.ok {
        std::process::exit(1);
    }
}