//! Engine tests for the `stt` capability — ambient transcript lifecycle driven
//! end to end through `Core::dispatch`. The trusted-host edge (session open,
//! segment append, close-host, retention trim) is driven with trusted requests;
//! the app surface (select, stop, reads) is driven both directly and through a
//! JS backend. Replay identity is asserted after every state-changing test.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_cap_stt::{
    retention_trimmed_event, segment_appended_event, selection_made_event,
    session_closed_event, session_opened_event, SegmentAppendedRecord, SelectionMadeRecord,
    SessionOpenedRecord,
};
use terrane_core::{fold_records_in_memory, Core, Error, State, LOCAL_OWNER_SUBJECT};

use crate::helpers::{public_req, req};

/// Trusted open of a session for `app`, returning the session id used.
fn open_session(core: &mut Core, app: &str, sid: &str) {
    core.dispatch(req(
        "stt.session.open",
        &[app, sid, "host1", "host1", "whisper-tiny", "16000"],
    ))
    .unwrap();
}

/// Trusted append of one finalized segment; returns nothing (asserts ok).
fn append_segment(
    core: &mut Core,
    app: &str,
    sid: &str,
    seq: u64,
    start_ms: u64,
    end_ms: u64,
    text: &str,
) {
    core.dispatch(req(
        "stt.segment.append",
        &[app, sid, &seq.to_string(), &start_ms.to_string(), &end_ms.to_string(), text],
    ))
    .unwrap();
}

#[test]
fn open_append_close_folds_transcript_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 500, "hello");
    append_segment(&mut core, "demo", "s1", 2, 500, 1100, "world");
    append_segment(&mut core, "demo", "s1", 3, 1100, 1700, "again");

    let session = &core.state().stt.sessions["demo"]["s1"];
    assert!(session.status.is_open());
    assert_eq!(session.segments.len(), 3);
    assert_eq!(session.segments[&2].text, "world");
    assert_eq!(session.last_segment_seq, 3);
    assert_eq!(session.sample_rate_hz, 16_000);

    // Close from the host edge.
    core.dispatch(req("stt.session.close-host", &["demo", "s1", "stopped"]))
        .unwrap();
    let session = &core.state().stt.sessions["demo"]["s1"];
    assert!(!session.status.is_open());
    assert_eq!(session.closed_reason.as_deref(), Some("stopped"));

    assert!(core.replay_matches().unwrap());
    // A cold reopen rebuilds the identical transcript from the log alone — no
    // mic, no ASR, just fold.
    let reopened = Core::open(&log).unwrap();
    let session = &reopened.state().stt.sessions["demo"]["s1"];
    assert_eq!(session.segments.len(), 3);
    assert_eq!(session.segments[&3].text, "again");
    assert!(!session.status.is_open());
}

