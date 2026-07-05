//! e2e smoke for `native`. Real OS UI work stays in connector tests; this
//! drives the trusted CLI command path and log descriptions.

use tempfile::tempdir;

use crate::helpers::terrane;

const PHOTO_HASH: &str = "42f114e0f62e883f51ee40aba3670315c6fefcb88f36b4058e5a99eab2c5f534";
const AUDIO_HASH: &str = "466dd61c5174cf1e25dd16cb8414f31bd47d362fca18cf78bed812d2a499042c";

#[test]
fn native_cli_records_request_and_trusted_completion() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");
    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "platform.observe",
            "local",
            "macos",
            "test-1",
            "external.openUrl",
        ],
    );
    assert!(ok, "platform observe failed: {err}");
    assert!(out.contains("native.platform.observed"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "external.open-url",
            "demo",
            "req-1",
            "https://example.com",
        ],
    );
    assert!(ok, "native request failed: {err}");
    assert!(out.contains("native.requested"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &["native", "complete", "demo", "req-1", r#"{"ok":true}"#],
    );
    assert!(ok, "native complete failed: {err}");
    assert!(out.contains("native.completed"), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        log.contains("native.platform.observed local macos"),
        "log: {log}"
    );
    assert!(
        log.contains("native.requested demo req-1 external.openUrl -> local"),
        "log: {log}"
    );
    assert!(log.contains("native.completed demo req-1"), "log: {log}");
}

#[test]
fn native_cli_exposes_explicit_host_drain_service() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(home, &["native", "observe-default"]);
    assert!(ok, "observe default failed: {err}");
    assert!(out.contains("native.platform.observed"), "out: {out}");

    let (ok, out, err) = terrane(home, &["native", "drain-once"]);
    assert!(ok, "drain once failed: {err}");
    assert_eq!(out.trim(), "native drain idle");
}

#[test]
fn native_cli_observe_default_rejects_capture_on_unsupported_host() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");
    let (ok, out, err) = terrane(home, &["native", "observe-default"]);
    assert!(ok, "observe default failed: {err}");
    assert!(out.contains("native.platform.observed"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "camera.capture-photo",
            "demo",
            "photo-1",
            r#"{"facing":"user"}"#,
        ],
    );
    assert!(!ok, "unsupported camera request should fail: {out}");
    assert!(
        err.contains("native operation is not supported on this host: camera.capturePhoto"),
        "stderr: {err}"
    );

    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "audio.record",
            "demo",
            "audio-1",
            r#"{"maxDurationMs":1000}"#,
        ],
    );
    assert!(!ok, "unsupported audio request should fail: {out}");
    assert!(
        err.contains("native operation is not supported on this host: audio.record"),
        "stderr: {err}"
    );
}

#[test]
fn native_capture_stub_executor_links_blobs_and_completes() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let mut core = terrane_host::open_at_home(home).unwrap();

    terrane_host::dispatch_on_core(
        &mut core,
        "app.add",
        &["demo".into(), "Demo".into()],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "native.platform.observe",
        &[
            "stub".into(),
            "test".into(),
            "capture-test-1".into(),
            "camera.capturePhoto".into(),
            "audio.record".into(),
        ],
    )
    .unwrap();

    terrane_host::dispatch_on_core(
        &mut core,
        "native.camera.capture-photo",
        &[
            "demo".into(),
            "photo-1".into(),
            r#"{"facing":"environment","maxWidth":640}"#.into(),
        ],
    )
    .unwrap();
    terrane_host::blob_store::insert_if_absent(home, PHOTO_HASH, b"fake-jpeg").unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "blob.link",
        &[
            "demo".into(),
            "__capture__/photo-1".into(),
            PHOTO_HASH.into(),
            "9".into(),
            "image/jpeg".into(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "native.complete",
        &[
            "demo".into(),
            "photo-1".into(),
            format!(
                r#"{{"hash":"{PHOTO_HASH}","size":9,"mime":"image/jpeg","width":1,"height":1,"blobName":"__capture__/photo-1"}}"#
            ),
        ],
    )
    .unwrap();

    terrane_host::dispatch_on_core(
        &mut core,
        "native.audio.record",
        &[
            "demo".into(),
            "audio-1".into(),
            r#"{"maxDurationMs":1000,"sampleRateHz":16000}"#.into(),
        ],
    )
    .unwrap();
    terrane_host::blob_store::insert_if_absent(home, AUDIO_HASH, b"fake-wav").unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "blob.link",
        &[
            "demo".into(),
            "__capture__/audio-1".into(),
            AUDIO_HASH.into(),
            "8".into(),
            "audio/wav".into(),
        ],
    )
    .unwrap();
    terrane_host::dispatch_on_core(
        &mut core,
        "native.complete",
        &[
            "demo".into(),
            "audio-1".into(),
            format!(
                r#"{{"hash":"{AUDIO_HASH}","size":8,"mime":"audio/wav","durationMs":1000,"sampleRateHz":16000,"blobName":"__capture__/audio-1"}}"#
            ),
        ],
    )
    .unwrap();

    let photo = &core.state().native.requests["demo"]["photo-1"];
    assert_eq!(photo.status.as_str(), "completed");
    assert!(photo.result_json.as_deref().unwrap().contains(PHOTO_HASH));
    let audio = &core.state().native.requests["demo"]["audio-1"];
    assert_eq!(audio.status.as_str(), "completed");
    assert!(audio.result_json.as_deref().unwrap().contains(AUDIO_HASH));
    assert_eq!(
        core.state().blob.blobs["demo"]["__capture__/photo-1"].mime,
        "image/jpeg"
    );
    assert_eq!(
        core.state().blob.blobs["demo"]["__capture__/audio-1"].mime,
        "audio/wav"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn native_cli_records_v2_requests_and_stub_completion() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "native",
            "platform.observe",
            "local",
            "macos",
            "test-1",
            "clipboard.readText",
            "screen.capture",
            "tray.setMenu",
        ],
    );
    assert!(ok, "platform observe failed: {err}");

    let (ok, out, err) = terrane(home, &["native", "clipboard.read-text", "demo", "clip-1"]);
    assert!(ok, "clipboard read request failed: {err}");
    assert!(out.contains("native.requested"), "out: {out}");
    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "complete",
            "demo",
            "clip-1",
            r#"{"text":"hello","truncated":false}"#,
        ],
    );
    assert!(ok, "clipboard read complete failed: {err}");
    assert!(out.contains("native.completed"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "tray.set-menu",
            "demo",
            "tray-1",
            "Demo",
            r#"[{"id":"open","label":"Open"}]"#,
        ],
    );
    assert!(ok, "tray request failed: {err}");
    assert!(out.contains("native.requested"), "out: {out}");
    let (ok, _, err) = terrane(
        home,
        &["native", "complete", "demo", "tray-1", r#"{"installed":true}"#],
    );
    assert!(ok, "tray complete failed: {err}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("native.requested demo clip-1 clipboard.readText -> local"));
    assert!(log.contains("native.requested demo tray-1 tray.setMenu -> local"));
}

#[test]
#[ignore = "requires real camera/microphone hardware plus macOS TCC or browser getUserMedia consent"]
fn native_real_capture_operations_require_hardware_and_tcc() {}
