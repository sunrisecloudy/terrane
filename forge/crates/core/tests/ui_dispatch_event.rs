//! Data-driven proof of the `ui.dispatch_event` command against the T034 golden
//! vectors (`forge/fixtures/ui-events/*.json`).
//!
//! prd-merged/05 UI-4, prd-merged/01 CR-6, CR-8. The keystone interactive loop
//! through the facade: a rendered control carried an `onTap`/`onChange`
//! `ActionRef`; the renderer sends that ref back with an event payload; the
//! `ui.dispatch_event` command re-enters the applet's handler over the same QuickJS
//! containment / capability gate / record path as `runtime.run`, captures the new
//! UI tree, diffs it against the applet's last-known tree to a patch, emits a
//! `ui.patch` event, records the event into the run/session record, and returns
//! `{ action_ref, tree, patches }`.
//!
//! Each vector is driven END TO END through the real `WorkspaceCore`:
//!   1. install a "vector player" applet (a generic interactive applet whose
//!      handlers render a tree queued by `main` from the run input);
//!   2. `runtime.run` to render the vector's `initial_tree` (the diff base, the
//!      initial render of the interactive session);
//!   3. dispatch the vector's event sequence through `ui.dispatch_event`;
//!   4. assert the produced patch sequence and/or the final tree match the vector,
//!      and the rejection vectors are clean typed rejections with state unchanged.
//!
//! A GUARD asserts the number of vectors actually run equals the manifest `count`
//! (12), so a new/renamed/dropped vector can never silently skip coverage.

use forge_core::{AppletLifecycle, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, WorkspaceId};
use forge_ui::{apply, diff, from_str, Node};

// ---------------------------------------------------------------------------
// The "vector player" applet
// ---------------------------------------------------------------------------
//
// A generic interactive applet that lets the harness reproduce ANY vector's
// behavior without hand-writing 12 applets. `main(ctx, input)` renders the
// vector's `initial_tree` (passed as `input.tree`) and queues the per-event next
// trees (`input.queue[i]`) into `ctx.storage`. Each accepted dispatch pops the
// next queued tree (by a cursor in storage), optionally does a `ctx.db` write
// (the db-write-before-render vector), and renders it. The rejection handlers
// throw / validate so the error vectors are exercised through the same command.
//
// Handlers are addressed by their REAL ActionRef — the exact string each vector's
// `events[].action` carries, including dotted/suffixed refs. Because those refs
// (`counter.increment`, `profile.name.change`, `todo.toggle`) are NOT valid bare
// JS identifiers, the applet exports a `handlers` OBJECT keyed by the free-form
// ActionRef string (the engine folds its keys into the dispatch registry, UI-4):
//   * `counter.increment`    — step: render the next queued tree (counter / seq /
//                              replay vectors);
//   * `profile.name.change`  — validate the change payload's `value` is a string
//                              (rejects the invalid-payload vector), else step;
//   * `tasks.add`            — db.insert a `tasks` record, then step (the
//                              db-write-before-render vector);
//   * `todo.toggle`          — step; addressed by the suffixed ref `todo.toggle:b`
//                              (the engine strips `:b` and merges it as
//                              `event.actionSuffix`), so the handler can read which
//                              list item fired (list-item-by-stable-key vector);
//   * `noop`                 — step (the identical-tree empty-patch vector);
//   * `explode`              — throw "boom" (the handler-throws vector).
// `counter.delete_everything` is intentionally ABSENT so the unknown-action vector
// resolves to a typed ValidationError.
const PLAYER_TS: &str = r#"
    async function cursor(ctx) {
        const raw = await ctx.storage.get("app/cursor");
        return raw === null ? 0 : Number(raw);
    }
    async function nextTree(ctx) {
        const i = await cursor(ctx);
        const raw = await ctx.storage.get("app/queue/" + i);
        await ctx.storage.set("app/cursor", String(i + 1));
        return raw === null ? null : JSON.parse(raw);
    }
    async function step(ctx) {
        const tree = await nextTree(ctx);
        ctx.ui.render(tree);
        return { ok: true, value: tree };
    }
    export async function main(ctx, input) {
        // Queue the per-event next trees, reset the cursor, render the initial tree.
        const queue = (input && input.queue) ? input.queue : [];
        for (let i = 0; i < queue.length; i++) {
            await ctx.storage.set("app/queue/" + i, JSON.stringify(queue[i]));
        }
        await ctx.storage.set("app/cursor", "0");
        ctx.ui.render(input.tree);
        return { ok: true, value: input.tree };
    }
    // Free-form ActionRef registry: keys are the exact refs the vectors declare.
    export const handlers = {
        "counter.increment": async (ctx, _event) => step(ctx),
        "noop": async (ctx, _event) => step(ctx),
        "todo.toggle": async (ctx, _event) => step(ctx),
        "tasks.add": async (ctx, event) => {
            await ctx.db.insert("tasks", { title: (event && event.title) ? event.title : "", done: false });
            return step(ctx);
        },
        "profile.name.change": async (ctx, event) => {
            if (typeof event.value !== "string") {
                throw new Error("invalid event payload: value must be a string");
            }
            return step(ctx);
        },
        "explode": async (_ctx, _event) => {
            throw new Error("boom");
        },
    };
"#;

