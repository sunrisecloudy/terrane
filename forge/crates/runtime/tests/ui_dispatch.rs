//! UI event-dispatch loop (prd-merged/05 UI-4, prd-merged/01 CR-6).
//!
//! The interactive keystone: a rendered tree carries `onTap`/`onChange`
//! `ActionRef`s; the renderer sends a ref back with an event payload; the runtime
//! dispatches the applet handler exported under that name over the SAME
//! containment / limits / host path as a normal run, records the dispatch, and a
//! session replays the event sequence byte-identically.
//!
//! Contract (T034): events ARE recorded in the run record (as a
//! `ui.dispatch_event` envelope); replaying the same event yields a byte-identical
//! trace + final tree. Handlers persist state via `ctx.db`/`ctx.storage` only —
//! the realm is one-shot per dispatch.

mod common;

use common::{owner, program, spine_manifest};
use forge_runtime::{
    record_dispatch, record_run, replay_dispatch, HostBridge, MemoryHostBridge, NullBridge,
};

/// An interactive applet (the JS the committed TS fixture
/// `forge/fixtures/ui-events/applet/applet.ts` transpiles to): `main` renders the
/// initial view; `increment`/`decrement` (onTap) and `setLabel` (onChange) are UI
/// event handlers addressed by ActionRef. Each handler reads the persisted state,
/// mutates it, writes it back (state lives in `ctx.storage`, NOT an in-memory
/// global — the realm is one-shot per dispatch), and re-renders. The rendered
/// Button/TextField carry the handler names as their `onTap`/`onChange` ActionRefs,
/// which is exactly the dispatch key.
fn interactive_applet() -> forge_runtime::Program {
    program(
        r#"
        async function readCount(ctx) {
            const raw = await ctx.storage.get("app/count");
            return raw === null ? 0 : Number(raw);
        }
        async function readLabel(ctx) {
            const raw = await ctx.storage.get("app/label");
            return raw === null ? "" : String(raw);
        }
        function view(count, label) {
            return {
                type: "Stack",
                testId: "root",
                direction: "v",
                children: [
                    { type: "Text", testId: "value", text: `Count: ${count}` },
                    { type: "Text", testId: "label", text: `Label: ${label}` },
                    { type: "Button", testId: "inc", label: "+", onTap: "increment" },
                    { type: "Button", testId: "dec", label: "-", onTap: "decrement" },
                    { type: "TextField", testId: "name", value: label, onChange: "setLabel" }
                ]
            };
        }
        export async function main(ctx, input) {
            const count = await readCount(ctx);
            const label = await readLabel(ctx);
            ctx.ui.render(view(count, label));
            return { ok: true, value: { count, label } };
        }
        export async function increment(ctx, event) {
            const next = (await readCount(ctx)) + (event.by ?? 1);
            await ctx.storage.set("app/count", String(next));
            const label = await readLabel(ctx);
            ctx.ui.render(view(next, label));
            ctx.log("incremented");
            return { ok: true, value: view(next, label) };
        }
        export async function decrement(ctx, event) {
            const next = (await readCount(ctx)) - (event.by ?? 1);
            await ctx.storage.set("app/count", String(next));
            const label = await readLabel(ctx);
            ctx.ui.render(view(next, label));
            return { ok: true, value: view(next, label) };
        }
        export async function setLabel(ctx, event) {
            const label = event.value ?? "";
            await ctx.storage.set("app/label", label);
            const count = await readCount(ctx);
            ctx.ui.render(view(count, label));
            return { ok: true, value: view(count, label) };
        }
        "#,
    )
}

/// Dispatching a handler by its ActionRef runs that named function: it reads the
/// persisted state, applies the event payload, writes it back, and renders the new
/// tree — the expected ctx effects + UI render (UI-4/CR-6).
#[test]
fn dispatch_invokes_handler_by_action_ref_with_effects_and_render() {
    let prog = interactive_applet();
    let mut bridge = MemoryHostBridge::new();
    // Seed an existing counter so we can see the handler read-modify-write it.
    bridge.storage_set("app/count", serde_json::json!("4")).unwrap();

    let record = record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "increment",
        &serde_json::json!({ "by": 3 }),
        7,
        100,
        &mut bridge,
    )
    .unwrap();

    assert!(record.is_completed(), "dispatch should complete: {:?}", record.outcome);
    // The handler wrote the new counter through ctx.storage (state persists there,
    // not in an in-memory global): 4 + 3 = 7.
    assert_eq!(bridge.peek_storage("app/count"), Some(&serde_json::json!("7")));
    // It rendered the new view (a ui.render effect was recorded + captured).
    let last = bridge.last_ui().expect("the handler rendered a tree");
    assert_eq!(last["children"][0]["text"], serde_json::json!("Count: 7"));
    // The host-call trace contains the handler's effects AND the dispatch envelope.
    let methods: Vec<&str> = record.calls.iter().map(|c| c.method.as_str()).collect();
    assert!(methods.contains(&"storage.get"), "{methods:?}");
    assert!(methods.contains(&"storage.set"), "{methods:?}");
    assert!(methods.contains(&"ui.render"), "{methods:?}");
    assert!(methods.contains(&"log"), "{methods:?}");
    // The dispatch envelope is the LAST recorded call (recorded after the effects).
    let dispatch = record.calls.last().expect("a recorded call exists");
    assert_eq!(dispatch.method, "ui.dispatch_event");
    assert_eq!(
        dispatch.args,
        serde_json::json!(["increment", { "by": 3 }]),
        "the envelope records (action_ref, payload)"
    );
}

