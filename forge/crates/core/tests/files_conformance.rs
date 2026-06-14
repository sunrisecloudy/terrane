//! Data-driven T028 conformance over the `ctx.files` capability vectors
//! (`forge/fixtures/files/`, manifest `count = 14`).
//!
//! This proves the **wired** `ctx.files` path of forge-core (CR-3): every op is
//! driven through the runtime's [`HostContext`](forge_runtime::HostContext) sitting
//! on top of forge-core's real [`StorageHostBridge`] — the same gate `runtime.run`
//! uses. The files grant comes from the **TRUSTED manifest snapshot** (the
//! manifest's `capabilities.files`, evaluated by the [`PolicyEngine`] into the
//! recorded [`PermissionSnapshot`]), never the request payload; the sandbox handle
//! root + seeded files come from the bridge's injected [`InMemoryFileSystem`]
//! (the trusted handle → per-applet-root resolution). So a vector exercises the
//! full chain: trusted grant → handle-root resolution → path confinement →
//! byte/content-type caps → record/replay.
//!
//! Each fixture pins one (or a sequence of) file op(s), the manifest grant, the
//! resolved handle root, the seeded filesystem, and the expected outcome:
//!   * `allowed` → the op succeeds and the response matches the vector;
//!   * `capability_required` → a `CapabilityRequired` denial (absent grant /
//!     outside grant / no write grant);
//!   * `permission_denied` → a `PermissionDenied` (traversal / absolute / symlink
//!     escape);
//!   * `not_found` → a clean `StorageError` carrying `not_found`;
//!   * `replay_byte_identical` → a recorded read replays its recorded bytes with
//!     the live filesystem emptied (CR-8).
//!
//! For every error vector the harness asserts BOTH the `CoreError` code AND the
//! vector's `error.detail_contains` substring against the live runtime message —
//! so the *reason* (not just the code) matches the spec vector, and a wording
//! drift on either side fails the test.
//!
//! The `ran == 14` guard means a renamed/dropped vector FAILS the test rather than
//! silently skipping (it equals the suite manifest's `count`).

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use forge_core::StorageHostBridge;
use forge_domain::{ActorContext, Capabilities, Limits, Manifest};
use forge_runtime::{
    FileReadRequest, FileWriteRequest, HostContext, InMemoryFileSystem, NullBridge, RunRecorder,
};
use forge_storage::Store;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/core
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/files")
        .canonicalize()
        .expect("files fixtures dir exists")
}

/// Build a manifest whose `capabilities.files` is the fixture's `grant.files`
/// (the TRUSTED grant the runtime gates against). An absent `files` key (the
/// `absent_files_capability_rejected` vector's `grant: {}`) deserializes to an
/// empty `FilesGrant` ⇒ no file access at all.
fn manifest_with_files_grant(grant: &Value) -> Manifest {
    let files = grant
        .get("files")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let capabilities: Capabilities = serde_json::from_value(serde_json::json!({
        "storage": { "read": [], "write": [] },
        "db": { "read": [], "write": [] },
        "ui": true,
        "files": files,
    }))
    .expect("fixture grant.files deserializes into Capabilities.files");
    Manifest {
        entrypoint: "src/main.ts".into(),
        min_api: "forge-api@0.1".into(),
        deterministic: true,
        capabilities,
        limits: Limits {
            wall_ms: 3000,
            fuel: 10_000_000,
            memory_bytes: 67_108_864,
            max_host_calls: 10_000,
            storage_bytes: 10_485_760,
            log_bytes: 262_144,
        },
        compatibility: Default::default(),
    }
}

