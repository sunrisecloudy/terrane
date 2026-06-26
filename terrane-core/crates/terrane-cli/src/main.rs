//! terrane — the CLI front door: a thin wrapper over the `terrane_cli` spine.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match terrane_cli::run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("terrane: {msg}");
            ExitCode::FAILURE
        }
    }
}
