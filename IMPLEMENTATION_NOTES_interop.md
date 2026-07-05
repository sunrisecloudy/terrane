# Interop + Item Implementation Notes

## Summary

Implemented the `interop` capability and the `item` primitive slice as a host-mediated recorded app-to-app call path. Added mandatory common API validation for bundles and rolled the common interface through repo apps, MCP/scaffold fixtures, app import, app install, and validation paths.

## Key Design Choices

- `ctx.resource.interop.call(target, verb, ...args)` is host-mediated through the existing edge runner and nested JS runtime execution. QuickJS does not open an MCP client or network transport.
- `Effect::AppCall { chain, target, verb, args }` records replies as `interop.called`, with inline replies up to 256 KiB and blob CAS offload up to 8 MiB.
- The target app runs under its own manifest resource grants while seeing the caller as `ExecutionPrincipal::app_caller(...)`.
- `terrane://app/<appId>/item/<itemId>` formatting/parsing lives in `terrane-cap-interface`; item ids are percent-encoded.
- Manifest `interfaces` are normalized to include mandatory `inbox` and `items`. `common.receive`, `common.list`, and `common.get` are validated on install/import/build validation.
- Action-table JS backends get default common verbs over `items/` and `inbox/`; custom `handle(input)` backends must explicitly declare and implement the common verbs.
- `interop.apps` lists apps declaring an interface. Public auth explicitly allows that discovery query.
- `interop.pick` records an `auth.granted` hook through the auth event path. Visual picker UI remains a documented follow-up, as allowed by the slice.

## Known Follow-Up

- The web/mac visual picker shell is not implemented in this slice. The backend grant hook/query path exists, and raw public `interop.pick` dispatch is refused so picker approval can stay on the recorded approval path.
- `interop.send` currently validates the route surface but does not auto-select a default target without a recorded picker selection UI.

## Files Changed

- Interface: `rust/crates/terrane-cap-interface/src/abi.rs`, `src/lib.rs`, `tests/integration.rs`.
- App manifest/catalog: `rust/crates/terrane-cap-app/src/lib.rs`, tests.
- Auth helper: `rust/crates/terrane-cap-auth/src/lib.rs`.
- New capability: `rust/crates/terrane-cap-interop/`.
- Runtime/edge/core: `rust/crates/terrane-core`, `rust/crates/terrane-host/src/edge.rs`, `src/lib.rs`, `src/preview.rs`, `src/public_authz.rs`.
- JS runtime/scaffold: `rust/crates/terrane-cap-js-runtime`, `rust/crates/terrane-host/src/scaffold/js_kv_app/main.js`, MCP scaffold templates.
- Repo app rollout: `apps/*/manifest.json`, `apps/todo/main.js`, `apps/todo-cli/main.js`.
- Docs/tests: `docs/APP_API.md`, `rust/crates/terrane-core/tests/cap/interop.rs`, `rust/crates/terrane-host/tests/cap/interop.rs`, MCP/public-auth fixtures.
- Shared wiring: root `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-host/Cargo.toml`.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`

## Test Coverage Pointers

- Grant enforcement: `interop::interop_call_requires_grant`.
- Cycle/depth typed errors: `interop::interop_rejects_cycles_and_depth_before_effect`.
- Item URI round-trip: `item_uri_round_trips_percent_encoded_item_ids`.
- Real two-app interop and item resolution: `interop::two_apps_call_each_other_and_resolve_item_uri`.
- Missing common API rejection: `interop::bundle_validation_rejects_missing_common_api`.
- Mandatory common rollout also covered by workspace app/MCP install and build validation tests.
