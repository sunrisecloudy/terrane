//! terrane-host — the CLI host.
//!
//! A superset of the `terrane` binary: every standard command works (delegated
//! to the shared host CLI adapter), plus a top-level `run <app> [input…]` that
//! executes an app backend via its cataloged runtime. It is the first
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
        // Top-level run: `terrane-host run <app> [input…]`.
        ["run", app, input @ ..] => {
            let mut core = terrane_host::open()?;
            let input = input.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            println!("{}", terrane_host::invoke_app_input(&mut core, app, &input)?);
            Ok(())
        }
        ["run"] => Err("usage: terrane-host run <app> [input…]".into()),
        [] | ["help"] | ["--help"] | ["-h"] => {
            print_host_help();
            terrane_host::cli::print_help();
            Ok(())
        }
        // Everything else is a standard terrane command.
        _ => terrane_host::cli::run(argv),
    }
}

fn print_host_help() {
    println!(
        "terrane-host — the terrane CLI plus the app runtime entry point\n\n\
         \x20 terrane-host run <app> [input…]   run an app backend\n\n\
         All standard terrane commands also work:\n"
    );
}
