//! E2E for `terrane-host app install`: copy a bundle into the home and catalog
//! it from there, so the home owns the app (no dependence on the external path
//! or the working directory). Proven by installing, then running the app from a
//! DIFFERENT cwd — the old `app add --source <relative>` would break here.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

fn host(home: &Path, cwd: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane-host"))
        .args(args)
        .current_dir(cwd)
        .env("TERRANE_HOME", home)
        .output()
        .expect("spawn terrane-host");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn apps_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .canonicalize()
        .unwrap()
}

#[test]
fn install_copies_the_bundle_into_the_home_and_runs_from_anywhere() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let elsewhere_dir = tempdir().unwrap(); // a cwd with no relation to the bundle
    let elsewhere = elsewhere_dir.path();
    let apps = apps_dir();

    // Install — relative bundle path, resolved against this cwd (apps dir).
    let (ok, out, err) = host(home, &apps, &["app", "install", "todo-cli-collaborate"]);
    assert!(ok, "install failed: {err}");
    assert!(out.contains("installed todo-cli-collaborate"), "out: {out}");

    // The bundle now lives inside the home, and the catalog points there.
    assert!(home
        .join("apps/todo-cli-collaborate/manifest.json")
        .exists());
    let (_, state, _) = host(home, elsewhere, &["state"]);
    assert!(
        state.contains(&format!(
            "{}",
            home.join("apps/todo-cli-collaborate").display()
        )),
        "catalog should point into the home: {state}"
    );

    // Run it from an UNRELATED cwd — works because the home owns the bundle.
    let (ok, out, err) = host(
        home,
        elsewhere,
        &["run", "todo-cli-collaborate", "add", "buy milk"],
    );
    assert!(ok, "run failed: {err}");
    assert_eq!(out.trim(), "added: buy milk");
    let (_, out, _) = host(home, elsewhere, &["run", "todo-cli-collaborate", "list"]);
    assert_eq!(out.trim(), "#1 buy milk");

    // The app self-describes via __actions__ (what the MCP app_actions tool uses).
    let (_, out, _) = host(
        home,
        elsewhere,
        &["run", "todo-cli-collaborate", "__actions__"],
    );
    assert!(
        out.contains("\"add\"") && out.contains("\"done\""),
        "actions: {out}"
    );

    // Re-install is a safe no-op on the catalog (data preserved).
    let (ok, out, _) = host(home, &apps, &["app", "install", "todo-cli-collaborate"]);
    assert!(ok && out.contains("refreshed"), "reinstall: {out}");
    let (_, out, _) = host(home, elsewhere, &["run", "todo-cli-collaborate", "list"]);
    assert_eq!(out.trim(), "#1 buy milk", "data survived reinstall");
}

#[test]
fn install_rejects_manifest_id_path_traversal() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let src = dir.path().join("src");
    let victim = dir.path().join("victim");
    fs::create_dir_all(home.join("apps")).unwrap();
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("keep.txt"), "do not delete\n").unwrap();
    fs::write(
        src.join("manifest.json"),
        r#"{ "id": "../../victim", "name": "Bad", "backend": "main.js", "resources": ["kv"] }"#,
    )
    .unwrap();
    fs::write(src.join("main.js"), "function handle() { return 'ok'; }\n").unwrap();

    let (ok, out, err) = host(
        &home,
        dir.path(),
        &["app", "install", src.to_str().unwrap()],
    );
    assert!(!ok, "install should reject traversal id; stdout={out}");
    assert!(
        err.contains("unsafe \"id\""),
        "expected unsafe id error, got stdout={out:?} stderr={err:?}"
    );
    assert_eq!(
        fs::read_to_string(victim.join("keep.txt")).unwrap(),
        "do not delete\n",
        "install must not replace paths outside home/apps"
    );
    assert!(!victim.join("manifest.json").exists());
}
