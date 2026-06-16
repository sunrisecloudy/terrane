# Review 072: core ctx.net bridge wiring (`95ea1cf6`)

Claude, the injected HTTP seam is a clean place to keep real I/O out of the runtime crate, and the default fail-closed client is the right safety posture. I found three follow-ups that are worth fixing before treating this as the SC-5/CR-8 core spine.

## Findings

- **P1: deterministic manifests can still make live HTTP calls in record mode.** `prd-merged/01-core-runtime-prd.md:52` says live network is forbidden in deterministic mode unless the run is marked non-deterministic or is replaying a recorded fixture. This commit wires whatever `set_http_client_factory` returns straight into `record_run` (`forge/crates/core/src/workspace.rs:443`, `forge/crates/core/src/workspace.rs:460`), and the new net fixture is explicitly `"deterministic": true` while still exercising an injected client (`forge/crates/core/tests/spine.rs:1752`). Please split live HTTP from fixture/mock replay at the run configuration level and deny live clients for deterministic manifests before `HttpClient::send`.

- **P1: allowed-but-no-client failures are saved, but they do not replay to the same error.** `NoNetworkClient::send` returns `PlatformUnavailable` (`forge/crates/core/src/bridge.rs:90`), `cmd_runtime_run` persists the resulting failed `RunRecord` (`forge/crates/core/src/workspace.rs:499`), but `RunRecorder::host_call` only appends a `RecordedCall` after `live()?` succeeds (`forge/crates/runtime/src/recorder.rs:221`). Replay then runs with `NullBridge` and an empty trace (`forge/crates/core/src/workspace.rs:595`), so the applet's `ctx.net.fetch` becomes a determinism divergence ("extra host call") instead of the original `PlatformUnavailable`. The new `net_fetch_with_no_injected_client_fails_closed_platform_unavailable` test stops before replay (`forge/crates/core/tests/spine.rs:1974`). Please record bridge/client errors in a replayable host-error shape, or model "no client configured" as a pre-live recorded denial.

- **P1: response-policy failures can still persist rejected response bodies as successful host responses.** Runtime records the bridge response before enforcing response content-type/size caps (`forge/crates/runtime/src/host.rs:392`, `forge/crates/runtime/src/host.rs:412`), and core persists failed runs (`forge/crates/core/src/workspace.rs:499`). That means an oversized or wrong-content-type response is denied to JS, but the rejected body can remain in the run log as a normal `net.fetch` response. Please validate response caps before appending the recorded host call, or record only a redacted denial/error entry and add a core spine regression for wrong content type / oversized body.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core --all-targets -- -D warnings`
- `git diff --check 95ea1cf6^ 95ea1cf6`
- `git show --check --format=short 95ea1cf6`
