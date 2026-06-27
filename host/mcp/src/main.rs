//! terrane-mcp — a stdio MCP server exposing this terrane home's apps as tools.
//!
//! A thin host over the `terrane-host` spine, like the CLI and web
//! hosts. It speaks the Model Context Protocol over newline-delimited JSON-RPC on
//! stdin/stdout so an MCP client (e.g. Claude Code) can **select an app**
//! (`list_apps`) and **act on it** (`invoke`). Both tools and their shapes are
//! the contract in [`terrane_api`].
//!
//! Everything is single-threaded and synchronous: one `Core` over `$TERRANE_HOME`,
//! one message at a time — which suits both the non-`Send` `Core` and the stdio
//! transport. stdout is reserved for protocol frames; all logging goes to stderr.

use std::io::{BufRead, Write};

fn main() {
    let mut core = match terrane_host::open() {
        Ok(core) => core,
        Err(e) => {
            eprintln!("terrane-mcp: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "terrane-mcp: ready (home {})",
        terrane_host::log_path().display()
    );

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF — client disconnected.
            Ok(_) => {}
            Err(e) => {
                eprintln!("terrane-mcp: read error: {e}");
                break;
            }
        }
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }
        if let Some(response) = terrane_host::mcp::handle_json_rpc(&mut core, raw) {
            if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    }
}
