//! e2e for `job` — CLI submits durable jobs, `scheduler tick` drains them, and
//! replay folds lifecycle facts without re-running the backend.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn job_submit_tick_complete_and_replay() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("job-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "job-app", "name": "Jobs", "runtime": "js", "backend": "main.js", "resources": ["job", "kv"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                var verb = input[0];
                var jobId = input[1];
                if (verb === "work") {
                    ctx.resource.kv.set("job/" + jobId, input.slice(2).join("|"));
                    return "done:" + jobId;
                }
                if (verb === "fail") {
                    throw new Error("planned failure " + jobId);
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
            "job-app",
            "Jobs",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "job-app", "job"]);
    assert!(ok, "job grant failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "job-app", "kv"]);
    assert!(ok, "kv grant failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &[
            "job",
            "submit",
            "job-app",
            "work",
            "--job-id",
            "job-1",
            "--now-ms",
            "1000",
            "alpha",
        ],
    );
    assert!(ok, "job submit failed: {err}");
    assert_eq!(out.trim(), "job-1");

    let (ok, out, err) = terrane(home, &["scheduler", "tick", "--now-ms", "1000"]);
    assert!(ok, "scheduler tick failed: {err}");
    assert!(
        out.contains("job_completed job-app/job-1 attempt=1 verb=work"),
        "tick output: {out}"
    );

    let (ok, out, err) = terrane(home, &["job", "stat", "job-app", "job-1"]);
    assert!(ok, "job stat failed: {err}");
    assert!(out.contains(r#""status":"done""#), "stat: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("job.submitted job-app/job-1"), "log: {log}");
    assert!(log.contains("job.started job-app/job-1"), "log: {log}");
    assert!(log.contains("job.completed job-app/job-1"), "log: {log}");
    assert!(
        log.contains("kv.set job-app/job/job-1"),
        "backend did not persist job marker: {log}"
    );

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}

#[test]
fn failing_job_records_retry_backoff_and_terminal_failure() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("job-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "job-app", "name": "Jobs", "runtime": "js", "backend": "main.js", "resources": ["job"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                throw new Error("planned failure " + input[1]);
            }
        "#,
    )
    .unwrap();

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "job-app",
            "Jobs",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "job-app", "job"]);
    assert!(ok, "job grant failed: {err}");

    let retry = r#"{"maxAttempts":2,"baseDelayMs":100,"factor":2,"maxDelayMs":1000}"#;
    let (ok, _, err) = terrane(
        home,
        &[
            "job",
            "submit",
            "job-app",
            "fail",
            "--job-id",
            "job-err",
            "--now-ms",
            "1000",
            "--retry",
            retry,
        ],
    );
    assert!(ok, "job submit failed: {err}");
    let (ok, out, err) = terrane(home, &["scheduler", "tick", "--now-ms", "1000"]);
    assert!(ok, "first tick failed: {err}");
    assert!(
        out.contains("job_failed job-app/job-err attempt=1 verb=fail"),
        "tick output: {out}"
    );
    let (ok, out, err) = terrane(home, &["job", "stat", "job-app", "job-err"]);
    assert!(ok, "job stat failed: {err}");
    assert!(out.contains(r#""status":"queued""#), "stat: {out}");
    assert!(out.contains(r#""next_attempt_at":"#), "stat: {out}");

    let (ok, out, err) = terrane(home, &["scheduler", "tick", "--now-ms", "9999999999999"]);
    assert!(ok, "second tick failed: {err}");
    assert!(
        out.contains("job_failed job-app/job-err attempt=2 verb=fail"),
        "tick output: {out}"
    );
    let (ok, out, err) = terrane(home, &["job", "stat", "job-app", "job-err"]);
    assert!(ok, "job stat failed: {err}");
    assert!(out.contains(r#""status":"failed""#), "stat: {out}");
}
