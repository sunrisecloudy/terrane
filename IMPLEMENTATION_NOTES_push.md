# Push Capability Implementation Notes

## Files Changed

- `rust/crates/terrane-cap-push/`: new deterministic `push` capability crate with command validation, events, fold state, docs, resource methods, and pure template rendering.
- `rust/crates/terrane-core/src/lib.rs`: added `PushState`, registered `PushCapability`, and allowed pure resource calls to return values without requiring an effect runner.
- `rust/crates/terrane-host/src/push_watch.rs`: new host-edge watcher that matches newly committed records, queues `native.notification.show`, records `push.delivered` / `push.failed`, and dedups by local log sequence.
- `rust/crates/terrane-host/src/lib.rs`: routes normal host dispatches through the push watcher after commit.
- `rust/crates/terrane-host/src/sync.rs`: includes `push.subscribed` / `push.unsubscribed` in local sync batches and applies sync batches through the host dispatch path so ingested events can trigger local push delivery.
- `rust/crates/terrane-cap-sync/src/lib.rs`: allowlists and validates synced push subscription facts; delivery outcomes are not allowlisted.
- `rust/crates/terrane-host/src/cli.rs`: adds `terrane push ls|rm`.
- `rust/crates/terrane-host/src/public_authz.rs`: grant-gates `push.subscribe` / `push.unsubscribe` and refuses `push.record-delivery` for public callers.
- `docs/APP_API.md`: adds the generated-style `ctx.resource.push` table.
- `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-host/Cargo.toml`: workspace and dependency wiring.
- Test wiring and coverage:
  - `rust/crates/terrane-cap-push/tests/capability.rs`
  - `rust/crates/terrane-core/tests/cap/push.rs`
  - `rust/crates/terrane-host/tests/cap/push.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
  - inventory updates in `rust/crates/terrane-core/tests/cap/interface.rs` and `rust/crates/terrane-host/tests/public_authz.rs`

## Key Design Choices

- Push v1 is local push only. `push.subscribe` and `push.unsubscribe` record synced facts; notification attempts happen only in `terrane-host` after new local or synced commits.
- Delivery outcomes are local bookkeeping. `push.delivered` and `push.failed` are never sync-allowlisted, so each running replica can deliver once for itself.
- The edge queues `native.notification.show` and records `push.delivered` when that queue request is accepted. Real OS display remains the native connector's drain responsibility.
- Subscription ids are deterministic when omitted, derived from `app`, `event_pattern`, and `template`.
- Templates are pure string rendering. They support `{kind}`, `{describe}`, and currently decoded `kv.*` payload fields such as `{key}` and `{value}`; title/body split on the first `|`.
- Runtime `ctx.resource.push.subscribe()` is a pure call-style resource method that returns `subId`; core now permits pure `Decision::Commit` resource calls without an effect runner, while effectful resource calls still require one.
- The watcher dedups by actual local log sequence, resolved from the committed `EventRecord`, so reprocessing the same record does not enqueue another native notification.

## Deviations / Limits

- Full startup catch-up scanning is not implemented as a background service. The watcher handles post-commit and sync-ingest delivery on running hosts; the 24h staleness cutoff is exposed as host policy constant for the future catch-up loop.
- Rate coalescing is implemented only for multiple matches in one committed batch. A persistent 10s per-subscription rate window needs a host runtime loop/state surface and should converge with the later automation integration.
- Web Notifications shell delivery is not separately wired; v1 uses the existing native notification queue. Web can consume the same queued native operation path when its connector supports it.

## Shared Files Touched

- `Cargo.toml`
- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-cap-sync/src/lib.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/src/sync.rs`
- `rust/crates/terrane-host/Cargo.toml`

## Proof Tests

- Crate pure behavior:
  - `validates_patterns_and_templates`
  - `pattern_matching_and_rendering_are_pure`
- Core capability behavior:
  - `push::push_subscribe_unsubscribe_and_replay_identity`
  - `push::push_limits_and_typed_errors_are_enforced`
  - `push::push_runtime_resource_records_subscription_and_lists_it`
  - `push::app_removal_drops_push_state`
- Host edge behavior:
  - `push::push_delivery_queues_native_notification_and_records_outcome`
  - `push::push_delivery_is_deduped_for_same_record`
  - `push::synced_push_subscription_delivers_on_target_home`
  - `push::push_watcher_exposes_staleness_cutoff_constant`
- Inventory / docs / auth:
  - `host::app_api_doc_resource_section_is_generated`
  - `interface::all_capability_docs_are_explicit_and_operational`
  - `public_command_inventory_covers_every_registered_command`
  - `grantable_command_inventory_requires_explicit_extractors_or_refusal`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
