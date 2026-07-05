use std::fs;

use tempfile::tempdir;

#[test]
fn scheduler_due_loop_invokes_backend_after_recording_fire() {
    let dir = tempdir().unwrap();
    let bundle = dir.path().join("ops-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"ops","name":"Ops","runtime":"js","backend":"main.js","resources":["kv"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
        function handle(input) {
          if (input[0] === "opsHeartbeat") {
            ctx.resource.kv.set("heartbeat", input.join("|"));
            return JSON.stringify({ ok: true, name: input[1], scheduledFor: input[2], arg: input[3] });
          }
          return "unknown";
        }
        "#,
    )
    .unwrap();

    let mut core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "app.add",
        &[
            "ops".into(),
            "Ops".into(),
            "--source".into(),
            bundle.to_string_lossy().into_owned(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "auth.grant",
        &[
            terrane_host::LOCAL_OWNER_SUBJECT.into(),
            "ops".into(),
            "kv".into(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "scheduler.set",
        &[
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            r#"{"at":60000,"verb":"opsHeartbeat","args":["premium-ops-proof"]}"#.into(),
        ],
    )
    .unwrap();

    let outcomes = terrane_host::scheduler::run_due_at(&mut core, 60_000).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert_eq!(
        outcomes[0].output.as_deref(),
        Some(r#"{"ok":true,"name":"quickjs-ops-heartbeat","scheduledFor":"60000","arg":"premium-ops-proof"}"#)
    );

    assert!(!core.state().scheduler.schedules["ops"].contains_key("quickjs-ops-heartbeat"));
    assert_eq!(
        core.state().kv.data["ops"]["heartbeat"],
        "opsHeartbeat|quickjs-ops-heartbeat|60000|premium-ops-proof"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn scheduler_due_loop_keeps_fire_fact_when_backend_fails() {
    let dir = tempdir().unwrap();
    let bundle = dir.path().join("ops-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"ops","name":"Ops","runtime":"js","backend":"main.js","resources":[]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"function handle(input) { if (input[0] === "opsHeartbeat") { throw new Error("boom"); } return "unknown"; }"#,
    )
    .unwrap();

    let mut core = terrane_host::open_at_log_path(dir.path().join("log.bin")).unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "app.add",
        &[
            "ops".into(),
            "Ops".into(),
            "--source".into(),
            bundle.to_string_lossy().into_owned(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "scheduler.set",
        &[
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            r#"{"at":60000,"verb":"opsHeartbeat"}"#.into(),
        ],
    )
    .unwrap();

    let outcomes = terrane_host::scheduler::run_due_at(&mut core, 60_000).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0].error.as_deref().unwrap_or_default().contains("boom"),
        "{outcomes:?}"
    );
    assert!(!core.state().scheduler.schedules["ops"].contains_key("quickjs-ops-heartbeat"));
    assert!(core.replay_matches().unwrap());
}