/// Build the trusted sandbox filesystem from the fixture: grant a root per
/// `resolved_handles` entry, seed every `filesystem.files` entry (base64 →
/// bytes), and mark every escaping `filesystem.symlinks` entry. This is the host
/// policy the bridge injects — the manifest never names a native root.
fn build_filesystem(resolved_handles: &Value, filesystem: &Value) -> InMemoryFileSystem {
    let mut fs = InMemoryFileSystem::new();
    if let Some(handles) = resolved_handles.as_object() {
        for (handle, info) in handles {
            let root = info
                .get("root")
                .and_then(|v| v.as_str())
                .unwrap_or("<applet-sandbox>");
            fs = fs.with_handle_root(handle.clone(), root.to_string());
        }
    }
    // A handle every seeded file/symlink belongs to: the suite is single-handle
    // (`workspace_data`), so resolve it from the first granted root.
    let handle = resolved_handles
        .as_object()
        .and_then(|h| h.keys().next().cloned())
        .unwrap_or_else(|| "workspace_data".to_string());

    if let Some(files) = filesystem.get("files").and_then(|f| f.as_array()) {
        for f in files {
            let path = f["path"].as_str().expect("file.path");
            let bytes = BASE64
                .decode(f["bytes_base64"].as_str().expect("file.bytes_base64"))
                .expect("file.bytes_base64 is valid base64");
            let content_type = f.get("content_type").and_then(|v| v.as_str());
            fs = fs.with_file(handle.clone(), path.to_string(), bytes, content_type);
        }
    }
    if let Some(symlinks) = filesystem.get("symlinks").and_then(|s| s.as_array()) {
        for s in symlinks {
            if s.get("target_resolves_outside_root")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                let path = s["path"].as_str().expect("symlink.path");
                fs = fs.with_escaping_symlink(handle.clone(), path.to_string());
            }
        }
    }
    fs
}

/// Parse a fixture `op` object into a typed read/write request.
enum Op {
    Read(FileReadRequest),
    Write(FileWriteRequest),
}

fn parse_op(op: &Value) -> Op {
    let handle = op["handle"].as_str().expect("op.handle").to_string();
    let path = op["path"].as_str().expect("op.path").to_string();
    match op["kind"].as_str().expect("op.kind") {
        "read" => Op::Read(FileReadRequest {
            handle,
            path,
            encoding: op
                .get("encoding")
                .and_then(|v| v.as_str())
                .unwrap_or("base64")
                .to_string(),
        }),
        "write" => Op::Write(FileWriteRequest {
            handle,
            path,
            bytes_base64: op["bytes_base64"].as_str().expect("write.bytes_base64").to_string(),
            content_type: op.get("content_type").and_then(|v| v.as_str()).map(String::from),
            mode: op
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("create_or_truncate")
                .to_string(),
        }),
        other => panic!("unknown op.kind {other:?}"),
    }
}

/// The CoreError code the vector's `expect`/`error.kind` predicts.
fn expected_error_code(fx: &Value) -> Option<&'static str> {
    match fx["expect"].as_str().expect("expect") {
        "allowed" | "replay_byte_identical" => None,
        "rejected" | "error" => Some(match fx["error"]["kind"].as_str().expect("error.kind") {
            "CapabilityRequired" => "CapabilityRequired",
            "PermissionDenied" => "PermissionDenied",
            "StorageError" => "StorageError",
            "ResourceLimitExceeded" => "ResourceLimitExceeded",
            "ValidationError" => "ValidationError",
            other => panic!("unknown error.kind {other:?}"),
        }),
        other => panic!("unknown expect {other:?}"),
    }
}

/// Run one op through a fresh `HostContext` on a `StorageHostBridge` seeded with
/// the fixture's trusted filesystem + manifest grant, returning the JSON response
/// (on success) or the error code + message (on failure).
fn run_op(grant: &Value, resolved_handles: &Value, filesystem: &Value, op: &Value) -> Result<Value, (String, String)> {
    let manifest = manifest_with_files_grant(grant);
    let actor = ActorContext::owner("dev");
    let mut store = Store::open_in_memory().expect("store");
    let fs = build_filesystem(resolved_handles, filesystem);
    let mut bridge = StorageHostBridge::new(&mut store, "app_files").with_file_system(Box::new(fs));
    let mut host = HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge)
        .expect("host context");
    match parse_op(op) {
        Op::Read(req) => host
            .files_read(req)
            .map(|r| serde_json::to_value(r).unwrap())
            .map_err(|e| (e.code().to_string(), e.to_string())),
        Op::Write(req) => host
            .files_write(req)
            .map(|r| serde_json::to_value(r).unwrap())
            .map_err(|e| (e.code().to_string(), e.to_string())),
    }
}