/// A permissive manifest (db + storage + ui + the player's needs) so the player's
/// handlers can write KV, insert a `tasks` record, and render.
fn player_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": ["app/*"], "write": ["app/*"] },
            "db": { "read": ["tasks", "notes"], "write": ["tasks", "notes"] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000,
            "fuel": 10000000,
            "memory_bytes": 67108864,
            "max_host_calls": 10000,
            "storage_bytes": 10485760,
            "log_bytes": 262144
        }
    })
}

fn owner() -> ActorContext {
    ActorContext::owner("dev")
}

fn cmd(name: &str, applet_id: &str, payload: serde_json::Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("r1"),
        actor: owner(),
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: Some(AppletId::new(applet_id)),
        name: name.into(),
        payload,
    }
}

/// Install the player applet into `core` under `applet_id`.
fn install_player(core: &mut WorkspaceCore, applet_id: &str) {
    let resp = core.handle(cmd(
        "applet.install",
        applet_id,
        serde_json::json!({
            "manifest": player_manifest(),
            "sources": { "src/main.ts": PLAYER_TS }
        }),
    ));
    assert!(resp.ok, "player install must succeed: {:?}", resp.error);
}

/// Render the vector's `initial_tree` (and queue the per-event next trees) via
/// `runtime.run` — the initial render of the interactive session that establishes
/// the diff base for the first event.
fn render_initial(
    core: &mut WorkspaceCore,
    applet_id: &str,
    initial_tree: &serde_json::Value,
    queue: &[serde_json::Value],
) {
    let resp = core.handle(cmd(
        "runtime.run",
        applet_id,
        serde_json::json!({ "input": { "tree": initial_tree, "queue": queue } }),
    ));
    assert!(resp.ok, "initial render must succeed: {:?}", resp.error);
}

/// Dispatch one UI event through `ui.dispatch_event`.
fn dispatch(
    core: &mut WorkspaceCore,
    applet_id: &str,
    action_ref: serde_json::Value,
    event_payload: serde_json::Value,
) -> forge_domain::CoreResponse {
    core.handle(cmd(
        "ui.dispatch_event",
        applet_id,
        serde_json::json!({ "action_ref": action_ref, "event_payload": event_payload }),
    ))
}

/// The per-event expected next trees, computed by applying each event's expected
/// patches to the running tree (UI-1 round-trip: `apply(diff)` reconstructs the
/// tree). `trees[0]` is the initial tree; `trees[i+1]` is the tree after event `i`.
/// Also returns the per-event expected patch lists (from the vector).
fn expected_trees_and_patches(
    initial: &Node,
    results: &[serde_json::Value],
) -> (Vec<Node>, Vec<Vec<forge_ui::Patch>>) {
    let mut trees = vec![initial.clone()];
    let mut patch_lists = Vec::new();
    for result in results {
        let patches_json = result.get("patches").cloned().unwrap_or(serde_json::json!([]));
        let patches: Vec<forge_ui::Patch> =
            serde_json::from_value(patches_json).expect("vector patches deserialize");
        let mut next = trees.last().unwrap().clone();
        apply(&mut next, &patches).expect("vector patches apply to the running tree");
        trees.push(next);
        patch_lists.push(patches);
    }
    (trees, patch_lists)
}

/// Drive a `dispatch`/`replay`-kind vector through the command and assert the
/// produced patch sequence and final tree match the vector. Returns nothing; a
/// mismatch panics with the vector name.
fn run_dispatch_vector(name: &str, vector: &serde_json::Value) {
    let applet_id = format!("vec.{name}");
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, &applet_id);

    let initial_json = &vector["initial_tree"];
    let initial: Node = from_str(&initial_json.to_string()).expect("initial_tree parses");
    let events = vector["events"].as_array().expect("events array");

    // The per-event expected patch lists. A `dispatch` vector lists them under
    // `expect.results[i].patches`; a `replay` vector lists the whole sequence under
    // `expect.first_run.patches` (one patch list per event). Normalize both into a
    // `[{ "patches": [...] }, ...]` shape so the rest of the harness is uniform.
    let results: Vec<serde_json::Value> = if let Some(arr) = vector["expect"]["results"].as_array()
    {
        arr.clone()
    } else if let Some(seq) = vector["expect"]["first_run"]["patches"].as_array() {
        seq.iter()
            .map(|patches| serde_json::json!({ "patches": patches }))
            .collect()
    } else {
        panic!("{name}: vector has neither expect.results nor expect.first_run.patches");
    };
    assert_eq!(events.len(), results.len(), "{name}: one result per event");

    // The per-event next trees (the player's queued renders) + the vector's
    // expected patch lists.
    let (trees, expected_patches) = expected_trees_and_patches(&initial, &results);
    let queue: Vec<serde_json::Value> = trees[1..]
        .iter()
        .map(|t| serde_json::to_value(t).unwrap())
        .collect();

    render_initial(&mut core, &applet_id, initial_json, &queue);

    let is_replay_kind = vector["kind"] == serde_json::json!("replay");
    let mut run_ids: Vec<String> = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let payload = event.get("payload").cloned().unwrap_or(serde_json::json!({}));
        // Dispatch the event's OWN ActionRef (the real, possibly dotted/suffixed
        // ref the vector declares — e.g. `counter.increment`, `tasks.add`,
        // `todo.toggle:b`), NOT a generic harness handler. The applet's `handlers`
        // object registers each of these; a suffixed ref resolves to its base.
        let action_ref = event["action"].clone();
        let resp = dispatch(&mut core, &applet_id, action_ref.clone(), payload);
        assert!(resp.ok, "{name} event #{i} must dispatch: {:?}", resp.error);

        // The command echoes back the SAME (real) action_ref it dispatched.
        assert_eq!(
            resp.payload["action_ref"], action_ref,
            "{name} event #{i}: the command dispatches the vector's own ActionRef"
        );
        run_ids.push(resp.payload["run_id"].as_str().unwrap().to_string());
        let produced_patches: Vec<forge_ui::Patch> =
            serde_json::from_value(resp.payload["patches"].clone()).unwrap();

        // The produced patch list equals the diff from the prior tree to this one —
        // and equals the vector's authored patch list for this event (the contract).
        let want = diff(Some(&trees[i]), &trees[i + 1]);
        assert_eq!(
            produced_patches, want,
            "{name} event #{i}: produced patches must equal diff(prev, next)"
        );
        assert_eq!(
            produced_patches, expected_patches[i],
            "{name} event #{i}: produced patches must equal the vector's expected patches"
        );

        // The returned tree is the new last-known tree (this event's render).
        let produced_tree: Node =
            from_str(&resp.payload["tree"].to_string()).expect("produced tree parses");
        assert_eq!(produced_tree, trees[i + 1], "{name} event #{i}: tree advances");
    }

    // The final tree (when the vector pins one) matches the accumulated tree.
    if let Some(final_tree_json) = vector["expect"].get("final_tree") {
        if !final_tree_json.is_null() {
            let final_tree: Node = from_str(&final_tree_json.to_string()).unwrap();
            assert_eq!(
                *trees.last().unwrap(),
                final_tree,
                "{name}: accumulated final tree matches the vector"
            );
            // And the command persisted it as the last-known diff base: a no-op
            // re-dispatch (rendering the SAME tree) yields an empty patch.
            // (We verify the empty-patch property below for the noop vector.)
        }
    }

    // FIX 3 — the `db_write_then_render` vector pins a `db_writes` entry; assert the
    // handler actually recorded the `db.insert` (collection + fields) into the run's
    // trace, so the db-write-BEFORE-render contract is exercised, not just the patch.
    // (The recorded STORAGE id is the runtime's deterministic `tasks/<n>`; the
    // vector's `task-1` is the applet's own logical row id baked into the rendered
    // tree — a distinct concern, so we assert the fields, not the id string.)
    if let Some(db_writes) = vector["expect"]["results"][0]["db_writes"].as_array() {
        let want = &db_writes[0];
        let saved = core
            .store()
            .load_run(&run_ids[0])
            .unwrap()
            .expect("the db-write dispatch run was saved");
        let insert = saved
            .calls
            .iter()
            .find(|c| c.method == "db.insert")
            .unwrap_or_else(|| panic!("{name}: the run trace must record a db.insert"));
        assert_eq!(
            insert.args[0], want["collection"],
            "{name}: db.insert targets the vector's collection"
        );
        assert_eq!(
            insert.args[1], want["fields"],
            "{name}: db.insert records the vector's pinned fields"
        );
        assert!(
            insert
                .response
                .as_str()
                .map(|id| !id.is_empty())
                .unwrap_or(false),
            "{name}: db.insert recorded a deterministic id in the trace"
        );
    }

    // A `ui.patch` event was emitted per accepted dispatch (UI-1/UI-4 link).
    let patch_events = core.events().events_of_kind("ui.patch").count();
    // initial run renders once (1) + one per accepted event.
    assert!(
        patch_events >= events.len(),
        "{name}: a ui.patch event per accepted dispatch"
    );

    // A `replay`-kind vector additionally proves the recorded event sequence
    // replays byte-identically (CR-8): each dispatch run carries its event in the
    // trace, and `runtime.replay` of that run is asserted `replays_identically`.
    if is_replay_kind {
        for (i, run_id) in run_ids.iter().enumerate() {
            let replay = core.handle(cmd(
                "runtime.replay",
                &applet_id,
                serde_json::json!({ "run_id": run_id }),
            ));
            assert!(replay.ok, "{name} event #{i} run must replay: {:?}", replay.error);
            assert_eq!(
                replay.payload["replays_identically"],
                serde_json::json!(true),
                "{name} event #{i}: the dispatched event replays byte-identically"
            );
        }
    }
}

/// Drive an `error`-kind vector: the dispatch must be a clean typed rejection with
/// the applet's state + last-known tree UNCHANGED (no patch emitted).
fn run_error_vector(name: &str, vector: &serde_json::Value) {
    let applet_id = format!("vec.{name}");
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, &applet_id);

    let initial_json = &vector["initial_tree"];
    render_initial(&mut core, &applet_id, initial_json, &[]);

    // Snapshot the patch-event count so we can prove a rejected event emits none.
    let patches_before = core.events().events_of_kind("ui.patch").count();

    let event = &vector["events"][0];
    let expect = &vector["expect"]["results"][0];
    let want_code = expect["error"]["code"].as_str();

    // Every error vector is now driven by the event's OWN ActionRef + payload (the
    // real ref the vector declares), so the rejection is exercised through the exact
    // dispatch the contract describes — not a stand-in harness handler:
    //   * `unknown_action_rejected`  — `counter.delete_everything` (absent from the
    //                                  registry → ValidationError carrying the ref);
    //   * `invalid_payload_rejected` — `profile.name.change` with `value: 42` (the
    //                                  handler rejects the non-string payload);
    //   * `handler_throws_…`         — `explode` (the handler throws "boom");
    //   * `suspended_applet_rejected`— `counter.increment`, rejected by the lifecycle
    //                                  gate BEFORE the handler runs (suspended).
    let action_ref = event["action"].clone();
    let payload = event
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let suspended = name == "suspended_applet_rejected";

    if suspended {
        core.set_applet_lifecycle(&applet_id, AppletLifecycle::Suspended)
            .unwrap();
        assert_eq!(
            core.applet_lifecycle(&applet_id).unwrap(),
            AppletLifecycle::Suspended
        );
    }

    let resp = dispatch(&mut core, &applet_id, action_ref, payload);
    assert!(!resp.ok, "{name}: a rejection vector must NOT succeed");
    let err = resp.error.expect("a typed error");

    // The error is the right family (the vector pins a renderer-facing code; we map
    // it to a CoreError kind). Unknown-action and suspended-applet are rejected by
    // the COMMAND before/at dispatch → typed `ValidationError`. Invalid-payload and
    // a handler throw are rejected INSIDE the handler (a JS `throw`), which the
    // engine surfaces as a `RuntimeError` — still a clean typed rejection that the
    // run record captures, never a panic.
    match name {
        "handler_throws_prior_tree_intact" | "invalid_payload_rejected" => {
            assert_eq!(err.code(), "RuntimeError", "{name}: {err}");
        }
        _ => {
            assert_eq!(err.code(), "ValidationError", "{name}: {err}");
        }
    }

    // The CONTRACT's renderer-facing code (`expect.results[0].error.code`) is NOT
    // discarded: the command surfaces it VERBATIM on the rejection event it emits
    // (`ui.dispatch_failed` for a post-dispatch failure, `ui.dispatch_event.rejected`
    // for the pre-dispatch suspended gate). All four pinned codes are produced exactly
    // by the command — no remapping:
    //   - `ui.action_not_found`        — unknown handler (engine resolve marker)
    //   - `ui.applet_not_dispatchable` — suspended lifecycle gate
    //   - `ui.invalid_event_payload`   — a handler throw carrying the `invalid
    //         event payload` marker (the payload-validation rejection)
    //   - `runtime.handler_error`      — any OTHER handler throw
    let want_code = want_code.expect("every error vector pins expect.results[0].error.code");
    let want_msg = expect["error"]["message_contains"]
        .as_str()
        .expect("every error vector pins expect.results[0].error.message_contains");

    // Find the rejection event the command emitted and read its renderer-facing
    // code + message. The suspended gate emits the spec-canonical
    // `ui.dispatch_event.rejected` (dispatch_attempted == false); every other
    // rejection emits `ui.dispatch_failed` (dispatch_attempted == true).
    let (reject_kind, want_attempted) = if name == "suspended_applet_rejected" {
        ("ui.dispatch_event.rejected", false)
    } else {
        ("ui.dispatch_failed", true)
    };
    let reject_event = core
        .events()
        .events_of_kind(reject_kind)
        .last()
        .unwrap_or_else(|| panic!("{name}: the command must emit a {reject_kind} event"));
    assert_eq!(
        reject_event.payload["code"], serde_json::json!(want_code),
        "{name}: the rejection event carries the contract's pinned renderer-facing code verbatim"
    );
    assert_eq!(
        reject_event.payload["dispatch_attempted"],
        serde_json::json!(want_attempted),
        "{name}: dispatch_attempted reflects whether the handler ran"
    );
    let event_msg = reject_event.payload["message"].as_str().unwrap_or("");
    assert!(
        event_msg.contains(want_msg),
        "{name}: the rejection message must contain {want_msg:?}, got {event_msg:?}"
    );
    // The same marker is in the typed transport error too.
    assert!(
        err.to_string().contains(want_msg),
        "{name}: the typed error must contain {want_msg:?}, got {err}"
    );

    // Per the contract: no ui.patch was emitted (tree/state unchanged).
    let patches_after = core.events().events_of_kind("ui.patch").count();
    assert_eq!(
        patches_before, patches_after,
        "{name}: a rejected event must emit no ui.patch (tree unchanged)"
    );
}

/// The `no_handler_event_ignored` vector: an event with a NULL action ref (a
/// control with no handler) is a safe ignored no-op — not an error, no patch.
fn run_no_handler_vector(name: &str, vector: &serde_json::Value) {
    let applet_id = format!("vec.{name}");
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, &applet_id);
    render_initial(&mut core, &applet_id, &vector["initial_tree"], &[]);

    let patches_before = core.events().events_of_kind("ui.patch").count();
    // action is null in the vector.
    let resp = dispatch(
        &mut core,
        &applet_id,
        serde_json::Value::Null,
        serde_json::json!({}),
    );
    assert!(resp.ok, "{name}: a null-action event is a safe no-op, not an error");
    assert_eq!(resp.payload["ignored"], serde_json::json!(true), "{name}");
    assert_eq!(
        resp.payload["patches"],
        serde_json::json!([]),
        "{name}: an ignored event produces no patches"
    );
    let patches_after = core.events().events_of_kind("ui.patch").count();
    assert_eq!(patches_before, patches_after, "{name}: no ui.patch on an ignored event");
}

/// Read the `forge/fixtures/ui-events` directory and its manifest.
fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/ui-events")
}

#[test]
fn t034_ui_event_dispatch_vectors_drive_through_the_command() {
    let dir = fixtures_dir();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let want_count = manifest["count"].as_u64().expect("manifest count") as usize;

    let mut ran = 0usize;
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .filter(|p| p.file_name().map(|n| n != "manifest.json").unwrap_or(false))
        .collect();
    entries.sort();

    for path in entries {
        let vector: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let name = vector["case"].as_str().unwrap().to_string();
        let kind = vector["kind"].as_str().unwrap();

        match kind {
            "dispatch" | "replay" if name == "no_handler_event_ignored" => {
                run_no_handler_vector(&name, &vector)
            }
            "dispatch" | "replay" => run_dispatch_vector(&name, &vector),
            "error" => run_error_vector(&name, &vector),
            other => panic!("unknown vector kind {other} in {}", path.display()),
        }
        ran += 1;
    }

    // GUARD: every vector ran. The manifest pins the count (12) so a new/renamed
    // /dropped vector can never silently skip coverage.
    assert_eq!(
        ran, want_count,
        "ran {ran} vectors but the manifest declares {want_count}"
    );
}

// ---------------------------------------------------------------------------
// Focused unit-style coverage of the command's loop semantics (beyond the
// data-driven vectors), proving the diff base advances and replay is identical.
// ---------------------------------------------------------------------------

/// Two sequential dispatches accumulate state through `ctx.storage` (the realm is
/// one-shot per dispatch), and each event's patch diffs against the PRIOR event's
/// tree — the loop's diff base advances across dispatches.
#[test]
fn sequential_dispatches_advance_the_diff_base() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.seq");

    let initial = serde_json::json!({
        "type": "Stack", "direction": "v",
        "children": [ { "type": "Text", "testId": "c", "text": "0" } ]
    });
    let t1 = serde_json::json!({
        "type": "Stack", "direction": "v",
        "children": [ { "type": "Text", "testId": "c", "text": "1" } ]
    });
    let t2 = serde_json::json!({
        "type": "Stack", "direction": "v",
        "children": [ { "type": "Text", "testId": "c", "text": "2" } ]
    });
    render_initial(&mut core, "vec.seq", &initial, &[t1.clone(), t2.clone()]);

    let r1 = dispatch(&mut core, "vec.seq", serde_json::json!("counter.increment"), serde_json::json!({}));
    assert!(r1.ok);
    // First event diffs against the initial tree → update_text "0" -> "1" at [0].
    let p1: Vec<forge_ui::Patch> = serde_json::from_value(r1.payload["patches"].clone()).unwrap();
    let want1 = diff(Some(&from_str(&initial.to_string()).unwrap()), &from_str(&t1.to_string()).unwrap());
    assert_eq!(p1, want1);

    let r2 = dispatch(&mut core, "vec.seq", serde_json::json!("counter.increment"), serde_json::json!({}));
    assert!(r2.ok);
    // Second event diffs against t1 (NOT the initial), so it is "1" -> "2".
    let p2: Vec<forge_ui::Patch> = serde_json::from_value(r2.payload["patches"].clone()).unwrap();
    let want2 = diff(Some(&from_str(&t1.to_string()).unwrap()), &from_str(&t2.to_string()).unwrap());
    assert_eq!(p2, want2);
}

/// The dispatched event is RECORDED in the run/session record (T034 contract): the
/// saved run carries a `ui.dispatch_event` envelope, and replaying that run is
/// byte-identical (the same event sequence reproduces the same trace + outcome).
#[test]
fn dispatched_event_is_recorded_and_replays_identically() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.rec");

    let initial = serde_json::json!({ "type": "Text", "testId": "t", "text": "a" });
    let t1 = serde_json::json!({ "type": "Text", "testId": "t", "text": "b" });
    render_initial(&mut core, "vec.rec", &initial, std::slice::from_ref(&t1));

    let resp = dispatch(&mut core, "vec.rec", serde_json::json!("counter.increment"), serde_json::json!({}));
    assert!(resp.ok, "{:?}", resp.error);
    let run_id = resp.payload["run_id"].as_str().unwrap().to_string();

    // The saved run carries the dispatched event in its recorded trace.
    let saved = core.store().load_run(&run_id).unwrap().expect("run saved");
    assert!(
        saved.calls.iter().any(|c| c.method == "ui.dispatch_event"),
        "the event must be recorded in the run/session record"
    );

    // Replaying the saved dispatch run is byte-identical (the runtime replay path).
    let replay = core.handle(cmd(
        "runtime.replay",
        "vec.rec",
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(replay.ok, "dispatch run must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// V3: deterministic UI event-SESSION replay (UI-4/CR-6, CR-8). A recorded
// sequence of [initial run + N dispatched events] replays as ONE unit to a
// byte-identical composite session fingerprint, per-event patches, and final
// tree (`runtime.replay_session`).
// ---------------------------------------------------------------------------

/// Render the vector player's initial tree and return the `run_id` of that
/// initial `runtime.run` (the head of the session).
fn render_initial_run_id(
    core: &mut WorkspaceCore,
    applet_id: &str,
    initial_tree: &serde_json::Value,
    queue: &[serde_json::Value],
) -> String {
    let resp = core.handle(cmd(
        "runtime.run",
        applet_id,
        serde_json::json!({ "input": { "tree": initial_tree, "queue": queue } }),
    ));
    assert!(resp.ok, "initial render must succeed: {:?}", resp.error);
    resp.payload["run_id"].as_str().unwrap().to_string()
}

/// Record a multi-event interactive session, replay the WHOLE ordered sequence
/// via `runtime.replay_session`, and assert the composite session fingerprint
/// matches AND every per-event patch is byte-identical (V3 #1).
#[test]
fn recorded_event_session_replays_byte_identically() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.session");

    // A 3-event session: text "0" -> "1" -> "2" -> "3".
    let trees: Vec<serde_json::Value> = (0..=3)
        .map(|n| {
            serde_json::json!({
                "type": "Stack", "testId": "root", "direction": "v",
                "children": [ { "type": "Text", "testId": "c", "text": n.to_string() } ]
            })
        })
        .collect();
    let queue = &trees[1..]; // the per-event next trees

    // Record the session: initial run + 3 dispatched `step` events.
    let initial_run_id = render_initial_run_id(&mut core, "vec.session", &trees[0], queue);
    let mut session = vec![initial_run_id];
    let mut recorded_patches: Vec<serde_json::Value> = Vec::new();
    for _ in 0..3 {
        let resp = dispatch(&mut core, "vec.session", serde_json::json!("counter.increment"), serde_json::json!({}));
        assert!(resp.ok, "event must dispatch: {:?}", resp.error);
        session.push(resp.payload["run_id"].as_str().unwrap().to_string());
        recorded_patches.push(resp.payload["patches"].clone());
    }

    // Replay the WHOLE session as one unit.
    let replay = core.handle(cmd(
        "runtime.replay_session",
        "vec.session",
        serde_json::json!({ "run_ids": session }),
    ));
    assert!(replay.ok, "session replay must succeed: {:?}", replay.error);

    // `replays_identically: true` is the SERVER-SIDE byte-identity claim: the
    // command itself asserts the per-run trace fingerprints, the ordered per-event
    // patch chain, AND the converged final tree all reproduce exactly (it errors
    // otherwise), so this flag is load-bearing, not something only the test checks.
    assert_eq!(
        replay.payload["replays_identically"],
        serde_json::json!(true),
        "the recorded event session must replay byte-identically"
    );

    // Every per-event patch the command re-derived (and already verified equal to
    // the recorded chain server-side) also equals the LIVE recorded patch here —
    // i.e. the command's derivation matches the live `ui.dispatch_event` loop.
    let replayed_patches = replay.payload["event_patches"].as_array().unwrap();
    assert_eq!(replayed_patches.len(), recorded_patches.len(), "one patch per event");
    for (i, (got, want)) in replayed_patches.iter().zip(&recorded_patches).enumerate() {
        assert_eq!(got, want, "event #{i}: replayed patch must equal the recorded patch");
    }

    // The session converged to the same final tree (text "3").
    let final_tree: Node = from_str(&replay.payload["final_tree"].to_string()).unwrap();
    let want_final: Node = from_str(&trees[3].to_string()).unwrap();
    assert_eq!(final_tree, want_final, "the session converges to the recorded final tree");

    // A `session.replayed` observability event was emitted.
    assert_eq!(core.events().events_of_kind("session.replayed").count(), 1);

    // Replaying the SAME session again is still identical (idempotent / stable).
    let again = core.handle(cmd(
        "runtime.replay_session",
        "vec.session",
        serde_json::json!({ "run_ids": replay.payload["run_ids"].clone() }),
    ));
    assert!(again.ok);
    assert_eq!(
        again.payload["session_fingerprint"],
        replay.payload["session_fingerprint"],
        "session fingerprint is stable across replays"
    );
}

/// Two events apply in RECORDED ORDER deterministically: replaying the SAME
/// run_ids in the recorded order reproduces the recorded per-event patch chain +
/// final tree, while a SWAPPED order produces an observably DIFFERENT diff-base
/// walk (different per-event patches, different final tree). Each run still
/// self-replays, but the ORDERED session output is order-sensitive — that is what
/// "two events apply in recorded order deterministically" means (V3 #2 order).
#[test]
fn session_replay_is_order_sensitive_over_the_patch_chain() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.order");

    // Two DISTINCT events so the ordered diff-base walk is observable: "a" -> "b" -> "c".
    let t0 = serde_json::json!({ "type": "Text", "testId": "t", "text": "a" });
    let t1 = serde_json::json!({ "type": "Text", "testId": "t", "text": "b" });
    let t2 = serde_json::json!({ "type": "Text", "testId": "t", "text": "c" });
    let initial_run_id = render_initial_run_id(&mut core, "vec.order", &t0, &[t1, t2.clone()]);

    let e1 = dispatch(&mut core, "vec.order", serde_json::json!("counter.increment"), serde_json::json!({}));
    let e2 = dispatch(&mut core, "vec.order", serde_json::json!("counter.increment"), serde_json::json!({}));
    let id1 = e1.payload["run_id"].as_str().unwrap().to_string();
    let id2 = e2.payload["run_id"].as_str().unwrap().to_string();

    // In recorded order the session replays identically and ends at "c" (t2).
    let ordered = core.handle(cmd(
        "runtime.replay_session",
        "vec.order",
        serde_json::json!({ "run_ids": [initial_run_id.clone(), id1.clone(), id2.clone()] }),
    ));
    assert!(ordered.ok, "{:?}", ordered.error);
    assert_eq!(ordered.payload["replays_identically"], serde_json::json!(true));
    let ordered_final: Node = from_str(&ordered.payload["final_tree"].to_string()).unwrap();
    assert_eq!(ordered_final, from_str(&t2.to_string()).unwrap(), "recorded order ends at t2 (\"c\")");

    // Swapping the two events is a DIFFERENT ordered session: every run still
    // self-replays (so it succeeds), but the diff-base walk — and therefore the
    // per-event patch chain AND the final tree — differ from the recorded order.
    // This proves the ORDER is load-bearing and deterministic, not interchangeable.
    let swapped = core.handle(cmd(
        "runtime.replay_session",
        "vec.order",
        serde_json::json!({ "run_ids": [initial_run_id, id2, id1] }),
    ));
    assert!(swapped.ok, "each run self-replays regardless of order: {:?}", swapped.error);
    assert_ne!(
        swapped.payload["event_patches"], ordered.payload["event_patches"],
        "a swapped event order produces a DIFFERENT per-event patch chain"
    );
    let swapped_final: Node = from_str(&swapped.payload["final_tree"].to_string()).unwrap();
    assert_eq!(
        swapped_final,
        from_str(&t1_text("b").to_string()).unwrap(),
        "swapped order ends at the now-last event's tree (\"b\"), not \"c\""
    );
    assert_ne!(swapped_final, ordered_final, "the ordered and swapped sessions converge to different trees");
}

/// Helper: a single-Text tree carrying `text`, matching the player's queued shape.
fn t1_text(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "Text", "testId": "t", "text": text })
}

/// Edge: an event whose ActionRef is absent from the applet's handler registry is
/// a typed `ValidationError` no-op — the applet's last-known tree + state are
/// UNCHANGED, and NO run is recorded for the rejected event (V3 #2 unknown ref).
#[test]
fn unknown_action_ref_is_typed_noop_state_unchanged() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.unknown");

    let t0 = serde_json::json!({ "type": "Text", "testId": "t", "text": "a" });
    let t1 = serde_json::json!({ "type": "Text", "testId": "t", "text": "b" });
    let initial_run_id = render_initial_run_id(&mut core, "vec.unknown", &t0, std::slice::from_ref(&t1));

    let patches_before = core.events().events_of_kind("ui.patch").count();
    // No handler named "nope" is exported.
    let resp = dispatch(&mut core, "vec.unknown", serde_json::json!("nope"), serde_json::json!({}));
    assert!(!resp.ok, "an absent ActionRef must be rejected");
    let err = resp.error.unwrap();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("no UI handler registered"), "{err}");

    // State unchanged: no ui.patch emitted, the cursor was NOT advanced — so a
    // SUBSEQUENT valid `step` still produces the FIRST queued tree ("a" -> "b"),
    // proving the rejected event did not consume the queue or move the diff base.
    assert_eq!(
        patches_before,
        core.events().events_of_kind("ui.patch").count(),
        "a rejected unknown-ref event emits no ui.patch (tree unchanged)"
    );
    let ok = dispatch(&mut core, "vec.unknown", serde_json::json!("counter.increment"), serde_json::json!({}));
    assert!(ok.ok, "{:?}", ok.error);
    let tree: Node = from_str(&ok.payload["tree"].to_string()).unwrap();
    assert_eq!(tree, from_str(&t1.to_string()).unwrap(), "state was not advanced by the rejected event");

    // The valid event records a session of [initial, valid] that replays identically.
    let session = serde_json::json!({
        "run_ids": [initial_run_id, ok.payload["run_id"].as_str().unwrap()]
    });
    let replay = core.handle(cmd("runtime.replay_session", "vec.unknown", session));
    assert!(replay.ok, "{:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}

/// Edge: a handler that THROWS is a typed runtime error and the prior tree is
/// intact — no patch, the diff base does not advance, and the next valid event
/// still diffs against the pre-throw tree (V3 #2 throwing handler).
#[test]
fn throwing_handler_leaves_prior_tree_intact() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.throw");

    let t0 = serde_json::json!({ "type": "Text", "testId": "t", "text": "0" });
    let t1 = serde_json::json!({ "type": "Text", "testId": "t", "text": "1" });
    render_initial(&mut core, "vec.throw", &t0, std::slice::from_ref(&t1));

    let patches_before = core.events().events_of_kind("ui.patch").count();
    let resp = dispatch(&mut core, "vec.throw", serde_json::json!("explode"), serde_json::json!({}));
    assert!(!resp.ok, "a throwing handler must be a typed error");
    assert_eq!(resp.error.unwrap().code(), "RuntimeError");
    assert_eq!(
        patches_before,
        core.events().events_of_kind("ui.patch").count(),
        "a throwing handler emits no ui.patch (prior tree intact)"
    );

    // The prior tree is the diff base: the next valid `step` diffs t0 -> t1.
    let ok = dispatch(&mut core, "vec.throw", serde_json::json!("counter.increment"), serde_json::json!({}));
    assert!(ok.ok, "{:?}", ok.error);
    let produced: Vec<forge_ui::Patch> = serde_json::from_value(ok.payload["patches"].clone()).unwrap();
    let want = diff(Some(&from_str(&t0.to_string()).unwrap()), &from_str(&t1.to_string()).unwrap());
    assert_eq!(produced, want, "the next event diffs against the pre-throw tree");
}

/// Edge: a handler that renders an IDENTICAL tree produces an EMPTY patch (no
/// spurious diff), and that empty-patch event still replays identically in a
/// session (V3 #2 identical-tree empty patch).
#[test]
fn identical_tree_yields_empty_patch_and_replays() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.same");

    // The queued "next" tree is byte-identical to the initial tree → empty patch.
    let same = serde_json::json!({ "type": "Text", "testId": "t", "text": "same" });
    let initial_run_id = render_initial_run_id(&mut core, "vec.same", &same, std::slice::from_ref(&same));

    let resp = dispatch(&mut core, "vec.same", serde_json::json!("counter.increment"), serde_json::json!({}));
    assert!(resp.ok, "{:?}", resp.error);
    let patches: Vec<forge_ui::Patch> = serde_json::from_value(resp.payload["patches"].clone()).unwrap();
    assert!(patches.is_empty(), "an identical re-render is an empty patch (no spurious diff): {patches:?}");

    // The empty-patch event still replays identically in a session.
    let replay = core.handle(cmd(
        "runtime.replay_session",
        "vec.same",
        serde_json::json!({ "run_ids": [initial_run_id, resp.payload["run_id"].as_str().unwrap()] }),
    ));
    assert!(replay.ok, "{:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
    // The re-derived event patch is empty too.
    assert_eq!(replay.payload["event_patches"][0], serde_json::json!([]));
}

/// A session-replay over an empty / missing / non-array `run_ids` is a clean typed
/// rejection, and an unknown run id in the session is a clean `ValidationError`.
#[test]
fn session_replay_rejects_malformed_input() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.bad");

    // Missing run_ids.
    let r = core.handle(cmd("runtime.replay_session", "vec.bad", serde_json::json!({})));
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code(), "ValidationError");

    // Empty run_ids.
    let r = core.handle(cmd(
        "runtime.replay_session",
        "vec.bad",
        serde_json::json!({ "run_ids": [] }),
    ));
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code(), "ValidationError");

    // Unknown run id.
    let r = core.handle(cmd(
        "runtime.replay_session",
        "vec.bad",
        serde_json::json!({ "run_ids": ["run_does_not_exist"] }),
    ));
    assert!(!r.ok);
    assert_eq!(r.error.unwrap().code(), "ValidationError");
}

/// A malformed session SHAPE is rejected at the command boundary BEFORE any
/// "converged final tree" / `replays_identically` claim is made — so that claim is
/// load-bearing about a real recorded session, not an artifact of an arbitrary id
/// list. Three malformed shapes, each over real recorded runs:
///   (a) a dispatched EVENT placed at the head (a session must open with the run);
///   (b) the initial `runtime.run` spliced into the tail (events only after head);
///   (c) a duplicated run id (a session is a linear trace, not a multiset).
#[test]
fn session_replay_rejects_malformed_session_shape() {
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    install_player(&mut core, "vec.shape");

    // Record TWO independent heads + two events so we have real run ids of each
    // kind to mis-order. Each head opens its own session with its own queue.
    let t0 = serde_json::json!({ "type": "Text", "testId": "t", "text": "a" });
    let t1 = serde_json::json!({ "type": "Text", "testId": "t", "text": "b" });
    let t2 = serde_json::json!({ "type": "Text", "testId": "t", "text": "c" });
    let head1 = render_initial_run_id(&mut core, "vec.shape", &t0, &[t1.clone(), t2.clone()]);
    let e1 = dispatch(&mut core, "vec.shape", serde_json::json!("counter.increment"), serde_json::json!({}))
        .payload["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let e2 = dispatch(&mut core, "vec.shape", serde_json::json!("counter.increment"), serde_json::json!({}))
        .payload["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    // A SECOND head (its own initial runtime.run) to splice into a tail.
    let head2 = render_initial_run_id(&mut core, "vec.shape", &t0, std::slice::from_ref(&t1));

    // Sanity: the correct shape replays identically (so the rejections below are
    // about SHAPE, not about un-replayable runs).
    let ok = core.handle(cmd(
        "runtime.replay_session",
        "vec.shape",
        serde_json::json!({ "run_ids": [head1.clone(), e1.clone(), e2.clone()] }),
    ));
    assert!(ok.ok, "the well-formed session must replay: {:?}", ok.error);
    assert_eq!(ok.payload["replays_identically"], serde_json::json!(true));

    // (a) a dispatched event at the head.
    let r = core.handle(cmd(
        "runtime.replay_session",
        "vec.shape",
        serde_json::json!({ "run_ids": [e1.clone(), e2.clone()] }),
    ));
    assert!(!r.ok, "a dispatch at the head must be rejected");
    let err = r.error.unwrap();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("dispatched UI event"), "{err}");

    // (b) the initial runtime.run spliced into the tail.
    let r = core.handle(cmd(
        "runtime.replay_session",
        "vec.shape",
        serde_json::json!({ "run_ids": [head1.clone(), e1.clone(), head2] }),
    ));
    assert!(!r.ok, "a runtime.run in the tail must be rejected");
    let err = r.error.unwrap();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("must be a ui.dispatch_event"), "{err}");

    // (c) a duplicated run id.
    let r = core.handle(cmd(
        "runtime.replay_session",
        "vec.shape",
        serde_json::json!({ "run_ids": [head1, e1.clone(), e1] }),
    ));
    assert!(!r.ok, "a duplicated run id must be rejected");
    let err = r.error.unwrap();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("more than once"), "{err}");
}

/// The named human-facing fixture applet
/// (`forge/fixtures/e2e/interactive_ui/applet.ts`) binds `Button.onTap` and
/// `TextField.onChange` to handler names that are the dispatch ActionRefs, and
/// persists state through `ctx.storage` / `ctx.db` — the contract artifact stays
/// in lockstep with the dispatch path.
#[test]
fn interactive_fixture_applet_drives_through_the_command() {
    let ts = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/interactive_ui/applet.ts"),
    )
    .expect("read the interactive_ui fixture applet");
    // onTap / onChange bound to exported handler names (the ActionRefs).
    assert!(ts.contains(r#"onTap: "increment""#));
    assert!(ts.contains(r#"onChange: "setLabel""#));
    assert!(ts.contains("export async function increment"));
    assert!(ts.contains("export async function setLabel"));
    assert!(ts.contains("export async function saveNote"));
    // It persists through ctx.storage AND ctx.db (both effect families in handlers).
    assert!(ts.contains("ctx.storage.set"));
    assert!(ts.contains("ctx.db.insert"));

    // Drive the real applet through the command: install, render, increment.
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();
    let resp = core.handle(cmd(
        "applet.install",
        "interactive",
        serde_json::json!({
            "manifest": player_manifest(),
            "sources": { "src/main.ts": ts }
        }),
    ));
    assert!(resp.ok, "install: {:?}", resp.error);

    let run = core.handle(cmd(
        "runtime.run",
        "interactive",
        serde_json::json!({ "input": {} }),
    ));
    assert!(run.ok, "initial render: {:?}", run.error);

    // Dispatch the onTap "increment" ActionRef: the counter goes 0 -> 1, and the
    // produced patch is a single update_text on the value Text.
    let bump = dispatch(&mut core, "interactive", serde_json::json!("increment"), serde_json::json!({}));
    assert!(bump.ok, "increment: {:?}", bump.error);
    let tree: Node = from_str(&bump.payload["tree"].to_string()).unwrap();
    let value_text = match &tree {
        Node::Stack { children, .. } => children[0].clone(),
        other => panic!("expected a Stack root, got {other:?}"),
    };
    assert_eq!(value_text.type_name(), "Text");
    let patches: Vec<forge_ui::Patch> = serde_json::from_value(bump.payload["patches"].clone()).unwrap();
    assert_eq!(patches.len(), 1, "incrementing the count is a single text patch: {patches:?}");

    // Dispatch the onChange "setLabel" ActionRef with a valid string, then prove an
    // invalid (non-string) payload is a clean typed rejection with no patch emitted.
    let set = dispatch(
        &mut core,
        "interactive",
        serde_json::json!("setLabel"),
        serde_json::json!({ "value": "Ada" }),
    );
    assert!(set.ok, "setLabel: {:?}", set.error);
    let before = core.events().events_of_kind("ui.patch").count();
    let bad = dispatch(
        &mut core,
        "interactive",
        serde_json::json!("setLabel"),
        serde_json::json!({ "value": 42 }),
    );
    assert!(!bad.ok, "a non-string change payload must be rejected");
    assert_eq!(bad.error.unwrap().code(), "RuntimeError");
    assert_eq!(
        before,
        core.events().events_of_kind("ui.patch").count(),
        "a rejected event emits no ui.patch"
    );
    // The rejection carries the contract's dedicated payload-validation code so a
    // renderer can re-prompt the field rather than show a generic crash — the same
    // `ui.invalid_event_payload` the `invalid_payload_rejected` vector pins.
    let reject = core
        .events()
        .events_of_kind("ui.dispatch_failed")
        .last()
        .expect("a non-string change payload emits ui.dispatch_failed");
    assert_eq!(
        reject.payload["code"],
        serde_json::json!("ui.invalid_event_payload")
    );
    assert_eq!(reject.payload["dispatch_attempted"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// review 112 — `ctx.files` must work through `ui.dispatch_event`, exactly like
// `runtime.run`
// ---------------------------------------------------------------------------
//
// `cmd_ui_dispatch_event` builds its `StorageHostBridge` over the SAME engine
// path as a run (UI-4). A prior version wired the bridge with only the HTTP
// client + secret store but NOT the injected `ctx.files` filesystem, so a UI
// event handler calling `ctx.files.read`/`write` failed closed even when the
// manifest granted files and `runtime.run` worked for the SAME applet. This test
// pins the fix: a granted file op inside a dispatched handler round-trips
// end-to-end through the injected `InMemoryFileSystem`.

/// An interactive applet whose `main` renders a placeholder tree (the diff base)
/// and whose `saveDraft` handler writes a text draft through `ctx.files.write`,
/// reads it back through `ctx.files.read`, and renders the round-tripped bytes —
/// so the read-back is visible in the dispatch's returned tree. Every path is
/// INSIDE the `files_manifest` grant. `ZHJhZnQgdjE=` = `draft v1`.
const FILES_DISPATCH_TS: &str = r#"
    export async function main(ctx, _input) {
        ctx.ui.render({ type: "Text", text: "no draft yet" });
        return { ok: true };
    }
    export const handlers = {
        "saveDraft": async (ctx, _event) => {
            await ctx.files.write({
                handle: "workspace_data", path: "drafts/note.txt",
                bytes_base64: "ZHJhZnQgdjE=", content_type: "text/plain",
                mode: "create_or_truncate"
            });
            const back = await ctx.files.read({ handle: "workspace_data", path: "drafts/note.txt" });
            ctx.ui.render({ type: "Text", text: "draft: " + back.bytes_base64 });
            return { ok: true, value: { draft_back: back.bytes_base64 } };
        },
    };
"#;

/// The `files_manifest` from spine.rs: grants `files.read`/`write` on the
/// `drafts/*.txt` glob under the `workspace_data` handle (plus ui).
fn files_dispatch_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": [], "write": [] },
            "db": { "read": [], "write": [] },
            "ui": true,
            "files": {
                "read": [
                    { "handle": "workspace_data", "path_glob": "drafts/*.txt",
                      "max_bytes": 65536, "content_types": ["text/plain"] }
                ],
                "write": [
                    { "handle": "workspace_data", "path_glob": "drafts/*.txt",
                      "max_bytes": 65536, "content_types": ["text/plain"] }
                ]
            }
        },
        "limits": {
            "wall_ms": 3000, "fuel": 10000000, "memory_bytes": 67108864,
            "max_host_calls": 10000, "storage_bytes": 10485760, "log_bytes": 262144
        }
    })
}

#[test]
fn dispatch_handler_files_read_write_runs_through_injected_filesystem() {
    use forge_runtime::InMemoryFileSystem;

    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // Inject the SAME trusted per-applet sandbox filesystem wiring the run-path
    // files tests use: grant the `workspace_data` handle a root. The factory
    // builds a fresh filesystem each invocation; the write the handler performs
    // (and reads back) lands in THIS dispatch's filesystem. Without the review-112
    // fix the dispatch path never wires this factory onto its bridge, so the
    // handler's `ctx.files.write` fails closed and the dispatch is rejected.
    core.set_file_system_factory(|| {
        Box::new(
            InMemoryFileSystem::new()
                .with_handle_root("workspace_data", "/sandbox/app_files/workspace_data"),
        )
    });

    let resp = core.handle(cmd(
        "applet.install",
        "files_dispatch",
        serde_json::json!({
            "manifest": files_dispatch_manifest(),
            "sources": { "src/main.ts": FILES_DISPATCH_TS }
        }),
    ));
    assert!(resp.ok, "install: {:?}", resp.error);

    // The initial render establishes the diff base (the interactive session start).
    let run = core.handle(cmd("runtime.run", "files_dispatch", serde_json::json!({ "input": {} })));
    assert!(run.ok, "initial render: {:?}", run.error);

    // Dispatch the `saveDraft` handler: it writes + reads back through `ctx.files`.
    // This SUCCEEDS only because the dispatch bridge now carries the injected
    // filesystem (the fix). Pre-fix this is a typed CapabilityRequired rejection
    // (`resp.ok == false`), so the dispatch succeeding meaningfully exercises the
    // files bridge in the dispatch path.
    let save = dispatch(
        &mut core,
        "files_dispatch",
        serde_json::json!("saveDraft"),
        serde_json::json!({}),
    );
    assert!(
        save.ok,
        "a granted ctx.files op inside a dispatched handler must succeed: {:?}",
        save.error
    );

    // The handler's read-back bytes are visible in the dispatch's returned tree —
    // proof the write→read round-trip ran end-to-end through the injected
    // filesystem, not a fail-closed empty default.
    let tree: Node = from_str(&save.payload["tree"].to_string()).expect("produced tree parses");
    match &tree {
        Node::Text { value, .. } => assert_eq!(
            value, "draft: ZHJhZnQgdjE=",
            "the handler rendered the round-tripped file bytes"
        ),
        other => panic!("expected a Text root carrying the read-back bytes, got {other:?}"),
    }

    // The file ops are in the dispatched run's recorded host-call trace, in order
    // (CR-8) — so the dispatch records + replays the file effects like a run.
    let run_id = save.payload["run_id"].as_str().unwrap().to_string();
    let rec = core.store().load_run(&run_id).unwrap().unwrap();
    let methods: Vec<&str> = rec.calls.iter().map(|c| c.method.as_str()).collect();
    assert!(
        methods.contains(&"files.write") && methods.contains(&"files.read"),
        "the dispatched run records both file ops: {methods:?}"
    );
    let read = rec.calls.iter().find(|c| c.method == "files.read").unwrap();
    assert_eq!(
        read.response["bytes_base64"],
        serde_json::json!("ZHJhZnQgdjE="),
        "the recorded read-back bytes equal what the handler wrote, within its grant"
    );

    // And the dispatched run replays byte-identically (CR-8: the recorded file
    // responses are served, never re-performed against any live filesystem).
    let replay = core.handle(cmd(
        "runtime.replay",
        "files_dispatch",
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(replay.ok, "the files dispatch must replay: {:?}", replay.error);
    assert_eq!(replay.payload["replays_identically"], serde_json::json!(true));
}
