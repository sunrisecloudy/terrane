//! Unit tests for the subprocess-output and SSE parsers plus runtime
//! resolution. Engine behaviour with real weights lives in `tests/engine.rs`
//! (`#[ignore]`d).

use crate::mlx::{
    extract_json_object, parse_generation_tokens, parse_worker_line_for_tests, WorkerEventForTests,
};

#[test]
fn generation_tokens_parse_from_mlx_stats() {
    let stats = "\nPrompt: 23 tokens, 25.278 tokens-per-sec\n\
                 Generation: 64 tokens, 410.321 tokens-per-sec\n\
                 Peak memory: 0.514 GB\n";
    assert_eq!(parse_generation_tokens(stats), Some(64));
    assert_eq!(parse_generation_tokens("no stats here"), None);
}

#[test]
fn json_object_extraction_survives_prose_and_fences() {
    let wrapped = "Thinking about it...\n```json\n{\"answer\": \"Paris\"}\n```\nHope that helps!";
    assert_eq!(
        extract_json_object(wrapped).as_deref(),
        Some("{\"answer\": \"Paris\"}")
    );

    assert_eq!(extract_json_object("no json at all"), None);
    assert_eq!(extract_json_object("{broken"), None);
    // An array is not the requested object shape.
    assert_eq!(extract_json_object("[1, 2]"), None);
}

#[test]
fn worker_lines_parse_deltas_done_and_errors() {
    // Text delta.
    match parse_worker_line_for_tests(r#"{"t": "hi"}"#) {
        WorkerEventForTests::Delta(piece) => assert_eq!(piece, "hi"),
        other => panic!("expected delta, got {other:?}"),
    }

    // Terminal record with engine stats and the constraint mode.
    match parse_worker_line_for_tests(
        r#"{"done": true, "tokens": 256, "genTps": 380.1, "promptTps": 542.0, "finish": "length", "constrained": "mask"}"#,
    ) {
        WorkerEventForTests::Done {
            tokens,
            finish_reason,
            constrained,
        } => {
            assert_eq!(tokens, Some(256));
            assert_eq!(finish_reason.as_deref(), Some("length"));
            assert_eq!(constrained.as_deref(), Some("mask"));
        }
        other => panic!("expected done, got {other:?}"),
    }

    // Worker-side failure.
    match parse_worker_line_for_tests(r#"{"error": "model not found"}"#) {
        WorkerEventForTests::Error(message) => assert!(message.contains("model not found")),
        other => panic!("expected error, got {other:?}"),
    }

    // Blank and malformed lines are skipped.
    assert!(matches!(
        parse_worker_line_for_tests(""),
        WorkerEventForTests::Skip
    ));
    assert!(matches!(
        parse_worker_line_for_tests("not-json"),
        WorkerEventForTests::Skip
    ));
    assert!(matches!(
        parse_worker_line_for_tests(r#"{"t": ""}"#),
        WorkerEventForTests::Skip
    ));
}

mod resolution {
    use std::fs;

    use crate::setup::{resolve_runtime, RuntimeSource};

    /// Manifest-based resolution: a manifest pointing at a runnable binary
    /// wins; one pointing at a missing binary is skipped. (The env-override
    /// branch is process-global state, so it is exercised only implicitly.)
    #[test]
    fn manifest_resolution_requires_a_runnable_binary() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join("engines")).unwrap();

        // `true` exists on every unix; use it as a stand-in runnable binary.
        fs::write(
            home.join("engines/mlx.json"),
            r#"{"generate_bin":"true","server_bin":"true","mlx_lm_version":"0.31.3","installed_by":"test"}"#,
        )
        .unwrap();
        let runtime = resolve_runtime(home).expect("manifest runtime resolves");
        assert_eq!(runtime.source, RuntimeSource::Manifest);
        assert_eq!(runtime.generate_bin, "true");
        assert_eq!(runtime.version.as_deref(), Some("0.31.3"));

        // A manifest pointing at a missing binary falls through (to PATH,
        // whose presence depends on the machine — so only assert no Manifest).
        fs::write(
            home.join("engines/mlx.json"),
            r#"{"generate_bin":"/nonexistent/mlx_lm.generate","server_bin":"/nonexistent/mlx_lm.server","mlx_lm_version":null,"installed_by":"test"}"#,
        )
        .unwrap();
        if let Some(runtime) = resolve_runtime(home) {
            assert_ne!(runtime.source, RuntimeSource::Manifest);
        }
    }
}

mod server_state {
    use std::fs;

    use crate::server::server_status;

    #[test]
    fn status_reports_not_running_without_state_or_with_a_dead_port() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        let status = server_status(home);
        assert!(!status.running);
        assert_eq!(status.pid, None);

        // A state file pointing at a dead socket reports not-running.
        fs::create_dir_all(home.join("engines")).unwrap();
        fs::write(
            home.join("engines/mlx-server.json"),
            r#"{"pid":4194000,"socket":"/nonexistent/mlx-worker.sock","started_unix":0}"#,
        )
        .unwrap();
        let status = server_status(home);
        assert!(!status.running);
        assert_eq!(status.socket, None);
    }
}
