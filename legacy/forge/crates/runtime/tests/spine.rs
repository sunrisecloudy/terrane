//! End-to-end spine tests: TS-shaped JS → QuickJS → capability-checked `ctx`
//! host bridge → effects + seeded seams → `AppResult`.
//!
//! prd-merged/01 CR-1 (zero ambient), CR-3 (ctx namespaces), CR-11 (seams);
//! prd-merged/07 SC-1.

mod common;

use common::{owner, program, spine_manifest, viewer};
use forge_domain::RunOutcome;
use forge_runtime::{record_run, run_once, MemoryHostBridge};

/// The canonical "hello world": a tiny async `main` returns an `AppResult`.
#[test]
fn hello_world_returns_app_result() {
    let prog =
        program("export async function main(ctx, input) { return { ok: true, value: 'hi' }; }");
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        42,
        1000,
        &mut bridge,
    )
    .unwrap();
    assert!(result.ok);
    assert_eq!(result.value, serde_json::json!("hi"));
}

/// `input` is passed to `main(ctx, input)` and is readable.
#[test]
fn input_is_passed_to_main() {
    let prog = program(
        "export async function main(ctx, input) { return { ok: true, value: input.name }; }",
    );
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({"name": "forge"}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(result.value, serde_json::json!("forge"));
}

/// A non-`{ok,value}` return is wrapped as `{ ok: true, value }`.
#[test]
fn bare_return_value_is_wrapped() {
    let prog = program("export async function main(ctx, input) { return 7; }");
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(result.ok);
    assert_eq!(result.value, serde_json::json!(7));
}

/// `ctx.storage.set` then `ctx.storage.get` returns the stored value, and the
/// write is visible to the host bridge (the SQLite-write seam of the spine).
#[test]
fn storage_set_then_get_roundtrips_and_is_capability_checked() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.storage.set("app/note", { title: input.title });
            const got = await ctx.storage.get("app/note");
            return { ok: true, value: got.title };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({"title": "Ship"}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(result.value, serde_json::json!("Ship"));
    // The effect landed in the (test) host bridge.
    assert_eq!(
        bridge.peek_storage("app/note"),
        Some(&serde_json::json!({ "title": "Ship" }))
    );
}

/// A storage write outside the granted scope surfaces `PermissionDenied` as the
/// run outcome (prd-merged/07 SC-8) — the run does not silently succeed.
#[test]
fn storage_write_outside_grant_is_permission_denied() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.storage.set("secret/keys", "leak");
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
    // Nothing was written (the denial happened before the bridge effect).
    assert!(bridge.peek_storage("secret/keys").is_none());
}

/// A read-only role cannot run applet code at all (prd-merged/07 SC-10): even
/// the first host call is denied.
#[test]
fn viewer_role_cannot_run() {
    let prog = program(
        "export async function main(ctx, input) { return { ok: true, value: ctx.time.now() }; }",
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &viewer(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected PermissionDenied for viewer, got {other:?}"),
    }
}

/// `ctx.ui.render` captures a UI tree the host (here a test bridge) can read —
/// the "UI tree patch" stage of the spine.
#[test]
fn ui_render_captures_tree() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.ui.render({ type: "text", value: "rendered:" + input.name });
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({"name": "forge"}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(
        bridge.last_ui().unwrap(),
        &serde_json::json!({ "type": "text", "value": "rendered:forge" })
    );
}

/// `ctx.db.insert` / `ctx.db.get` round-trip a record through the bridge and are
/// capability-checked against the `tasks` collection grant.
#[test]
fn db_insert_and_get_roundtrips() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const id = await ctx.db.insert("tasks", { title: "T1" });
            const back = await ctx.db.get("tasks", id);
            return { ok: true, value: { id, title: back.title } };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(result.value["id"], serde_json::json!("tasks/1"));
    assert_eq!(result.value["title"], serde_json::json!("T1"));
}

/// `ctx.db.query(collection, plan)` runs a structured query against the bridge
/// and returns the matched rows (DL-15). The test bridge applies a single
/// equality leaf, so the applet sees only the rows the plan selects.
#[test]
fn db_query_returns_matched_rows() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.db.insert("tasks", { title: "A", status: "todo" });
            await ctx.db.insert("tasks", { title: "B", status: "done" });
            await ctx.db.insert("tasks", { title: "C", status: "todo" });
            const rows = await ctx.db.query("tasks", {
                from: "tasks",
                where: { field: "status", value: "todo" }
            });
            return { ok: true, value: { count: rows.length, titles: rows.map(r => r.title) } };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(result.value["count"], serde_json::json!(2));
    assert_eq!(result.value["titles"], serde_json::json!(["A", "C"]));
}

