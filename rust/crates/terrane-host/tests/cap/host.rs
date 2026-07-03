//! e2e for `host` — drive the real `terrane` binary running real app bundles in
//! QuickJS. Deterministic + local (no clock / rng / network), so it runs by
//! DEFAULT. The CLI lifecycle uses the UI-free `apps/todo-cli`; a smaller smoke
//! exercises `apps/todo`'s UI-facing `items` verb. The replay assertion is the
//! Option-A contract: the log holds only the kv.* events the backend emitted.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::tempdir;

use crate::helpers::terrane;

/// Absolute path to a repo app bundle (`apps/<name>`).
fn app_source(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")) // …/rust/crates/terrane-host
        .join("../../../apps")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|_| panic!("apps/{name} bundle exists"))
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn todo_cli_backend_runs_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("todo-cli");

    let (ok, out, err) = terrane(
        home,
        &["app", "add", "todo-cli", "Todo (CLI)", "--source", &src],
    );
    assert!(ok, "app add failed: {err}");
    assert!(out.contains("app.added"), "out: {out}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "todo-cli", "kv"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo-cli", "add", "buy milk"]);
    assert!(ok, "js-runtime run add failed: {err}");
    assert_eq!(out.trim(), "added #1 buy milk", "out: {out}");

    let (ok, out, _) = terrane(home, &["js-runtime", "run", "todo-cli", "add", "ship it"]);
    assert!(ok);
    assert_eq!(out.trim(), "added #2 ship it", "out: {out}");

    let (ok, out, _) = terrane(home, &["js-runtime", "run", "todo-cli", "list"]);
    assert!(ok);
    assert_eq!(out.trim(), "#1 buy milk\n#2 ship it", "out: {out}");

    let (ok, out, _) = terrane(home, &["js-runtime", "run", "todo-cli", "done", "1"]);
    assert!(ok);
    assert_eq!(out.trim(), "done #1", "out: {out}");

    let (ok, out, _) = terrane(home, &["js-runtime", "run", "todo-cli", "list"]);
    assert!(ok);
    assert_eq!(out.trim(), "#2 ship it", "out: {out}");

    // The log holds app.added + auth.granted + kv.* — no host.* record (Option A).
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("kv.set todo-cli/seq = 1"), "log: {log}");
    assert!(
        log.contains("kv.set todo-cli/item:1 = buy milk"),
        "log: {log}"
    );
    assert!(log.contains("kv.deleted todo-cli/item:1"), "log: {log}");
    assert!(!log.contains("host."), "no host.* in log: {log}");

    // Replay folds kv.* only; QuickJS is never re-entered.
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "out: {out}");
}

/// Smoke the full `apps/todo` bundle's UI-facing `items` verb (the GUI app's
/// data surface) — JSON, no spaces, emits no events.
#[test]
fn todo_app_items_returns_json() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("todo");

    terrane(home, &["app", "add", "todo", "Todo", "--source", &src]);
    terrane(home, &["auth", "grant", "user:local-owner", "todo", "kv"]);
    terrane(home, &["js-runtime", "run", "todo", "add", "buy milk"]);

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "todo", "items"]);
    assert!(ok, "items failed: {err}");
    assert_eq!(
        out.trim(),
        r#"[{"id":1,"text":"buy milk"}]"#,
        "items out: {out}"
    );

    let (ok, _, _) = terrane(home, &["replay"]);
    assert!(ok);
}

/// A backend that never returns must be interrupted by the time budget, not hang
/// the host. (If the DoS guard regresses, this test hangs the suite — which is
/// the loud failure we want.)
#[test]
fn runaway_backend_is_interrupted() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("loop");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "loop", "name":"Loop","runtime":"js","backend":"main.js", "resources": ["kv"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        "function handle(input) { while (true) {} }",
    )
    .unwrap();

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "loop",
            "Loop",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "loop", "kv"]);
    assert!(ok, "auth grant failed: {err}");

    // Short budget so the test is fast; the run must fail, not wedge.
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(["js-runtime", "run", "loop", "go"])
        .env("TERRANE_HOME", home)
        .env("TERRANE_BACKEND_BUDGET_MS", "200")
        .output()
        .expect("spawn terrane");
    assert!(
        !output.status.success(),
        "runaway backend should be interrupted and error, not succeed/hang"
    );
}

/// Drive the real `apps/scribe` bundle through the CLI: the host edge (simulated
/// via trusted `stt open/append`) opens a session and records finalized
/// segments; the scribe backend reads the folded transcript and records a
/// selection. Option A: the log holds only stt.* events — no audio, no ASR, no
/// host.* record — and replay rebuilds identically.
#[test]
fn scribe_app_reads_transcript_and_records_selection() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("scribe");

    let (ok, _, err) = terrane(
        home,
        &["app", "add", "scribe", "Scribe", "--source", &src],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "scribe", "stt"]);
    assert!(ok, "auth grant failed: {err}");

    // Host edge: open a session, append two finalized segments.
    let (ok, out, err) = terrane(
        home,
        &[
            "stt", "open", "scribe", "s1", "host1", "host1", "whisper-tiny", "16000",
        ],
    );
    assert!(ok, "stt open failed: {err}");
    assert!(out.contains("stt.session.opened"), "open out: {out}");
    let (ok, _, err) = terrane(
        home,
        &["stt", "append", "scribe", "s1", "1", "0", "500", "hello"],
    );
    assert!(ok, "stt append 1 failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "stt", "append", "scribe", "s1", "2", "500", "900", "world",
        ],
    );
    assert!(ok, "stt append 2 failed: {err}");

    // Scribe backend reads the folded transcript (no audio, no ASR at replay).
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "scribe", "transcript", "s1"]);
    assert!(ok, "transcript failed: {err}");
    assert_eq!(out.trim(), "hello world", "transcript out: {out}");

    // select() records the slice and returns the re-derived text to the host.
    let (ok, out, err) = terrane(
        home,
        &[
            "js-runtime", "run", "scribe", "select", "s1", "1", "2", "clipboard",
        ],
    );
    assert!(ok, "select failed: {err}");
    assert_eq!(out.trim(), "hello world", "select out: {out}");

    // The log holds only recorded events (Option A): stt.* and the kv.* from
    // app.add/grant — never any audio or host.* artifact.
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("stt.session.opened"), "log: {log}");
    assert!(log.contains("stt.segment.appended"), "log: {log}");
    assert!(log.contains("stt.selection.made"), "log: {log}");
    assert!(!log.contains("host."), "no host.* in log: {log}");

    // Replay folds stt.* only; QuickJS and the edge are never re-entered.
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}
