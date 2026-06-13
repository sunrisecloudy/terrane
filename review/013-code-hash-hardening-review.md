# 013 - Code Hash Hardening Review

Reviewed commit: `85fa860` (`forge-domain: fix harden code_hash provenance contract`)

The canonical hash validator is a good domain primitive: it nails the `sha256:` shape, rejects the old `fnv1a64:` form, and has solid known-answer tests. The remaining issue is that the commit says this is enforced at the record boundary, but no current boundary calls it.

## Findings

- **P1 - `validate_code_hash()` is still opt-in, so non-canonical run records are still produced and accepted.** `RunRecord::validate_code_hash()` correctly rejects non-canonical hashes (`forge/crates/domain/src/run.rs:76-96`), but `runtime::record_run` / `finish_run` never call it before returning a `RunRecord` (`forge/crates/runtime/src/runner.rs:90-121`), and `runtime::Program::code_hash()` still emits `fnv1a64:` (`forge/crates/runtime/src/lib.rs:67-77`). `replay()` also only compares `program.code_hash()` to `run.code_hash` without validating either (`forge/crates/runtime/src/runner.rs:64-74`). Storage persists and loads raw `RunRecord` JSON without validating (`forge/crates/storage/src/lib.rs:506-537`). This means the exact old bad record shape can still be generated, saved, loaded, and replayed unless callers remember to invoke the new method manually. Wire validation into record construction, replay input, and storage save/load, then add a cross-crate test proving current runtime cannot emit or accept `fnv1a64:`.

- **P2 - The docs overstate enforcement before the callers exist.** The new comment says "Recording and replay paths call this before trusting the record's provenance" (`forge/crates/domain/src/run.rs:81-86`), and `hash.rs` says recorder/replayer use the predicate, but `rg` shows no caller outside domain tests. Until runtime/storage are wired, soften those docs or make them true in the same patch; otherwise future work may assume this gap is already closed.

## Verification

- `cargo test --locked -p forge-domain` passed.
- `cargo clippy --locked -p forge-domain --all-targets -- -D warnings` passed.
- `cargo build --locked -p forge-domain --target wasm32-unknown-unknown` passed.

## Handoff Check

- No new handoff files beyond the already completed T004/T005 files.
