# Implementation Notes: native v2

## Files Changed

- `rust/crates/terrane-cap-native/src/operations/{mod.rs,common.rs,desktop.rs}`: added v2 operation constants and promoted `screen.capture` / `tray.setMenu` catalog rows to live `v1`.
- `rust/crates/terrane-cap-native/src/{commands.rs,resources.rs,types.rs,events.rs,lib.rs,doc.rs,tests.rs}`: added command/resource aliases, input/result validation, operation selector grant metadata, folded tray/shortcut registrations, and docs.
- `rust/crates/terrane-cap-auth/src/lib.rs`: taught `auth.grant` / `auth.revoke` to resolve `native:clipboard.readText` and `native:screen.capture` through `native.operation.v1`.
- `rust/crates/terrane-core/src/lib.rs`: enforced exact operation grants for sensitive native runtime resource writes.
- `rust/crates/terrane-core/tests/cap/native.rs`: extended native replay, validation, registration, and selector coverage.
- `rust/crates/terrane-host/src/public_authz.rs` and `rust/crates/terrane-host/tests/public_authz.rs`: classified the expanded native command inventory for public callers.
- `rust/crates/terrane-host/tests/cap/native.rs`: added v2 CLI/request lifecycle e2e coverage and an ignored real-macOS/TCC placeholder.
- `docs/APP_API.md`: regenerated/updated the native resource method table.

## Key Design Choices

- Preserved the existing queued Commit-request model: app calls still emit `native.requested`; trusted hosts still finish via `native.completed`, `native.failed`, or `native.cancelled`.
- Kept `screen.capture` bytes out of native events. The completed result must be a blob reference shape with `{hash,size,mime,width,height,blobName}` and `mime == "image/png"`.
- Added durable `NativeState.tray_menus` and `NativeState.shortcuts`, folded only from successful completions. Replay rebuilds registration intent without touching the OS.
- Added `native.operation.v1` selector grants for the two sensitive operations and blocked `ctx.resource.native.clipboardReadText` / `screenCapture` unless the exact `native:<operation>` grant exists.
- Refused sensitive native direct public capability commands because the existing public command gate is namespace-level. The selector-enforced path is the app runtime resource path.

## Deviations / Follow-up Edge Work

- Real macOS/Web OS execution arms are not implemented in this Rust slice. The available host native seam is the existing connector queue/drain abstraction; tests cover trusted stub completion and mark real macOS chrome/TCC cases ignored. A platform host can now safely drain these request facts and complete them with validated JSON/blob refs.

## Shared Files Touched

- `docs/APP_API.md`
- `rust/crates/terrane-cap-auth/src/lib.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`

## Tests Proving Properties

- Happy path / lifecycle: `native::native_cli_records_v2_requests_and_stub_completion`, `native::native_request_lifecycle_is_replay_safe`
- Replay identity: `native::native_v2_validates_inputs_and_blob_ref_results`, `native::native_v2_folds_tray_shortcut_registrations_and_replays`
- Input and result validation: `native::native_v2_validates_inputs_and_blob_ref_results`
- Registration fold/replace/removal: `native::native_v2_folds_tray_shortcut_registrations_and_replays`, `native::app_removal_drops_native_request_state`
- Selector grant security: `native::native_sensitive_resource_methods_require_operation_selector_grants`, `public_authz::grantable_command_inventory_requires_explicit_extractors_or_refusal`
- E2E host path: `native::native_cli_records_v2_requests_and_stub_completion`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