#[test]
fn files_t028_vectors_match_expected_outcome() {
    let dir = fixtures_dir();
    let mut ran = 0usize;

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read files fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| p.file_name().and_then(|n| n.to_str()) != Some("manifest.json"))
        .collect();
    entries.sort();

    for path in entries {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let fx: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        let case = fx["case"].as_str().unwrap_or("<no-case>").to_string();
        let grant = &fx["grant"];
        let resolved = &fx["resolved_handles"];

        match fx["expect"].as_str().expect("expect") {
            // ---- the write-then-read sequence vector --------------------------
            "allowed" if fx.get("ops").is_some() => {
                // A single bridge/host must persist the write so the read-back
                // sees the bytes. Build one context and drive both ops on it.
                let manifest = manifest_with_files_grant(grant);
                let actor = ActorContext::owner("dev");
                let mut store = Store::open_in_memory().unwrap();
                let fs = build_filesystem(resolved, &fx["filesystem_before"]);
                let mut bridge =
                    StorageHostBridge::new(&mut store, "app_files").with_file_system(Box::new(fs));
                let mut host =
                    HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge)
                        .unwrap();

                let ops = fx["ops"].as_array().expect("ops");
                let responses = fx["responses"].as_array().expect("responses");
                assert_eq!(ops.len(), responses.len(), "[{case}] ops/responses length");
                for (op, want) in ops.iter().zip(responses) {
                    let got = match parse_op(op) {
                        Op::Read(req) => serde_json::to_value(host.files_read(req).expect("read-back")).unwrap(),
                        Op::Write(req) => serde_json::to_value(host.files_write(req).expect("write")).unwrap(),
                    };
                    assert_response_matches(&case, want, &got);
                }
            }
            // ---- single allowed op -------------------------------------------
            "allowed" => {
                let got = run_op(grant, resolved, &fx["filesystem"], &fx["op"])
                    .unwrap_or_else(|(code, msg)| panic!("[{case}] expected allowed, got {code}: {msg}"));
                assert_response_matches(&case, &fx["response"], &got);
            }
            // ---- deterministic replay (CR-8) ---------------------------------
            "replay_byte_identical" => {
                assert_replay_byte_identical(&case, grant, resolved, &fx);
            }
            // ---- rejected / error --------------------------------------------
            "rejected" | "error" => {
                let want_code = expected_error_code(&fx).unwrap();
                let (code, msg) = run_op(grant, resolved, &fx["filesystem"], &fx["op"])
                    .err()
                    .unwrap_or_else(|| panic!("[{case}] expected {want_code}, op was allowed"));
                assert_eq!(code, want_code, "[{case}] error code mismatch ({msg})");
                // Pin the vector's `detail_contains` against the LIVE runtime
                // message for EVERY error vector — not just `not_found` (review:
                // the harness must assert the reason matches the vector, mirroring
                // T034's per-vector `message_contains`). The runtime emits the
                // fixtures' stable error vocabulary (e.g. "is outside granted glob",
                // "manifest did not request files.<action>", "path traversal is not
                // allowed", "absolute paths are not allowed", "symlink target
                // escapes handle root", "not_found"), so a substring match is the
                // contract between the spec vectors and the runtime, not brittle
                // prose. A wording drift on either side now FAILS here.
                let detail = fx["error"]
                    .get("detail_contains")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("[{case}] error.detail_contains is required"));
                assert!(
                    msg.contains(detail),
                    "[{case}] runtime message must contain the vector's detail_contains.\n  \
                     want substring: {detail:?}\n  got message:     {msg:?}"
                );
            }
            other => panic!("[{case}] unknown expect {other:?}"),
        }

        ran += 1;
    }

    assert_eq!(ran, 14, "expected exactly 14 files (T028) vectors, ran {ran}");
}

