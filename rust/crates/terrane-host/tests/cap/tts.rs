use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn tts_cli_validation_paths_are_typed_and_local() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(home, &["tts", "render", "ghost", "hello"]);
    assert!(!ok, "missing app should fail: {out}");
    assert!(err.contains("app not found") || out.contains("app not found"), "out={out} err={err}");

    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");

    let (ok, out, err) = terrane(home, &["tts", "render", "demo", "--rate", "499", "hello"]);
    assert!(!ok, "bad rate should fail: {out}");
    assert!(
        err.contains("rate_milli") || out.contains("rate_milli"),
        "out={out} err={err}"
    );

    let (ok, out, err) = terrane(home, &["tts", "render", "demo", "--voice", "bad voice", "hello"]);
    assert!(!ok, "bad voice should fail: {out}");
    assert!(err.contains("voice") || out.contains("voice"), "out={out} err={err}");
}

#[test]
fn tts_renders_read_empty_folded_state() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");

    let (ok, out, err) = terrane(home, &["tts", "renders", "demo"]);
    assert!(ok, "tts renders failed: {err}");
    assert_eq!(out.trim(), "[]");
}

#[test]
fn tts_help_lists_speak_and_render() {
    let dir = tempdir().unwrap();
    let (ok, out, err) = terrane(dir.path(), &["help"]);
    assert!(ok, "help failed: {err}");
    assert!(out.contains("terrane tts speak"), "help: {out}");
    assert!(out.contains("terrane tts render"), "help: {out}");
}

#[test]
#[ignore = "runs real macOS speech synthesis"]
fn tts_render_writes_blob_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");

    let (ok, out, err) = terrane(home, &["tts", "render", "demo", "hello from terrane"]);
    assert!(ok, "tts render failed: out={out} err={err}");
    assert!(out.contains("blob.stored"), "render out: {out}");
    assert!(out.contains("tts.rendered"), "render out: {out}");
    assert!(home.join("blobs.sqlite3").is_file());

    let (ok, out, err) = terrane(home, &["tts", "renders", "demo"]);
    assert!(ok, "tts renders failed: {err}");
    assert!(out.contains("\"blobHash\""), "renders out: {out}");

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: out={out} err={err}");
    assert!(out.contains("replay ok"), "replay out: {out}");
}
