# Capture implementation notes

## Files changed

- `rust/crates/terrane-cap-native/src/operations/mod.rs`
- `rust/crates/terrane-cap-native/src/operations/common.rs`
- `rust/crates/terrane-cap-native/src/commands.rs`
- `rust/crates/terrane-cap-native/src/resources.rs`
- `rust/crates/terrane-cap-native/src/doc.rs`
- `rust/crates/terrane-cap-native/src/lib.rs`
- `rust/crates/terrane-cap-auth/src/lib.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/native.rs`
- `rust/crates/terrane-host/src/native/unsupported.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/native.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `docs/APP_API.md`

## Key design choices

- Implemented capture as new `native` operations, not a new capability crate:
  `camera.capturePhoto` and `audio.record`.
- Kept `screen.capture` in the existing desktop/native-v2 surface, per plan.
- Added app command aliases:
  `native.camera.capture-photo`, `native.audio.record`,
  `native.cameraCapturePhoto`, and `native.audioRecord`.
- Both new operations are `status: "v1"`, `safety: "sensitive"`,
  `policy: "refuse-until-selector"`, and `result_size: "blob-ref"`.
- Added operation-level grants:
  `native:camera.capturePhoto` and `native:audio.record`.
- Completion validation accepts only blob-reference JSON:
  camera uses `image/jpeg`; audio uses `audio/wav`; both require
  `hash`, `size`, `mime`, `blobName`, and operation-specific metadata.
- Completion validation enforces the capture blob name convention
  `__capture__/<request_id>` and rejects success results above 64 MiB.
- The unsupported CLI/native connector now advertises a conservative supported
  operation list that excludes sensitive capture operations, so default CLI
  observation rejects camera/mic requests before any edge effect.

## Deviations

- Real AVFoundation and browser `getUserMedia` executors were not added in this
  slice. The current Rust host native connector remains an unsupported edge
  adapter; the default-run host e2e proves the recorded-fact/CAS sequence with a
  stub executor, and the real hardware/TCC test is ignored with an explicit
  reason.

## Shared files touched

- `rust/crates/terrane-cap-auth/src/lib.rs`: native operation-grant allowlist.
- `rust/crates/terrane-core/src/lib.rs`: sensitive native resource selectors.
- `rust/crates/terrane-host/src/public_authz.rs`: public command refusal for
  sensitive native capture commands.
- `rust/crates/terrane-host/tests/public_authz.rs`: registered command
  inventory counts.
- `docs/APP_API.md`: resource method table.

## Tests proving behavior

- `native::native_capture_requests_complete_with_blob_refs_and_replay`
  proves camera/audio request facts, `blob.link` metadata, completion facts,
  and replay identity.
- `native::native_capture_input_limits_are_validated`
  proves typed validation for camera facing and audio duration limits.
- `native::native_sensitive_resource_methods_require_operation_selector_grants`
  proves operation-level selector grants for `cameraCapturePhoto`.
- `native::native_cli_observe_default_rejects_capture_on_unsupported_host`
  proves the CLI/unsupported path rejects camera/mic capture before execution.
- `native::native_capture_stub_executor_links_blobs_and_completes`
  proves the host-side stub executor sequence: write fixture bytes to CAS,
  record `blob.link`, then record `native.completed`.
- `native::native_real_capture_operations_require_hardware_and_tcc`
  is ignored with reason:
  `requires real camera/microphone hardware plus macOS TCC or browser getUserMedia consent`.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
