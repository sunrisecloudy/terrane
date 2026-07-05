//! e2e for `time` — drive the real `terrane` binary, which reads `SystemTime`
//! at the edge and records `time.observed`. Pure (no network / no model), so it
//! runs by DEFAULT. Proves the Option-A contract: replay rebuilds the recorded
//! observations without ever consulting a clock.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

/// True iff `value` is a 13-digit (current-epoch) UTC millis decimal string.
fn is_epoch_ms(value: &str) -> bool {
    value.len() == 13 && value.bytes().all(|b| b.is_ascii_digit())
}

#[test]
fn time_now_records_observation_and_replays_without_a_clock() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("clock-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "clock-app", "name": "Clock", "runtime": "js", "backend": "main.js", "resources": ["time"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                var a = ctx.resource.time.now();
                var b = ctx.resource.time.now();
                return a + ";" + b;
            }
        "#,
    )
    .unwrap();

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "clock-app",
            "Clock",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "clock-app", "time"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "clock-app", "go"]);
    assert!(ok, "js-runtime run failed: {err}");
    let parts: Vec<&str> = out.trim().split(';').collect();
    assert_eq!(parts.len(), 2, "expected two epoch-ms values, got: {out}");
    assert!(parts.iter().all(|p| is_epoch_ms(p)), "epoch-ms parts: {out:?}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    let observed = log.matches("time.observed").count();
    assert_eq!(observed, 2, "log: {log}");

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}