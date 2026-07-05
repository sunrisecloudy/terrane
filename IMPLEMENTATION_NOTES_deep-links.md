# Deep Links Implementation Notes

## Files changed

- `rust/crates/terrane-cap-app/src/lib.rs`
- `rust/crates/terrane-cap-app/src/doc.rs`
- `rust/crates/terrane-cap-app/src/tests.rs`
- `rust/crates/terrane-cap-app/tests/capability.rs`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/actor.rs`
- `rust/crates/terrane-core/tests/cap/app.rs`
- `rust/crates/terrane-core/tests/cap/host.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-core/tests/cap/kv.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/include/terrane_host.h`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/deep_links.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/ffi.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/mcp_tests.rs`
- `rust/crates/terrane-host/src/preview.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/abi.rs`
- `rust/crates/terrane-host/tests/cap/deep_links.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `docs/APP_API.md`
- `host/macos/Sources/AppDelegate.swift`
- `host/macos/Sources/TerraneBridge.swift`
- `host/macos/project.yml`
- `host/web/src/js/app_shell.js`
- `Cargo.lock`

## Shared files touched

- `Cargo.lock`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `docs/APP_API.md`
- host ABI files: `rust/crates/terrane-host/include/terrane_host.h`, `rust/crates/terrane-host/src/ffi.rs`
- host shell files: `host/macos/*`, `host/web/src/js/app_shell.js`

## Design choices

- Implemented deep links as an extension of the existing `app` capability surface, not a new capability crate.
- Added replayable `app.link.registered` facts and folded them into `AppState.links`.
- `app.add` records default Terrane URL registrations plus manifest-declared file type registrations.
- Host-observed external opens are routed through trusted-host-only `app.link.deliver`.
- `app.link.deliver` validates the target app, kind, and 64 KiB inline payload limit, then emits an app-call effect to `common.receive`.
- Item URIs are parsed with `parse_item_uri` and delivered as `common.receive("link", {"item":"..."})`.
- File opens are resolved from folded app link registrations, offloaded through `blob.put`, and delivered as `common.receive("blob", {"name","hash","size","mime"})`.
- Public/untrusted `capability_command` callers cannot invoke `app.link.deliver`; only trusted host authority can.
- The host CLI exposes `terrane open <url-or-file>`.
- The native host has an FFI entry point and macOS open-file/open-URL handlers.
- The web host registers a best-effort browser protocol handler and consumes `#open` hashes for web-open routing.

## Deviations and notes

- No new `terrane-cap-deep-links` crate was added because the authoritative plan says this extends the `app` surface.
- The app capability does not depend on `terrane-cap-interop`; delivery is represented as an `Effect::AppCall`, avoiding a capability dependency cycle.
- Browser protocol handlers cannot claim the raw `terrane:` scheme, so the web host uses the browser-supported `web+terrane` scheme and routes through `/#open/...`.
- macOS document type declaration is static in this slice, while per-app file type registrations are stored as replayable catalog facts.

## Proof

- Registration and replay:
  - `app_capability_adds_queries_and_removes_apps`
  - `app_add_registers_filetypes_and_rejects_bad_link_specs`
  - `link_registrations_fold_and_replay`
- Trusted boundary, validation, and limits:
  - `app_link_deliver_validates_target_kind_and_size`
  - `public_link_delivery_requires_trusted_host_authority`
  - `public_command_inventory_covers_every_registered_command`
  - `terrane_open_rejects_unknown_filetype`
- Delivery through `common.receive`:
  - `terrane_open_send_url_delivers_link_via_common_receive`
  - `terrane_open_item_uri_delivers_item_focus_payload`
  - `terrane_open_registered_file_imports_blob_and_delivers_reference`
- ABI/MCP regressions:
  - `checked_in_c_header_declares_the_exported_abi`
  - `open_host_run_output_free_round_trip`
  - `capability_command_and_query_tools_use_core_without_protocol_errors`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
