//! `forge` — the M0a CLI harness (prd-merged/06 PS-5).
//!
//! Minimal arg parsing over [`forge_cli`]: today the one real subcommand is
//! `demo`, which drives the whole executable spine and asserts deterministic
//! replay (prd-merged/09 M0a exit). The process exits non-zero if the run failed
//! or replay diverged, so CI can gate on the spine staying green.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let cmd = args.next();

    match cmd.as_deref() {
        Some("demo") => run_demo(),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("forge: unknown subcommand {other:?}\n");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn run_demo() -> ExitCode {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match forge_cli::demo(&mut out) {
        Ok(outcome) if outcome.run_ok && outcome.replay_identical => ExitCode::SUCCESS,
        Ok(outcome) => {
            eprintln!(
                "forge demo: spine assertion failed (run_ok={}, replay_identical={})",
                outcome.run_ok, outcome.replay_identical
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("forge demo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!("forge — the Terrane M0a spine harness (prd-merged/06 PS-5)");
    println!();
    println!("USAGE:");
    println!("    forge <command>");
    println!();
    println!("COMMANDS:");
    println!("    demo    Run the notes-lite applet end to end (TS → SQLite → UI →");
    println!("            deterministic replay) and print the result. Exits non-zero");
    println!("            if the run fails or replay is not byte-identical.");
    println!("    help    Show this message.");
}
