//! e2e for `scheduler` — app records a one-shot wake-up, the CLI host tick
//! records `scheduler.fired`, then invokes the backend verb. Pure and local.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn scheduler_tick_fires_due_one_shot_runs_backend_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("scheduler-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "scheduler-app", "name": "Scheduler", "runtime": "js", "backend": "main.js", "resources": ["scheduler", "kv"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                var verb = input[0];
                if (verb === "init") {
                    ctx.resource.scheduler.set("once", JSON.stringify({
                        at: 1000,
                        verb: "on_timer",
                        args: ["payload"]
                    }));
                    return "scheduled";
                }
                if (verb === "on_timer") {
                    ctx.resource.kv.set("timer", input.join("|"));
                    return "fired";
                }
                return "unknown";
            }
        "#,
    )
    .unwrap();

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "scheduler-app",
            "Scheduler",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "scheduler-app", "scheduler"],
    );
    assert!(ok, "scheduler grant failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "scheduler-app", "kv"],
    );
    assert!(ok, "kv grant failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "scheduler-app", "init"]);
    assert!(ok, "init run failed: {err}");
    assert_eq!(out.trim(), "scheduled");

    let (ok, out, err) = terrane(home, &["scheduler", "tick", "--now-ms", "1000"]);
    assert!(ok, "scheduler tick failed: {err}");
    assert!(
        out.contains("ran scheduler-app/once scheduled_for=1000 skipped=0 verb=on_timer"),
        "tick output: {out}"
    );

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("scheduler.fired"), "log: {log}");
    assert!(
        log.contains("kv.set scheduler-app/timer"),
        "backend did not persist timer marker: {log}"
    );

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}
