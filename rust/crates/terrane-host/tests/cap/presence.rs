//! e2e smoke for `presence`: app resource publish is live-only and rate-limited.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

fn write_bundle(home: &std::path::Path, id: &str, backend: &str) -> String {
    let bundle = home.join(id);
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{"id":"{id}","name":"Presence","runtime":"js","backend":"main.js","resources":["presence"]}}"#
        ),
    )
    .unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

#[test]
fn presence_publish_records_nothing_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let source = write_bundle(
        home,
        "presence-app",
        r#"
        function handle(input) {
            if (input[0] === "peers") return JSON.stringify(ctx.resource.presence.peers("cursor"));
            return ctx.resource.presence.publish("cursor", {x: 1, y: 2});
        }
        "#,
    );
    let (ok, out, err) = terrane(
        home,
        &["app", "add", "presence-app", "Presence", "--source", &source],
    );
    assert!(ok, "app add failed: {out} {err}");
    let (ok, out, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "presence-app", "presence"],
    );
    assert!(ok, "auth grant failed: {out} {err}");

    let (ok, out, err) = terrane(home, &["run", "presence-app", "publish"]);
    assert!(ok, "publish failed: {out} {err}");
    assert_eq!(out.trim(), "ok");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(!log.contains("PresencePublish"), "log leaked effect: {log}");
    assert!(!log.contains(r#""x":1"#), "log leaked payload: {log}");
    assert!(!log.contains("presence.publish"), "log recorded publish: {log}");

    let (ok, out, err) = terrane(home, &["run", "presence-app", "peers"]);
    assert!(ok, "peers failed: {out} {err}");
    assert_eq!(out.trim(), "[]");

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {out} {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}

#[test]
fn presence_publish_rate_limit_surfaces_error_without_queueing() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let source = write_bundle(
        home,
        "presence-burst",
        r#"
        function handle(input) {
            for (var i = 0; i < 21; i++) {
                ctx.resource.presence.publish("cursor", {i: i});
            }
            return "done";
        }
        "#,
    );
    let (ok, out, err) = terrane(
        home,
        &["app", "add", "presence-burst", "Presence", "--source", &source],
    );
    assert!(ok, "app add failed: {out} {err}");
    terrane(
        home,
        &["auth", "grant", "user:local-owner", "presence-burst", "presence"],
    );

    let (ok, out, err) = terrane(home, &["run", "presence-burst", "burst"]);
    assert!(!ok, "burst should fail rate limit: {out}");
    assert!(
        err.contains("rate limit") || out.contains("rate limit"),
        "expected rate limit error, out={out} err={err}"
    );
}
