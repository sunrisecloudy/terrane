//! terrane — the CLI front door.
//!
//! The CLI is a thin arg parser: it builds a `Command`, hands it to
//! terrane-core, and renders the result. It never touches data directly.
//!
//! Scaffold only — argument parsing and command dispatch arrive with the first
//! vertical slice.

fn main() {
    println!("terrane: scaffold (no commands yet)");
}
