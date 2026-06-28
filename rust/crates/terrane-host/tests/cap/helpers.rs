//! Shared fixtures for the per-capability e2e tests.

use std::path::Path;
use std::process::Command;

/// Run the built `terrane` binary with `args` against `home`; capture the result.
pub(crate) fn terrane(home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(args)
        .env("TERRANE_HOME", home)
        .output()
        .expect("spawn terrane");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// True if `bin` can be spawned (i.e. is installed and on PATH).
pub(crate) fn on_path(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}