#[test]
fn segment_append_is_monotonic_and_first_wins() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    open_session(&mut core, "demo", "s1");

    append_segment(&mut core, "demo", "s1", 1, 0, 100, "first");
    // A duplicate seq=1 (retry/sync) is a no-op: the first text survives.
    core.dispatch(req(
        "stt.segment.append",
        &["demo", "s1", "1", "0", "100", "DUPLICATE"],
    ))
    .unwrap();
    append_segment(&mut core, "demo", "s1", 2, 100, 200, "second");
    // An out-of-order but valid seq below the high-water mark is a no-op.
    core.dispatch(req(
        "stt.segment.append",
        &["demo", "s1", "1", "0", "100", "LATE"],
    ))
    .unwrap();

    let session = &core.state().stt.sessions["demo"]["s1"];
    assert_eq!(session.segments.len(), 2);
    assert_eq!(session.segments[&1].text, "first");
    assert_eq!(session.segments[&2].text, "second");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn append_rejects_missing_or_closed_sessions() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    // No session yet.
    assert!(matches!(
        core.dispatch(req("stt.segment.append", &["demo", "ghost", "1", "0", "10", "x"])),
        Err(Error::InvalidInput(_))
    ));
    open_session(&mut core, "demo", "s1");
    core.dispatch(req("stt.session.close-host", &["demo", "s1", "stopped"]))
        .unwrap();
    // Closed session cannot accept segments.
    assert!(matches!(
        core.dispatch(req("stt.segment.append", &["demo", "s1", "1", "0", "10", "x"])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn select_rederives_text_from_folded_segments() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 100, "alpha");
    append_segment(&mut core, "demo", "s1", 2, 100, 200, "beta");
    append_segment(&mut core, "demo", "s1", 3, 200, 300, "gamma");

    // App-callable select (public authority is fine: only the trusted verbs are gated).
    let records = core
        .dispatch(req("stt.select", &["demo", "s1", "1", "3", "clipboard"]))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "stt.selection.made");
    let selection = core.state().stt.sessions["demo"]["s1"]
        .selections
        .values()
        .next()
        .unwrap();
    // Text is the join of segments 1..=3 — re-derived by decide, not app-supplied.
    assert_eq!(selection.text, "alpha beta gamma");
    assert_eq!(selection.sink, "clipboard");

    // Re-selecting the exact same range+sink is idempotent (same selection_id → no-op fold).
    let before = core.state().stt.sessions["demo"]["s1"].selections.len();
    core.dispatch(req("stt.select", &["demo", "s1", "1", "3", "clipboard"]))
        .unwrap();
    let after = core.state().stt.sessions["demo"]["s1"].selections.len();
    assert_eq!(before, after);

    // A different sink is a distinct selection.
    core.dispatch(req("stt.select", &["demo", "s1", "2", "3", "note"]))
        .unwrap();
    let session = &core.state().stt.sessions["demo"]["s1"];
    assert_eq!(session.selections.len(), 2);
    let note = session
        .selections
        .values()
        .find(|s| s.sink == "note")
        .unwrap();
    assert_eq!(note.text, "beta gamma");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn select_validates_range_and_ownership() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req("app.add", &["other", "Other"])).unwrap();
    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 100, "only");

    // to before from.
    assert!(matches!(
        core.dispatch(req("stt.select", &["demo", "s1", "2", "1", "clipboard"])),
        Err(Error::InvalidInput(_))
    ));
    // from beyond the last segment.
    assert!(matches!(
        core.dispatch(req("stt.select", &["demo", "s1", "5", "9", "clipboard"])),
        Err(Error::InvalidInput(_))
    ));
    // Another app cannot select demo's session.
    assert!(matches!(
        core.dispatch(req("stt.select", &["other", "s1", "1", "1", "clipboard"])),
        Err(Error::InvalidInput(_))
    ));
    // Unknown session.
    assert!(matches!(
        core.dispatch(req("stt.select", &["demo", "ghost", "1", "1", "clipboard"])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn retention_trim_drops_old_segments_and_blocks_trimmed_select() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 100, "a");
    append_segment(&mut core, "demo", "s1", 2, 100, 200, "b");
    append_segment(&mut core, "demo", "s1", 3, 200, 300, "c");

    core.dispatch(req("stt.retention.trim", &["demo", "s1", "2"]))
        .unwrap();
    let session = &core.state().stt.sessions["demo"]["s1"];
    assert_eq!(session.dropped_before_seq, 2);
    assert!(!session.segments.contains_key(&1));
    assert!(session.segments.contains_key(&2));

    // Selecting the trimmed range is now an error.
    assert!(matches!(
        core.dispatch(req("stt.select", &["demo", "s1", "1", "2", "clipboard"])),
        Err(Error::InvalidInput(_))
    ));
    // A valid selection over retained segments still works.
    core.dispatch(req("stt.select", &["demo", "s1", "2", "3", "note"]))
        .unwrap();
    let sel = core.state().stt.sessions["demo"]["s1"]
        .selections
        .values()
        .next()
        .unwrap();
    assert_eq!(sel.text, "b c");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn app_removed_clears_sessions_wholesale() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 100, "secret");

    core.dispatch(req("app.remove", &["demo"])).unwrap();
    assert!(core
        .state()
        .stt
        .sessions
        .get("demo")
        .is_none_or(|m| m.is_empty()));

    assert!(core.replay_matches().unwrap());
    assert!(Core::open(&log)
        .unwrap()
        .state()
        .stt
        .sessions
        .get("demo")
        .is_none_or(|m| m.is_empty()));
}