/// A **handler-only applet** (one that exports event handlers but no `main`) can
/// still be dispatched: the realm exposes handlers by ActionRef independent of the
/// entrypoint, so the absence of `main` must not make a dispatch fail to load. (The
/// naive wrap unconditionally bound `globalThis.__forge_main = main`, which threw
/// `ReferenceError: main is not defined` at load for such applets and turned every
/// dispatch into a load failure — UI-4/CR-6.)
#[test]
fn handler_only_applet_without_main_can_be_dispatched() {
    let prog = program(
        r#"
        export async function bump(ctx, event) {
            await ctx.storage.set("app/x", String(event.by ?? 1));
            ctx.ui.render({ type: "Text", testId: "x", text: "bumped" });
            return { ok: true, value: { bumped: true } };
        }
        "#,
    );
    let mut bridge = MemoryHostBridge::new();
    let record = record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "bump",
        &serde_json::json!({ "by": 9 }),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(
        record.is_completed(),
        "a handler-only applet (no main) must dispatch: {:?}",
        record.outcome
    );
    assert_eq!(bridge.peek_storage("app/x"), Some(&serde_json::json!("9")));
}

/// Guarding `__forge_main` behind `typeof main === 'function'` (the handler-only
/// fix above) must NOT mask a genuinely missing `main` on the *run* path: running a
/// program that exports no `main` still fails with the clean
/// "does not export ... main" runtime error, not a silent success.
#[test]
fn run_on_a_main_less_program_still_reports_missing_main() {
    use forge_domain::RunOutcome;

    let prog = program(
        r#"
        export async function bump(ctx, event) { return { ok: true, value: 1 }; }
        "#,
    );
    let mut bridge = MemoryHostBridge::new();
    let record = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    match record.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "RuntimeError", "{error}");
            assert!(error.to_string().contains("main"), "{error}");
        }
        other => panic!("a main-less run must fail with the missing-main error, got {other:?}"),
    }
}

/// Dispatching an unknown ActionRef is a clean, typed engine error
/// (`ValidationError`) — not a panic across the FFI boundary, and not a generic
/// runtime fault. The run record carries the failure.
#[test]
fn unknown_action_ref_is_a_clean_validation_error() {
    use forge_domain::RunOutcome;

    let prog = interactive_applet();
    let mut bridge = MemoryHostBridge::new();
    let record = record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "no_such_handler",
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();

    match record.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "ValidationError", "{error}");
            assert!(error.to_string().contains("no_such_handler"), "{error}");
        }
        other => panic!("unknown action ref must fail typed, got {other:?}"),
    }
}

/// Record-then-replay of a single event is byte-identical (the jewel's
/// interactive link). The recorder serves the recorded `ctx.*` responses (the live
/// bridge is a `NullBridge` that refuses every effect) and the recorded
/// `ui.dispatch_event` envelope, so the replay reproduces the exact trace +
/// outcome. `replay_fingerprint` therefore covers the dispatched event.
#[test]
fn record_then_replay_of_a_single_event_is_byte_identical() {
    let prog = interactive_applet();

    let mut bridge = MemoryHostBridge::new();
    let original = record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "increment",
        &serde_json::json!({ "by": 2 }),
        42,
        1000,
        &mut bridge,
    )
    .unwrap();
    assert!(original.is_completed());
    // The dispatched event is part of the recorded trace.
    assert!(
        original.calls.iter().any(|c| c.method == "ui.dispatch_event"),
        "the event must be recorded"
    );

    // Replay against a NullBridge: the recorder serves the recording, never the
    // live bridge, yet the replay is byte-identical.
    let mut null = NullBridge::new();
    let replayed = replay_dispatch(&original, &prog, &spine_manifest(), &owner(), &mut null).unwrap();

    assert!(
        original.replays_identically(&replayed),
        "event replay must be byte-identical:\n original={:#?}\n replayed={:#?}",
        original.calls,
        replayed.calls
    );
    assert_eq!(original.calls, replayed.calls);
    assert_eq!(original.outcome, replayed.outcome);
    // The fingerprint covers the dispatched event (same event → same fingerprint).
    assert_eq!(original.replay_fingerprint(), replayed.replay_fingerprint());
}

