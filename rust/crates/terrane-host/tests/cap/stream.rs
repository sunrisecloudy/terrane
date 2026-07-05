//! e2e smoke for `stream`.

use std::fs;
use std::path::Path;

use tempfile::tempdir;

use crate::helpers::terrane;

fn write_bundle(dir: &Path) -> String {
    let bundle = dir.join("streamer");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"streamer","name":"Streamer","runtime":"js","backend":"main.js","resources":["kv","stream","blob"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
        function handle(input) {
            if (input[0] === "onMessage") {
                var msg = JSON.parse(input[1]);
                ctx.resource.kv.set("last", msg.dataKind + ":" + msg.data);
                return msg.data;
            }
            if (input[0] === "last") return ctx.resource.kv.get("last");
            if (input[0] === "streams") return ctx.resource.stream.list();
            return "?";
        }
        "#,
    )
    .unwrap();
    bundle.to_str().unwrap().to_string()
}

#[test]
fn stream_cli_opens_ingests_delivers_reopens_closes_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let source = write_bundle(home);
    let (ok, out, err) = terrane(
        home,
        &["app", "add", "streamer", "Streamer", "--source", &source],
    );
    assert!(ok, "app add failed: {out} {err}");
    terrane(home, &["auth", "grant", "user:local-owner", "streamer", "kv"]);
    terrane(home, &["auth", "grant", "user:local-owner", "streamer", "stream"]);
    terrane(home, &["auth", "grant", "user:local-owner", "streamer", "blob"]);

    let request = r#"{"kind":"sse","url":"http://127.0.0.1/feed?token=secret","headers":{"Authorization":"Bearer raw"}}"#;
    let (ok, out, err) = terrane(home, &["stream", "open", "streamer", "ticks", "onMessage", request]);
    assert!(ok, "open failed: {out} {err}");
    assert!(out.contains("stream.opened"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "stream",
            "ingest-text",
            "streamer",
            "ticks",
            "--received-at",
            "1000",
            "hello",
        ],
    );
    assert!(ok, "ingest failed: {out} {err}");
    assert!(out.contains("hello"), "backend output: {out}");

    let (ok, out, err) = terrane(home, &["run", "streamer", "last"]);
    assert!(ok, "last failed: {out} {err}");
    assert!(out.contains("inline:hello"), "last: {out}");

    let (ok, out, err) = terrane(home, &["stream", "reopened", "streamer", "ticks", "1"]);
    assert!(ok, "reopen failed: {out} {err}");
    assert!(out.contains("stream.reopened"), "out: {out}");

    let (ok, out, err) = terrane(home, &["stream", "close", "streamer", "ticks"]);
    assert!(ok, "close failed: {out} {err}");
    assert!(out.contains("stream.closed"), "out: {out}");

    let (ok, out, err) = terrane(home, &["stream", "list", "streamer"]);
    assert!(ok, "list failed: {out} {err}");
    assert!(out.contains(r#""lastSeq":1"#), "list: {out}");
    assert!(out.contains(r#""status":"closed""#), "list: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("stream.opened streamer/ticks sse onMessage 127.0.0.1/feed"));
    assert!(!log.contains("Bearer raw"), "log leaked header: {log}");
    assert!(!log.contains("token=secret"), "log leaked query: {log}");

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {out} {err}");
}

#[test]
fn stream_large_ingest_offloads_to_blob_metadata() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let source = write_bundle(home);
    terrane(
        home,
        &["app", "add", "streamer", "Streamer", "--source", &source],
    );
    terrane(home, &["auth", "grant", "user:local-owner", "streamer", "kv"]);
    terrane(home, &["auth", "grant", "user:local-owner", "streamer", "stream"]);
    terrane(home, &["auth", "grant", "user:local-owner", "streamer", "blob"]);
    let request = r#"{"kind":"sse","url":"http://127.0.0.1/feed"}"#;
    terrane(home, &["stream", "open", "streamer", "ticks", "onMessage", request]);

    let large = "x".repeat(terrane_cap_stream::INLINE_TEXT_LIMIT + 1);
    let (ok, out, err) = terrane(
        home,
        &[
            "stream",
            "ingest-text",
            "streamer",
            "ticks",
            "--received-at",
            "1000",
            &large,
        ],
    );
    assert!(ok, "large ingest failed: {out} {err}");
    assert!(out.contains("__stream__/streamer/ticks/1"), "out: {out}");

    let (ok, blobs, err) = terrane(home, &["blob", "ls", "streamer", "__stream__/"]);
    assert!(ok, "blob ls failed: {blobs} {err}");
    assert!(blobs.contains("__stream__/streamer/ticks/1"), "blobs: {blobs}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("stream.message streamer/ticks #1 blob"), "log: {log}");
    assert!(!log.contains(&large), "log should not inline large data");
}
