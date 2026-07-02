//! Unit tests for the subprocess-output and SSE parsers plus runtime
//! resolution. Engine behaviour with real weights lives in `tests/engine.rs`
//! (`#[ignore]`d).

use crate::mlx::{extract_json_object, parse_generation_tokens, parse_sse_line_for_tests};

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
fn sse_lines_parse_deltas_done_usage_and_keepalives() {
    // Content delta.
    let (content, finish, usage, done, skip) = parse_sse_line_for_tests(
        r#"data: {"object":"chat.completion.chunk","choices":[{"index":0,"finish_reason":null,"delta":{"role":"assistant","content":"hi"}}]}"#,
    );
    assert_eq!(content.as_deref(), Some("hi"));
    assert_eq!(finish, None);
    assert!(!done && !skip && usage.is_none());

    // Final chunk carries finish_reason without content.
    let (content, finish, _, done, _) = parse_sse_line_for_tests(
        r#"data: {"choices":[{"index":0,"finish_reason":"stop","delta":{"role":"assistant"}}]}"#,
    );
    assert_eq!(content, None);
    assert_eq!(finish.as_deref(), Some("stop"));
    assert!(!done);

    // Usage-only chunk (include_usage) has an empty choices array.
    let (content, _, usage, done, _) = parse_sse_line_for_tests(
        r#"data: {"choices":[],"usage":{"prompt_tokens":13,"completion_tokens":4,"total_tokens":17}}"#,
    );
    assert_eq!(content, None);
    assert_eq!(usage, Some(4));
    assert!(!done);

    // Terminator, keepalive comment, and blank separator.
    assert!(parse_sse_line_for_tests("data: [DONE]").3);
    assert!(parse_sse_line_for_tests(": keepalive 3/17").4);
    assert!(parse_sse_line_for_tests("").4);
    assert!(parse_sse_line_for_tests("data: not-json").4);
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

        // A state file pointing at a dead port reports not-running.
        fs::create_dir_all(home.join("engines")).unwrap();
        fs::write(
            home.join("engines/mlx-server.json"),
            r#"{"pid":4194000,"port":1,"started_unix":0}"#,
        )
        .unwrap();
        let status = server_status(home);
        assert!(!status.running);
        assert_eq!(status.port, None);
    }
}
