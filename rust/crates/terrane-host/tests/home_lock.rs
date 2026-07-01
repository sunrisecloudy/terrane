//! The home lock rejects a *second process* opening a home a live `Core` holds,
//! and frees it on drop. This is the invariant that makes an in-session grant
//! visible without a restart: a stray `terrane auth grant` in another terminal
//! must fail rather than fork the state. Complements the in-process sharing test
//! in `terrane-core/tests/durability.rs`. Tests live in their own file.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

/// Run `terrane state` (which opens the home's Core) as a separate process.
fn terrane_state(home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_terrane"))
        .arg("state")
        .env("TERRANE_HOME", home)
        .output()
        .expect("spawn terrane")
}

#[test]
fn second_process_is_rejected_while_home_is_open_then_freed() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");

    // Hold the home open in-process, as the live MCP server does.
    let core = terrane_host::open_at_home(&home).unwrap();

    let blocked = terrane_state(&home);
    assert!(
        !blocked.status.success(),
        "a second process must fail while the home is held"
    );
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("another terrane process holds"),
        "expected the home-lock message on stderr, got: {stderr}"
    );

    // Release the guard; the same home is now openable by another process.
    drop(core);
    let freed = terrane_state(&home);
    assert!(
        freed.status.success(),
        "a second process should succeed once the home is released: {}",
        String::from_utf8_lossy(&freed.stderr)
    );
}
