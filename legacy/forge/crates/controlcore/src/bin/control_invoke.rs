//! Stdin/stdout helper for Node reference-host and tooling to invoke `control.*`
//! commands without a full WorkspaceCore database.

use forge_controlcore::dispatch;
use forge_domain::{CoreError, CoreResponse, RequestId};
use serde::Deserialize;
use serde_json::Value;
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
struct InvokeEnvelope {
    request_id: String,
    name: String,
    payload: Value,
}

fn main() {
    let mut input = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input) {
        eprintln!("control-invoke: failed to read stdin: {e}");
        std::process::exit(1);
    }

    let envelope: InvokeEnvelope = match serde_json::from_str(input.trim()) {
        Ok(envelope) => envelope,
        Err(e) => {
            let response = CoreResponse::err(
                RequestId::new("control-invoke"),
                CoreError::ValidationError(format!("stdin is not a valid invoke envelope: {e}")),
            );
            println!("{}", serde_json::to_string(&response).expect("serialize"));
            std::process::exit(1);
        }
    };

    let request_id = RequestId::new(&envelope.request_id);
    let response = match dispatch(&envelope.name, &envelope.payload) {
        Ok(payload) => CoreResponse::ok(request_id, payload),
        Err(error) => CoreResponse::err(request_id, error),
    };

    println!("{}", serde_json::to_string(&response).expect("serialize"));
    if !response.ok {
        std::process::exit(1);
    }
}