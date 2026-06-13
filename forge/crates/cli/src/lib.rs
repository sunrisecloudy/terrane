//! forge-cli: the M0a spine harness library.
//!
//! prd-merged/06 PS-5 (the CLI harness shell) + prd-merged/09 M0a exit (the
//! executable spine + its acceptance proof). The `forge` binary is a thin arg
//! parser over this library; the heavy lifting (drive the whole jewel end to
//! end, then assert deterministic replay) lives here so integration tests can
//! call the same code path the binary does.
//!
//! The spine this drives:
//!
//! ```text
//!   TS ─SWC─▶ JS ─QuickJS─▶ ctx capability gate ─▶ SQLite write
//!       ─▶ UI tree patch ─▶ deterministic RunRecord ─▶ replay (byte-identical)
//! ```
//!
//! Everything is offline and in a temp/in-memory workspace, so `forge demo` is
//! reproducible on any machine.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, CoreCommand, CoreError, RequestId, Result, WorkspaceId};

/// The applet id the demo installs under.
const DEMO_APPLET_ID: &str = "notes-lite";

/// The notes-lite demo source + manifest, embedded so `forge demo` needs no
/// filesystem layout at runtime (the binary is self-contained). Kept in lockstep
/// with `examples/notes-lite/` — the e2e test loads those files from disk and
/// asserts they match these embeds, so they cannot drift silently.
pub const NOTES_LITE_MAIN_TS: &str =
    include_str!("../../../examples/notes-lite/src/main.ts");
pub const NOTES_LITE_MANIFEST_JSON: &str =
    include_str!("../../../examples/notes-lite/manifest.json");

/// The outcome of driving the spine once: enough of the run to print a report
/// and to assert the M0a exit conditions in a test.
#[derive(Debug, Clone)]
pub struct DemoOutcome {
    /// Whether the run's `main` returned `{ ok: true }`.
    pub run_ok: bool,
    /// The `AppResult` the applet returned (the `{ ok, value }` object).
    pub result: serde_json::Value,
    /// The id of the recorded run (replay source).
    pub run_id: String,
    /// The replay fingerprint of the recorded run (its observable digest).
    pub fingerprint: String,
    /// The UI trees the run rendered, in order (canonical catalog JSON).
    pub ui_trees: Vec<serde_json::Value>,
    /// The records stored in the `notes` collection after the run.
    pub notes: Vec<serde_json::Value>,
    /// Whether replay reproduced the run byte-identically (the jewel's last link).
    pub replay_identical: bool,
}