/// Tampering with the recorded dispatch envelope so it replays a *different*
/// event (a different action_ref) diverges with a determinism `RuntimeError`: the
/// recorder asserts the live `(action_ref, payload)` match the recording.
#[test]
fn replaying_a_diverging_event_is_a_determinism_error() {
    use forge_domain::RunOutcome;

    let prog = interactive_applet();
    let mut bridge = MemoryHostBridge::new();
    let original = record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "increment",
        &serde_json::json!({ "by": 1 }),
        3,
        0,
        &mut bridge,
    )
    .unwrap();

    // Tamper: rewrite the recorded dispatch envelope's action_ref to `decrement`.
    // `replay_dispatch` recovers the (tampered) action_ref from the envelope, so it
    // re-runs `decrement` while the recorder still expects `increment` at the
    // cursor → divergence.
    let mut tampered = original.clone();
    let dispatch = tampered
        .calls
        .iter_mut()
        .find(|c| c.method == "ui.dispatch_event")
        .unwrap();
    dispatch.args = serde_json::json!(["decrement", { "by": 1 }]);

    let mut null = NullBridge::new();
    let diverged =
        replay_dispatch(&tampered, &prog, &spine_manifest(), &owner(), &mut null).unwrap();
    match diverged.outcome {
        RunOutcome::Failed { error } => {
            assert_eq!(error.code(), "RuntimeError");
            assert!(error.to_string().contains("divergence"), "{error}");
        }
        other => panic!("a diverging event must be a determinism error, got {other:?}"),
    }
}

/// A `TextField.onChange` ActionRef dispatches its handler too (not just
/// `Button.onTap`): `setLabel` reads the change event's `value`, persists it, and
/// re-renders. Proves the dispatch addressing is by ActionRef name regardless of
/// which control fired it.
#[test]
fn on_change_action_ref_dispatches_its_handler() {
    let prog = interactive_applet();
    let mut bridge = MemoryHostBridge::new();
    let record = record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "setLabel",
        &serde_json::json!({ "value": "Ada" }),
        5,
        0,
        &mut bridge,
    )
    .unwrap();
    assert!(record.is_completed(), "{:?}", record.outcome);
    assert_eq!(bridge.peek_storage("app/label"), Some(&serde_json::json!("Ada")));
    let last = bridge.last_ui().expect("the handler rendered a tree");
    // children[1] is the label Text; the TextField (children[4]) carries onChange.
    assert_eq!(last["children"][1]["text"], serde_json::json!("Label: Ada"));
    assert_eq!(last["children"][4]["onChange"], serde_json::json!("setLabel"));
}

/// The committed TS fixture is the human-facing contract artifact (matching the
/// `forge/fixtures/e2e/*` shape): it binds `Button.onTap` / `TextField.onChange`
/// to handler names that are the dispatch ActionRefs. Assert it carries both
/// binding kinds keyed by the handler names the runtime dispatches, so the fixture
/// and the engine's handler registry stay in lockstep.
#[test]
fn fixture_applet_binds_on_tap_and_on_change_action_refs() {
    let ts = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/ui-events/applet/applet.ts"
    ))
    .expect("read the interactive UI-events fixture applet");
    // onTap bound to the increment/decrement handlers (exported by the fixture).
    assert!(ts.contains(r#"onTap: "increment""#), "fixture must bind onTap to increment");
    assert!(ts.contains(r#"onTap: "decrement""#), "fixture must bind onTap to decrement");
    assert!(ts.contains("export async function increment"));
    assert!(ts.contains("export async function decrement"));
    // onChange bound to the setLabel handler (exported by the fixture).
    assert!(ts.contains(r#"onChange: "setLabel""#), "fixture must bind onChange to setLabel");
    assert!(ts.contains("export async function setLabel"));
}

/// Two sequential dispatches each run in a FRESH (one-shot) realm: the second
/// handler only sees the first's effect because state was persisted through
/// `ctx.storage`, not an in-memory global. This is the contract that makes the
/// loop deterministic across dispatches.
#[test]
fn state_persists_across_dispatches_only_through_storage() {
    let prog = interactive_applet();
    let mut bridge = MemoryHostBridge::new();

    // First event: increment by 5 from the default 0 → 5.
    record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "increment",
        &serde_json::json!({ "by": 5 }),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(bridge.peek_storage("app/count"), Some(&serde_json::json!("5")));

    // Second event on the SAME bridge: decrement by 2. A fresh realm is built, so
    // the only way it reads "5" is via ctx.storage — proving state did not survive
    // in-memory between the two one-shot realms.
    record_dispatch(
        &prog,
        &spine_manifest(),
        &owner(),
        "decrement",
        &serde_json::json!({ "by": 2 }),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    assert_eq!(bridge.peek_storage("app/count"), Some(&serde_json::json!("3")));
}
