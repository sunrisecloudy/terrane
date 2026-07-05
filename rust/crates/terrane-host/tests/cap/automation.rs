//! e2e for `automation` — app records an event rule, CLI tick records
//! `automation.fired`, then invokes the backend verb. Pure and local.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn automation_tick_fires_matching_kv_event_runs_backend_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("automation-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "automation-app", "name": "Automation", "runtime": "js", "backend": "main.js", "resources": ["automation", "kv"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                var verb = input[0];
                if (verb === "init") {
                    ctx.resource.automation.set("inbox", JSON.stringify({
                        trigger: { kind: "kv.set", filter: "payload.key == 'inbox/1'" },
                        action: { verb: "summarize", argsTemplate: ["{{payload.key}}"] },
                        cooldownMs: 1000
                    }));
                    return "armed";
                }
                if (verb === "seed") {
                    ctx.resource.kv.set("inbox/1", "hello");
                    return "seeded";
                }
                if (verb === "summarize") {
                    ctx.resource.kv.set("summary", input[1] || "");
                    return "summarized";
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
            "automation-app",
            "Automation",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    for namespace in ["automation", "kv"] {
        let (ok, _, err) = terrane(
            home,
            &["auth", "grant", "user:local-owner", "automation-app", namespace],
        );
        assert!(ok, "{namespace} grant failed: {err}");
    }

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "automation-app", "init"]);
    assert!(ok, "init run failed: {err}");
    assert_eq!(out.trim(), "armed");
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "automation-app", "seed"]);
    assert!(ok, "seed run failed: {err}");
    assert_eq!(out.trim(), "seeded");

    let (ok, out, err) = terrane(home, &["automation", "tick", "--now-ms", "2000"]);
    assert!(ok, "automation tick failed: {err}");
    assert!(
        out.contains("ran automation-app/inbox verb=summarize"),
        "tick output: {out}"
    );

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("automation.fired"), "log: {log}");
    assert!(
        log.contains("kv.set automation-app/summary"),
        "backend did not persist summary marker: {log}"
    );

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}

#[test]
fn automation_tick_records_suppression_when_fire_budget_is_exhausted() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("automation-budget-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "automation-budget-app", "name": "Automation Budget", "runtime": "js", "backend": "main.js", "resources": ["automation", "kv"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                var verb = input[0];
                if (verb === "init") {
                    for (var i = 0; i < 9; i++) {
                        ctx.resource.automation.set("rule-" + i, JSON.stringify({
                            trigger: { kind: "kv.set", filter: "payload.key == 'inbox/1'" },
                            action: { verb: "mark", argsTemplate: ["{{payload.key}}"] },
                            cooldownMs: 1000
                        }));
                    }
                    return "armed";
                }
                if (verb === "seed") {
                    ctx.resource.kv.set("inbox/1", "hello");
                    return "seeded";
                }
                if (verb === "mark") {
                    ctx.resource.kv.set("last-mark", input[1] || "");
                    return "marked";
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
            "automation-budget-app",
            "Automation Budget",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    for namespace in ["automation", "kv"] {
        let (ok, _, err) = terrane(
            home,
            &[
                "auth",
                "grant",
                "user:local-owner",
                "automation-budget-app",
                namespace,
            ],
        );
        assert!(ok, "{namespace} grant failed: {err}");
    }

    let (ok, _, err) = terrane(
        home,
        &["js-runtime", "run", "automation-budget-app", "init"],
    );
    assert!(ok, "init run failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["js-runtime", "run", "automation-budget-app", "seed"],
    );
    assert!(ok, "seed run failed: {err}");

    let (ok, out, err) = terrane(home, &["automation", "tick", "--now-ms", "2000"]);
    assert!(ok, "automation tick failed: {err}");
    assert!(out.contains("suppressed 1 automation fire(s)"), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert_eq!(log.matches("automation.fired").count(), 8, "log: {log}");
    assert_eq!(
        log.matches("automation.suppressed").count(),
        1,
        "log: {log}"
    );
}