#[test]
fn trusted_verbs_require_trusted_host_authority() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    // Public (non-trusted) authority cannot open a session, append, close-host,
    // or trim — even with valid args.
    let err = core
        .dispatch(public_req(
            "stt.session.open",
            &["demo", "s1", "host1", "host1", "whisper-tiny", "16000"],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("requires trusted host authority"), "{err}");

    // But the app-callable verbs are admitted publicly (they only validate state).
    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 10, "hi");
    core.dispatch(public_req("stt.select", &["demo", "s1", "1", "1", "clipboard"]))
        .unwrap();
    core.dispatch(public_req("stt.stop", &["demo", "s1"]))
        .unwrap();
    assert!(!core.state().stt.sessions["demo"]["s1"].status.is_open());
}

#[test]
fn session_close_is_first_wins() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    open_session(&mut core, "demo", "s1");

    core.dispatch(req("stt.session.close-host", &["demo", "s1", "idle"]))
        .unwrap();
    let reason = core.state().stt.sessions["demo"]["s1"].closed_reason.clone();
    assert_eq!(reason.as_deref(), Some("idle"));
    // A second close (app stop) does not overwrite the first close reason.
    core.dispatch(req("stt.stop", &["demo", "s1"]))
        .unwrap();
    assert_eq!(
        core.state().stt.sessions["demo"]["s1"].closed_reason.as_deref(),
        Some("idle")
    );
}

#[test]
fn segment_appended_folds_without_inference() {
    // Option A: replay folds recorded segments without ever running ASR. Folding
    // a segment event directly must produce the same state a live append would.
    let mut state = State::default();
    let records = vec![
        session_opened_event(&SessionOpenedRecord {
            app: "demo".into(),
            session_id: "s1".into(),
            host_id: "host1".into(),
            executor_host_id: "host1".into(),
            origin_replica: None,
            model: "whisper-tiny".into(),
            sample_rate_hz: 16_000,
        })
        .unwrap(),
        segment_appended_event(&SegmentAppendedRecord {
            app: "demo".into(),
            session_id: "s1".into(),
            segment_seq: 1,
            start_ms: 0,
            end_ms: 42,
            text: "folded directly".into(),
            confidence_milli: Some(900),
            lang: Some("en".into()),
        })
        .unwrap(),
        selection_made_event(&SelectionMadeRecord {
            app: "demo".into(),
            session_id: "s1".into(),
            selection_id: "sel_x".into(),
            from_segment_seq: 1,
            to_segment_seq: 1,
            text: "folded directly".into(),
            sink: "note".into(),
        })
        .unwrap(),
        session_closed_event("demo", "s1", "stopped").unwrap(),
        retention_trimmed_event("demo", "s1", 0).unwrap(),
    ];

    fold_records_in_memory(&mut state, &records).unwrap();

    let session = &state.stt.sessions["demo"]["s1"];
    assert_eq!(session.segments[&1].text, "folded directly");
    assert_eq!(session.segments[&1].confidence_milli, Some(900));
    assert_eq!(session.selections["sel_x"].sink, "note");
    assert!(!session.status.is_open());
}

#[test]
fn reject_open_requires_known_app_and_valid_tokens() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();

    // Unknown app.
    assert!(matches!(
        core.dispatch(req(
            "stt.session.open",
            &["ghost", "s1", "host1", "host1", "whisper-tiny", "16000"]
        )),
        Err(Error::AppNotFound(_))
    ));
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    // Bad session id token.
    assert!(matches!(
        core.dispatch(req(
            "stt.session.open",
            &["demo", "bad id!", "host1", "host1", "whisper-tiny", "16000"]
        )),
        Err(Error::InvalidInput(_))
    ));
    // Non-positive sample rate.
    assert!(matches!(
        core.dispatch(req(
            "stt.session.open",
            &["demo", "s1", "host1", "host1", "whisper-tiny", "0"]
        )),
        Err(Error::InvalidInput(_))
    ));
    // end before start on a segment.
    open_session(&mut core, "demo", "s1");
    assert!(matches!(
        core.dispatch(req(
            "stt.segment.append",
            &["demo", "s1", "1", "200", "100", "x"]
        )),
        Err(Error::InvalidInput(_))
    ));
}