/// Drive the whole M0a spine once against a fresh in-memory workspace: install
/// notes-lite, run it with `input`, capture the rendered UI + stored records +
/// recorded run, then replay and check byte-identity.
///
/// This is the single code path `forge demo` and the e2e acceptance test share,
/// so the test proves exactly what the binary does (prd-merged/09 M0a exit).
pub fn run_demo(input: serde_json::Value) -> Result<DemoOutcome> {
    let mut core = WorkspaceCore::in_memory("ws-demo")?;

    install(&mut core, DEMO_APPLET_ID, NOTES_LITE_MANIFEST_JSON, NOTES_LITE_MAIN_TS)?;

    // ---- runtime.run: the TS → ... → SQLite write → UI patch links ----------
    let run_resp = handle(
        &mut core,
        Some(DEMO_APPLET_ID),
        "runtime.run",
        serde_json::json!({ "input": input }),
    )?;

    let run_id = run_resp
        .get("run_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::RuntimeError("runtime.run returned no run_id".into()))?
        .to_string();
    let run_ok = run_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let result = run_resp.get("result").cloned().unwrap_or(serde_json::Value::Null);
    let ui_trees: Vec<serde_json::Value> = run_resp
        .get("ui_renders")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // ---- the SQLite read-back: the stored notes records ---------------------
    let notes = list_records(&mut core, "notes")?;

    // ---- runtime.replay: the deterministic-replay link ----------------------
    let replay_resp = handle(
        &mut core,
        Some(DEMO_APPLET_ID),
        "runtime.replay",
        serde_json::json!({ "run_id": run_id }),
    )?;
    let replay_identical = replay_resp
        .get("replays_identically")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let fingerprint = replay_resp
        .get("fingerprint")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(DemoOutcome {
        run_ok,
        result,
        run_id,
        fingerprint,
        ui_trees,
        notes,
        replay_identical,
    })
}

/// Run the demo and print a human report to `out`, returning the outcome so the
/// caller can pick an exit code. The `forge demo` subcommand is this plus an
/// exit-code mapping.
pub fn demo(out: &mut dyn std::io::Write) -> Result<DemoOutcome> {
    let input = serde_json::json!({ "title": "Buy milk" });
    let outcome = run_demo(input)?;

    let _ = writeln!(out, "forge demo — M0a executable spine (prd-merged/09 M0a exit)");
    let _ = writeln!(out, "applet: {DEMO_APPLET_ID}");
    let _ = writeln!(out);

    let _ = writeln!(out, "── emitted UI tree(s) ──");
    for (i, tree) in outcome.ui_trees.iter().enumerate() {
        let pretty = serde_json::to_string_pretty(tree)
            .unwrap_or_else(|_| tree.to_string());
        let _ = writeln!(out, "render[{i}]:\n{pretty}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "── stored `notes` records ──");
    for note in &outcome.notes {
        let _ = writeln!(out, "{note}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "── run ──");
    let _ = writeln!(out, "run_id:      {}", outcome.run_id);
    let _ = writeln!(out, "result:      {}", outcome.result);
    let _ = writeln!(out, "fingerprint: {}", outcome.fingerprint);
    let _ = writeln!(out);

    let _ = writeln!(out, "REPLAY IDENTICAL: {}", outcome.replay_identical);

    Ok(outcome)
}

// ---------------------------------------------------------------- helpers

/// Install an applet: parse the manifest JSON, derive the entrypoint source key,
/// and issue `applet.install` through the core. A non-`ok` response is surfaced
/// as the underlying [`CoreError`] (so a rejected eval/compile fails here).
pub fn install(
    core: &mut WorkspaceCore,
    applet_id: &str,
    manifest_json: &str,
    entry_ts: &str,
) -> Result<serde_json::Value> {
    let manifest: serde_json::Value = serde_json::from_str(manifest_json).map_err(|e| {
        CoreError::ValidationError(format!("manifest.json is not valid JSON: {e}"))
    })?;
    let entrypoint = manifest
        .get("entrypoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::ValidationError("manifest has no `entrypoint`".into()))?
        .to_string();

    handle(
        core,
        Some(applet_id),
        "applet.install",
        serde_json::json!({
            "manifest": manifest,
            "sources": { entrypoint: entry_ts },
        }),
    )
}

/// Issue a command through the core, mapping a non-`ok` [`forge_domain::CoreResponse`]
/// back into its [`CoreError`] so callers use `?` over the whole spine.
pub fn handle(
    core: &mut WorkspaceCore,
    applet_id: Option<&str>,
    name: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value> {
    let cmd = CoreCommand {
        request_id: RequestId::new(format!("req-{name}")),
        actor: ActorContext::owner("cli"),
        workspace_id: WorkspaceId::new("ws-demo"),
        applet_id: applet_id.map(Into::into),
        name: name.to_string(),
        payload,
    };
    let resp = core.handle(cmd);
    if resp.ok {
        Ok(resp.payload)
    } else {
        Err(resp
            .error
            .unwrap_or_else(|| CoreError::RuntimeError(format!("{name} failed without an error"))))
    }
}

/// List every record in `collection` via `query.execute`, returning the rows
/// (each `{ id, fields }`).
pub fn list_records(core: &mut WorkspaceCore, collection: &str) -> Result<Vec<serde_json::Value>> {
    let resp = handle(
        core,
        None,
        "query.execute",
        serde_json::json!({ "collection": collection }),
    )?;
    Ok(resp
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_demo_assets_match_examples_dir() {
        // The embeds and the on-disk examples/ files are the same bytes, so the
        // binary's self-contained demo cannot drift from the published example.
        let disk_ts = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/notes-lite/src/main.ts"
        ))
        .unwrap();
        let disk_manifest = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/notes-lite/manifest.json"
        ))
        .unwrap();
        assert_eq!(disk_ts, NOTES_LITE_MAIN_TS);
        assert_eq!(disk_manifest, NOTES_LITE_MANIFEST_JSON);
    }

    #[test]
    fn run_demo_drives_the_whole_spine() {
        let outcome = run_demo(serde_json::json!({ "title": "Buy milk" })).unwrap();
        assert!(outcome.run_ok, "demo run must complete ok");
        assert_eq!(outcome.result["value"]["count"], serde_json::json!(1));
        // A note record was stored (the SQLite write link).
        assert_eq!(outcome.notes.len(), 1);
        assert_eq!(outcome.notes[0]["fields"]["title"], serde_json::json!("Buy milk"));
        // A UI tree was produced (the tree-patch link).
        assert!(!outcome.ui_trees.is_empty());
        let tree = outcome.ui_trees[0].to_string();
        assert!(tree.contains("\"Notes\""), "header rendered: {tree}");
        assert!(tree.contains("\"Buy milk\""), "title in list: {tree}");
        // Replay reproduced it byte-identically (the determinism link).
        assert!(outcome.replay_identical, "demo run must replay identically");
    }
}
