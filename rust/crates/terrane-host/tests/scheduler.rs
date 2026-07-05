use std::fs;

use tempfile::tempdir;

#[test]
fn scheduler_due_loop_invokes_quickjs_action_and_records_history() {
    let dir = tempdir().unwrap();
    let bundle = dir.path().join("ops-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"ops","name":"Ops","runtime":"js","backend":"main.js","resources":["scheduler"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
        var actions = {
          opsHeartbeat: {
            run: function (args) {
              var payload = JSON.parse(args[0]);
              return JSON.stringify({ ok: true, source: payload.source });
            }
          }
        };
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
        "scheduler.create",
        &[
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            "* * * * *".into(),
            "Asia/Bangkok".into(),
            "opsHeartbeat".into(),
            r#"{"source":"premium-ops-proof"}"#.into(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "auth.grant",
        &[
            terrane_host::LOCAL_OWNER_SUBJECT.into(),
            "ops".into(),
            "scheduler".into(),
        ],
    )
    .unwrap();

    let outcomes = terrane_host::scheduler::run_due_at(&mut core, 60).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].status, "completed", "{outcomes:?}");
    assert_eq!(
        outcomes[0].output.as_deref(),
        Some(r#"{"ok":true,"source":"premium-ops-proof"}"#)
    );

    let schedule = &core.state().scheduler.schedules["ops"]["quickjs-ops-heartbeat"];
    assert_eq!(schedule.active_run_id, None);
    assert!(schedule.next_due_at > 60);
    let run = core
        .state()
        .scheduler
        .runs
        .get("ops")
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(run.status.as_str(), "completed");
    assert_eq!(run.action, "opsHeartbeat");
}

#[test]
fn scheduler_due_loop_records_failure_when_action_fails() {
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
        r#"var actions = { opsHeartbeat: { run: function () { throw new Error("boom"); } } };"#,
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
        "scheduler.create",
        &[
            "ops".into(),
            "quickjs-ops-heartbeat".into(),
            "* * * * *".into(),
            "Asia/Bangkok".into(),
            "opsHeartbeat".into(),
            r#"{"source":"premium-ops-proof"}"#.into(),
        ],
    )
    .unwrap();

    let outcomes = terrane_host::scheduler::run_due_at(&mut core, 60).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].status, "failed");
    let run = core
        .state()
        .scheduler
        .runs
        .get("ops")
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(run.status.as_str(), "failed");
    assert!(run.error_json.as_deref().unwrap().contains("boom"));
}
