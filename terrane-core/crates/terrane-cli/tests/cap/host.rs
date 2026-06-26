//! e2e for `host` — drive the real `terrane` binary running the real
//! `apps/todo` JS backend in QuickJS. Deterministic + local (no clock / rng /
//! network), so it runs by DEFAULT. The replay assertion is the Option-A
//! contract: the log holds only the kv.* events the backend emitted; `replay`
//! rebuilds the todos by folding them, never by re-running JS.

use std::path::PathBuf;

use tempfile::tempdir;

use crate::helpers::terrane;

/// Absolute path to the repo's `apps/todo` bundle.
fn todo_source() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")) // …/terrane-core/crates/terrane-cli
        .join("../../../apps/todo") // repo-root/apps/todo
        .canonicalize()
        .expect("apps/todo bundle exists")
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn todo_backend_runs_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = todo_source();

    let (ok, out, err) = terrane(home, &["app", "add", "todo", "Todo", "--source", &src]);
    assert!(ok, "app add failed: {err}");
    assert!(out.contains("app.added"), "out: {out}");

    let (ok, out, err) = terrane(home, &["host", "run", "todo", "add", "buy milk"]);
    assert!(ok, "host run add failed: {err}");
    assert_eq!(out.trim(), "added #1 buy milk", "out: {out}");

    let (ok, out, _) = terrane(home, &["host", "run", "todo", "add", "ship it"]);
    assert!(ok);
    assert_eq!(out.trim(), "added #2 ship it", "out: {out}");

    let (ok, out, _) = terrane(home, &["host", "run", "todo", "list"]);
    assert!(ok);
    assert_eq!(out.trim(), "#1 buy milk\n#2 ship it", "out: {out}");

    let (ok, out, _) = terrane(home, &["host", "run", "todo", "done", "1"]);
    assert!(ok);
    assert_eq!(out.trim(), "done #1", "out: {out}");

    let (ok, out, _) = terrane(home, &["host", "run", "todo", "list"]);
    assert!(ok);
    assert_eq!(out.trim(), "#2 ship it", "out: {out}");

    // The log holds only app.added + kv.* — no host.* record (Option A).
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("kv.set todo/seq = 1"), "log: {log}");
    assert!(log.contains("kv.set todo/item:1 = buy milk"), "log: {log}");
    assert!(log.contains("kv.deleted todo/item:1"), "log: {log}");
    assert!(!log.contains("host."), "no host.* in log: {log}");

    // Replay folds kv.* only; QuickJS is never re-entered.
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "out: {out}");
}
