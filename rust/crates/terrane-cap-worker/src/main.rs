use std::env;
use std::io::{stdin, stdout};

use terrane_cap_worker::{serve, Worker};

fn main() {
    if let Err(error) = run() {
        eprintln!("terrane capability worker failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut namespace = None;
    let mut version = None;
    let mut manifest_sha256 = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--namespace" => namespace = Some(value),
            "--version" => version = Some(value),
            "--manifest-sha256" => manifest_sha256 = Some(value),
            other => return Err(format!("unknown worker argument: {other}")),
        }
    }
    let mut worker = Worker::new(
        namespace.ok_or_else(|| "--namespace is required".to_string())?,
        version.ok_or_else(|| "--version is required".to_string())?,
        manifest_sha256.ok_or_else(|| "--manifest-sha256 is required".to_string())?,
    )
    .map_err(|error| error.to_string())?;
    serve(&mut worker, stdin(), stdout()).map_err(|error| error.to_string())
}
