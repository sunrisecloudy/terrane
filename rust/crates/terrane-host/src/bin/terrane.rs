//! terrane — the CLI front door for the host crate.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let outcome = terrane_host::cli::run(&argv);
    // Cached inference engines must be dropped before ggml's static
    // destructors run, or the process aborts at exit.
    terrane_host::local_llm_shutdown();
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("terrane: {msg}");
            ExitCode::FAILURE
        }
    }
}