/// `ctx.db.query(plan)` — the single-argument overload — resolves the queried
/// collection from the plan's own `from` (DL-15). It must reach the same gated,
/// recorded host call as the two-argument form, returning the matched rows.
#[test]
fn db_query_single_arg_resolves_collection_from_plan() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.db.insert("tasks", { title: "A", status: "todo" });
            await ctx.db.insert("tasks", { title: "B", status: "done" });
            const rows = await ctx.db.query({
                from: "tasks",
                where: { field: "status", value: "todo" }
            });
            return { ok: true, value: { count: rows.length, titles: rows.map(r => r.title) } };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let result = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(result.value["count"], serde_json::json!(1));
    assert_eq!(result.value["titles"], serde_json::json!(["A"]));
}

/// A `ctx.db.query` against an ungranted collection surfaces `PermissionDenied`
/// as the run outcome (CR-3/SC-10): the query needs `db.read` for the queried
/// collection, and no rows are returned.
#[test]
fn db_query_outside_grant_is_denied() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const rows = await ctx.db.query("users", { from: "users" });
            return { ok: true, value: rows };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
    // The denied query is recorded so the denial replays; its response is the
    // recorded denial, not rows.
    let q = rec.calls.iter().find(|c| c.method == "db.query").unwrap();
    assert!(q.response.get("denied").is_some(), "denied query must record a denial");
}

/// `ctx.db` against an ungranted collection is denied.
#[test]
fn db_write_outside_grant_is_denied() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.db.insert("users", { name: "x" });
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match rec.outcome {
        RunOutcome::Failed { error } => assert_eq!(error.code(), "PermissionDenied"),
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
}

/// `ctx.log` lines are captured into the run record (bounded observability).
#[test]
fn logs_are_captured_into_the_run_record() {
    let prog = program(
        r#"export async function main(ctx, input) {
            ctx.log("first");
            ctx.log("second");
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(rec.logs, vec!["first".to_string(), "second".to_string()]);
}

/// `ctx.time.now()` is the seeded logical clock: it starts at `time_start` and
/// advances by one per call (prd-merged/01 CR-11). `ctx.random.next()` is the
/// seeded RNG: same seed ⇒ same first value.
#[test]
fn time_and_random_seams_are_seeded_and_deterministic() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const t0 = ctx.time.now();
            const t1 = ctx.time.now();
            const r0 = ctx.random.next();
            return { ok: true, value: { t0, t1, r0 } };
        }"#,
    );
    let mut b1 = MemoryHostBridge::new();
    let a = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        42,
        1000,
        &mut b1,
    )
    .unwrap();
    assert_eq!(a.value["t0"], serde_json::json!(1000));
    assert_eq!(a.value["t1"], serde_json::json!(1001));

    // Same seed/start ⇒ identical seam values.
    let mut b2 = MemoryHostBridge::new();
    let b = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        42,
        1000,
        &mut b2,
    )
    .unwrap();
    assert_eq!(a.value, b.value);

    // Different seed ⇒ different random.
    let mut b3 = MemoryHostBridge::new();
    let c = run_once(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        43,
        1000,
        &mut b3,
    )
    .unwrap();
    assert_ne!(a.value["r0"], c.value["r0"]);
}

/// The recorded trace lists every host call in order with seeded seam values —
/// the raw material for deterministic replay (prd-merged/01 CR-8/CR-9).
#[test]
fn run_record_captures_ordered_call_trace() {
    let prog = program(
        r#"export async function main(ctx, input) {
            const t = ctx.time.now();
            await ctx.storage.set("app/k", t);
            await ctx.ui.render({ type: "text", value: "ok" });
            return { ok: true, value: null };
        }"#,
    );
    let mut bridge = MemoryHostBridge::new();
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        5,
        100,
        &mut bridge,
    )
    .unwrap();
    let methods: Vec<_> = rec.calls.iter().map(|c| c.method.as_str()).collect();
    assert_eq!(methods, vec!["time.now", "storage.set", "ui.render"]);
    // Seqs are dense and ordered.
    for (i, c) in rec.calls.iter().enumerate() {
        assert_eq!(c.seq, i as u64);
    }
    assert!(rec.is_completed());
    assert_eq!(rec.code_hash, prog.code_hash());
}
