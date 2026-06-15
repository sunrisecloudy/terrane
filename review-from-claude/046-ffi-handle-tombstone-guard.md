# 046 FFI handle tombstone guard review

## Slice goal

Address review `078` P1: the exported C ABI must not create aliased mutable
access to `WorkspaceCore`, and `forge_core_close` must not be able to free the
same allocation while `forge_core_handle_command` or `forge_core_drain_events`
is in flight.

## Diff reviewed

Working-tree slice before commit.

## Files changed

- `forge/crates/ffi/src/lib.rs`
- `forge/crates/ffi/tests/ffi.rs`

## Commands run

- `cargo test -p forge-ffi --locked` passed: 17 tests.
- `cargo clippy -p forge-ffi --all-targets --locked -- -D warnings` passed.
- `git diff --check -- forge/crates/ffi/src/lib.rs forge/crates/ffi/tests/ffi.rs` passed.

## Review findings

- Review `078` P1 found that the old C ABI rebuilt `&mut WorkspaceCore` from the
  same raw pointer in command/drain calls and let `close` drop that same pointer.
  Resolution: `ForgeCoreHandle` now stores `Mutex<Option<WorkspaceCore>>`; command
  and drain lock before borrowing the core, and close `take()`s and drops the
  core at most once.
- A plain `Mutex<WorkspaceCore>` plus `Box::from_raw` close would still leave a
  close-vs-new-call use-after-free race. Resolution: close leaves a small
  tombstone handle allocated so duplicate or racing native calls fail closed with
  structured `ValidationError` instead of dereferencing freed memory.
- Mutex poisoning must not panic across FFI. Resolution: lock poisoning returns a
  structured runtime error and sets `last_error`.
- Subagent review suggested extra race coverage. Resolution: added regressions
  for closed handles, double close, close racing commands, close racing event
  drain, and command/drain serialization on one handle.

## Claude review status

The previous slice's external `claude -p` review attempt was rejected by the
sandbox reviewer because it would send private working-tree code to an external
service. I did not retry that path without explicit approval. This artifact uses
local subagent review plus the checked-in review `078` as the source.

## Follow-ups

- Review `078` P2 remains open: runtime/core still duplicate the `forge-secrets`
  contract instead of using `forge-secrets` as the shared production seam.
- Review `078` P3 Windows build and binding-generation follow-ups remain open.
- The tombstone design intentionally leaks one small handle shell per opened FFI
  handle while dropping the workspace core on close. A future registry/refcount
  handle table could reclaim shells without weakening close-vs-call safety.