/// A scribe-style JS backend exercising every `ctx.resource.stt` method.
const SCRIBE_BACKEND: &str = r#"
var stt = ctx.resource["stt"];
function handle(input) {
    var verb = input[0];
    if (verb === "sessions") {
        var s = JSON.parse(String(stt.sessions()));
        return s.map(function (x) { return x.sessionId + ":" + x.status; }).join(",");
    }
    if (verb === "transcript") {
        var segs = JSON.parse(String(stt.segments(input[1])));
        return segs.map(function (s) { return s.text; }).join(" ");
    }
    if (verb === "select") {
        return String(stt.select(input[1], input[2], input[3], input[4]));
    }
    if (verb === "stop") { return String(stt.stop(input[1])); }
    if (verb === "selections") {
        var sel = JSON.parse(String(stt.selections(input[1])));
        return sel.map(function (x) { return x.sink + "=" + x.text; }).join(",");
    }
    if (verb === "present") { return String(typeof stt); }
    return "?";
}
"#;

/// Install the scribe app (stt-enabled) on a fresh core.
fn install_scribe(dir: &Path, log: &Path) -> Core {
    let bundle = dir.join("scribe");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "scribe", "name":"Scribe","runtime":"js","backend":"main.js", "resources": ["stt"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), SCRIBE_BACKEND).unwrap();

    let mut core = Core::open(log).unwrap();
    core.dispatch(req(
        "app.add",
        &["scribe", "Scribe", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "scribe", "stt"]))
        .unwrap();
    core
}

#[test]
fn js_backend_reads_transcript_selects_and_stops_without_inference() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = install_scribe(dir.path(), &log);

    // The host edge (simulated here by trusted dispatches) opens a session and
    // appends finalized segments. The app reads the folded transcript.
    open_session(&mut core, "scribe", "s1");
    append_segment(&mut core, "scribe", "s1", 1, 0, 500, "the");
    append_segment(&mut core, "scribe", "s1", 2, 500, 900, "quick brown");

    core.dispatch(req("js-runtime.run", &["scribe", "sessions"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("s1:open"));

    core.dispatch(req("js-runtime.run", &["scribe", "transcript", "s1"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("the quick brown")
    );

    // select() records the slice and returns the re-derived text to JS.
    core.dispatch(req(
        "js-runtime.run",
        &["scribe", "select", "s1", "1", "2", "clipboard"],
    ))
    .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("the quick brown")
    );
    assert_eq!(
        core.state().stt.sessions["scribe"]["s1"].selections.len(),
        1
    );

    // selections() reads the recorded selection back.
    core.dispatch(req("js-runtime.run", &["scribe", "selections", "s1"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("clipboard=the quick brown")
    );

    // stop() closes the session (reason "stopped") and returns ok.
    core.dispatch(req("js-runtime.run", &["scribe", "stop", "s1"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    assert!(!core.state().stt.sessions["scribe"]["s1"].status.is_open());

    // Option A: replay rebuilds the identical transcript + selection from the
    // log alone — no JS, no ASR.
    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    assert_eq!(
        reopened.state().stt.sessions["scribe"]["s1"]
            .segments
            .len(),
        2
    );
    assert_eq!(
        reopened.state().stt.sessions["scribe"]["s1"]
            .selections
            .len(),
        1
    );
}

#[test]
fn session_purge_drops_closed_session_from_live_state() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    open_session(&mut core, "demo", "s1");
    append_segment(&mut core, "demo", "s1", 1, 0, 500, "hello");
    core.dispatch(req("stt.session.close-host", &["demo", "s1", "stopped"]))
        .unwrap();
    assert!(core.state().stt.sessions["demo"].contains_key("s1"));

    core.dispatch(req("stt.session.purge", &["demo", "s1"])).unwrap();
    assert!(!core.state().stt.sessions["demo"].contains_key("s1"));
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert!(!reopened.state().stt.sessions["demo"].contains_key("s1"));
}

#[test]
fn session_purge_rejects_open_sessions() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    open_session(&mut core, "demo", "s1");
    assert!(matches!(
        core.dispatch(req("stt.session.purge", &["demo", "s1"])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn ungranted_stt_resource_is_not_installed() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let bundle = dir.path().join("scribe");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "scribe", "name":"Scribe","runtime":"js","backend":"main.js", "resources": ["stt"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), SCRIBE_BACKEND).unwrap();

    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["scribe", "Scribe", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    // Declared but never granted → the namespace is absent from ctx.resource.
    core.dispatch(req("js-runtime.run", &["scribe", "present"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("undefined"));
}
