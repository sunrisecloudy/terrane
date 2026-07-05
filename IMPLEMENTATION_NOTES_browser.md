# Browser Capability Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-browser/`
  - `src/lib.rs`: `browser` capability, `browser.rendered` event constructor/fold/describe, `browser.render` recorded effect, `browser.peek` transient effect, 30 recorded renders per backend run.
  - `src/request.rs`: request parsing, validation, canonical JSON, redacted JSON, request key hashing, limits.
  - `src/doc.rs`: capability docs, resource docs, security/replay/limit notes.
  - `tests/capability.rs`: public capability tests.
- Updated shared workspace wiring:
  - `Cargo.toml`
  - `Cargo.lock`
  - `rust/crates/terrane-core/Cargo.toml`
  - `rust/crates/terrane-host/Cargo.toml`
- Updated ABI/core registry/state:
  - `rust/crates/terrane-cap-interface/src/abi.rs`
  - `rust/crates/terrane-core/src/lib.rs`
- Updated host edge/CLI/public authz:
  - `rust/crates/terrane-host/src/edge.rs`
  - `rust/crates/terrane-host/src/cli.rs`
  - `rust/crates/terrane-host/src/public_authz.rs`
- Updated docs and inventory tests:
  - `docs/APP_API.md`
  - `rust/crates/terrane-core/tests/cap/interface.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/public_authz.rs`
- Added tests:
  - `rust/crates/terrane-core/tests/cap/browser.rs`
  - `rust/crates/terrane-host/tests/cap/browser.rs`

## Key design choices

- `browser.render` returns `Decision::Effect(Effect::BrowserRender { app, request })`; replay folds only `browser.rendered` and never re-renders.
- `browser.peek` returns `Decision::TransientEffect(Effect::BrowserRender { ... })`, so app backends can inspect a page without recording a render.
- `request_key` is SHA-256 of canonical request JSON. Folded state is keyed by `app/request_key`.
- Event payloads are built only through `encode_event`; the capability never sets `actor`.
- Text/html outputs inline at or below 256 KiB and otherwise offload through `blob`; screenshot/pdf always offload to `blob` under `__browser__/{request_key}`.
- Edge rendering uses an ephemeral Chrome/Chromium profile (`--user-data-dir` in a temp dir) and removes it after the render.
- URL policy mirrors net-v2: `http`/`https` only, cloud metadata IP denied after DNS/IP handling, localhost allowed. Optional `allowedHosts` narrows request hosts further.
- Public `capability_command` access is grant-gated for `browser.render` with the app id at arg 0.

## Deviations

- The Rust CLI/web host implements the planned Chromium fallback. The macOS WKWebView-first runner is not implemented in this slice because this worktree does not expose a mac-native host-services runner seam for WKWebView. The edge boundary is ready for a mac host to route `Effect::BrowserRender` to WKWebView and return the same `browser.rendered`/`blob.stored` records.
- The default browser e2e treats a locally installed browser that aborts in the sandboxed headless test environment as a skipped engine case. The metadata-blocking test remains strict, and screenshot/blob rendering is covered by an ignored smoke test for environments with a working browser engine.

## Proof tests

- Happy path and fold:
  - `terrane-cap-browser tests/capability.rs::browser_render_canonicalizes_redacts_and_returns_effect`
  - `terrane-cap-browser tests/capability.rs::browser_folds_recorded_render_and_cleans_removed_app`
  - `terrane-core tests/cap/browser.rs::browser_render_resource_records_and_replays_identically`
- Replay identity:
  - `terrane-core tests/cap/browser.rs::browser_render_resource_records_and_replays_identically`
  - `rust/crates/terrane-host/tests/cap/browser.rs::browser_render_sees_js_inserted_text_that_net_fetch_misses`
- Transient live render:
  - `terrane-cap-browser tests/capability.rs::browser_peek_is_transient_and_render_is_rate_limited_for_resources`
  - `terrane-core tests/cap/browser.rs::browser_peek_resource_returns_body_but_records_nothing`
- Input validation and typed errors:
  - `terrane-cap-browser tests/capability.rs::browser_rejects_invalid_inputs`
  - `rust/crates/terrane-host/tests/cap/browser.rs::browser_render_blocks_cloud_metadata_url`
- Security and grants:
  - `terrane-cap-browser tests/capability.rs::browser_doc_covers_replay_security_and_limits`
  - `terrane-host tests/public_authz.rs::browser_render_is_grant_gated_for_public_callers`
  - `terrane-core tests/cap/interface.rs::default_registry_exposes_registered_grant_resource_namespaces`
- Blob offload smoke:
  - `rust/crates/terrane-host/tests/cap/browser.rs::browser_screenshot_offloads_to_blob` (`#[ignore]`, requires a working system browser)

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