/// Assert a `files.read`/`files.write` response matches the vector's expected
/// shape (path + bytes/size/content-type for a read; path + written_bytes +
/// version for a write).
fn assert_response_matches(case: &str, want: &Value, got: &Value) {
    assert_eq!(got["path"], want["path"], "[{case}] response.path");
    if want.get("bytes_base64").is_some() {
        // Read response: bytes are byte-exact (the base64 string is compared
        // directly, so a single flipped byte fails here).
        assert_eq!(got["bytes_base64"], want["bytes_base64"], "[{case}] response.bytes_base64");
        assert_eq!(got["size"], want["size"], "[{case}] response.size");
        assert_eq!(got["content_type"], want["content_type"], "[{case}] response.content_type");
    }
    if want.get("written_bytes").is_some() {
        assert_eq!(got["written_bytes"], want["written_bytes"], "[{case}] response.written_bytes");
        if want.get("version").is_some() {
            assert_eq!(got["version"], want["version"], "[{case}] response.version");
        }
    }
}

/// Record a read in the record phase, then replay it with the live filesystem
/// emptied and assert the recorded bytes are served byte-identically and the live
/// filesystem is never consulted (CR-8). Mirrors the net.fetch replay contract.
fn assert_replay_byte_identical(case: &str, grant: &Value, resolved: &Value, fx: &Value) {
    let record_phase = &fx["record_phase"];
    let manifest = manifest_with_files_grant(grant);
    let actor = ActorContext::owner("dev");

    // --- record phase: a real filesystem, a recording recorder ---
    let mut store = Store::open_in_memory().unwrap();
    let fs = build_filesystem(resolved, &record_phase["filesystem"]);
    let mut bridge = StorageHostBridge::new(&mut store, "app_files").with_file_system(Box::new(fs));
    let mut host =
        HostContext::new(&manifest, &actor, RunRecorder::recording(1, 0), &mut bridge).unwrap();
    let req = match parse_op(&record_phase["op"]) {
        Op::Read(r) => r,
        Op::Write(_) => panic!("[{case}] replay vector op must be a read"),
    };
    let recorded_resp = host.files_read(req).expect("record-phase read");
    let recorded_json = serde_json::to_value(&recorded_resp).unwrap();
    // The recorded response matches the vector's recorded_call response.
    assert_response_matches(case, &record_phase["recorded_call"]["response"], &recorded_json);
    let (recorder, _logs) = host.finish();
    let calls = recorder.into_calls();
    assert_eq!(calls.len(), 1, "[{case}] exactly one recorded call");
    assert_eq!(calls[0].method, "files.read", "[{case}] recorded method");

    // --- replay phase: the live filesystem is GONE (NullBridge); the recorder
    // serves the recorded bytes. A live read reaching the NullBridge would error,
    // proving the live filesystem is never consulted on replay (CR-8). ---
    let mut null = NullBridge::new();
    let mut replay_host = HostContext::with_policy(
        forge_policy::PolicyEngine::new(&manifest, &actor).unwrap(),
        manifest.limits.clone(),
        RunRecorder::replaying(1, 0, calls.clone()),
        &mut null,
    );
    let replay_req = match parse_op(&record_phase["op"]) {
        Op::Read(r) => r,
        Op::Write(_) => unreachable!(),
    };
    let replayed = replay_host.files_read(replay_req).expect("replay-phase read");
    let replayed_json = serde_json::to_value(&replayed).unwrap();
    // The replay serves the recorded bytes byte-identically (CR-8), even though
    // the live filesystem (NullBridge) holds nothing.
    assert_response_matches(case, &fx["replay_phase"]["expect_response"], &replayed_json);
    assert_eq!(replayed_json, recorded_json, "[{case}] replay must be byte-identical to record");
}
