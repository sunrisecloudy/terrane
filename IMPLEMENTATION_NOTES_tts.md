# TTS Capability Implementation Notes

## Files Changed

- Added `rust/crates/terrane-cap-tts/` with manifest, command validation, event folding, resource docs, and folded state.
- Added `rust/crates/terrane-core/tests/cap/tts.rs`.
- Added `rust/crates/terrane-host/src/tts_edge.rs`.
- Added `rust/crates/terrane-host/tests/cap/tts.rs`.
- Updated shared wiring in `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`, `rust/crates/terrane-host/Cargo.toml`, `rust/crates/terrane-host/src/lib.rs`, and `rust/crates/terrane-cap-interface/src/abi.rs`.
- Updated host CLI/public surfaces in `rust/crates/terrane-host/src/cli.rs`, `rust/crates/terrane-host/src/edge.rs`, `rust/crates/terrane-host/src/public_authz.rs`, and related inventory tests.
- Updated `docs/APP_API.md` resource reference.

## Key Design Choices

- `tts.speak` returns `Decision::TransientEffect(Effect::TtsSpeak)` and records nothing.
- `tts.render` returns `Decision::Effect(Effect::TtsRender)`. The host edge synthesizes with `/usr/bin/say` on macOS, writes bytes to the blob CAS, then returns `blob.stored` and `tts.rendered`.
- `tts.rendered` records `{app, text_hash, voice, rate_milli, blob_hash, size, mime, duration_ms}` and never records source text.
- Fold state stores render metadata per app by `text_hash` and tracks insertion order to keep the last 100 renders deterministically.
- `describe()` prints voice, duration, text-hash prefix, and blob hash, never text.
- Public `capability_command` gates `tts.speak` and `tts.render` on the app's `tts` grant. `tts.supports` is an allowed public query.

## Deviations

- Web shell `speechSynthesis` bridge was not implemented in this Rust worktree slice; the CLI/macOS edge path and core capability are implemented. Web render remains unsupported per plan.
- CLI `tts.speak` is implemented as a host convenience that validates the app and calls the edge directly, so it leaves no top-level transient command record.

## Shared Files Touched

- `Cargo.toml`
- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-cap-interface/src/abi.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-core/tests/cap/main.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`

## Test Proofs

- Decision shape: `tts::decide_shapes_are_transient_for_speak_and_recorded_for_render`.
- Recorded render + blob metadata + replay identity: `tts::render_records_blob_and_tts_metadata_and_replays`.
- Transient speak resource records nothing: `tts::speak_resource_returns_ok_and_records_nothing`.
- Render resource return JSON + event recording: `tts::render_resource_returns_json_and_records_artifact`.
- Validation typed errors: `tts::validation_errors_are_typed`, `tts::tts_cli_validation_paths_are_typed_and_local`.
- Keep-last-100 and `app.removed`: `tts::fold_keeps_last_100_renders_per_app_and_app_removed_clears`.
- Description text privacy: `tts::describe_never_prints_source_text`.
- Host CLI/read surface: `tts::tts_help_lists_speak_and_render`, `tts::tts_renders_read_empty_folded_state`.
- Real synthesis e2e: `tts::tts_render_writes_blob_and_replays` is ignored with reason `runs real macOS speech synthesis`.

## Validation Run

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
