//! terrane-host — the CLI host.
//!
//! A superset of the `terrane` binary: every standard command works (delegated
//! to the `terrane-cli` adapter), plus a top-level `run <app> [input…]` that
//! executes an app's JS backend via the core's `host.run`. It is the first
//! concrete "host" — the same spine a native shell will wrap, minus the UI.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("terrane-host: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(argv: &[&str]) -> Result<(), String> {
    match argv {
        // Top-level run: `terrane-host run <app> [input…]` → host.run.
        ["run", app, input @ ..] => {
            let mut args = Vec::with_capacity(1 + input.len());
            args.push(*app);
            args.extend_from_slice(input);
            terrane_cli::dispatch("host.run", &args)
        }
        ["run"] => Err("usage: terrane-host run <app> [input…]".into()),
        [] | ["help"] | ["--help"] | ["-h"] => {
            print_host_help();
            terrane_cli::print_help();
            Ok(())
        }
        // Everything else is a standard terrane command (incl. `host run …`).
        _ => terrane_cli::run(argv),
    }
}

fn print_host_help() {
    println!(
        "terrane-host — the terrane CLI plus the app runtime entry point\n\n\
         \x20 terrane-host run <app> [input…]   run an app's JS backend\n\n\
         All standard terrane commands also work:\n"
    );
}
