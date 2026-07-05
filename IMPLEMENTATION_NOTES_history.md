# History Capability Implementation Notes

## Files Changed

- New capability crate: `rust/crates/terrane-cap-history/`
  - `src/lib.rs`: history projection, queries/resources, `history.revert`, `history.reverted`.
  - `src/doc.rs`: operational capability docs and resource docs.
  - `tests/capability.rs`: integration tests over the capability trait surface.
- Core wiring:
  - `rust/crates/terrane-core/src/lib.rs`: added `HistoryState`, `StateStore` accessors, and `default_registry()` registration.
  - `rust/crates/terrane-core/Cargo.toml`, root `Cargo.toml`, `Cargo.lock`: workspace/dependency wiring.
  - `rust/crates/terrane-core/tests/cap/history.rs`, `tests/cap/main.rs`, `tests/cap/interface.rs`: registered-capability and inventory coverage.
- Host wiring:
  - `rust/crates/terrane-host/src/cli.rs`: `terrane history ...` and `terrane revert ...` with dry-run default and `--yes` apply.
  - `rust/crates/terrane-host/src/public_authz.rs`: `history.revert` is grant-gated on the target app's `history` grant.
  - `rust/crates/terrane-host/tests/cap/history.rs`, `tests/cap/main.rs`, `tests/public_authz.rs`: CLI e2e and auth inventory updates.
- Docs:
  - `docs/APP_API.md`: generated resource surface updated for `ctx.resource.history`.

## Key Design Choices

- The log is never rewritten. `history.revert` emits ordinary compensating `kv.set` / `kv.deleted` events, followed by a `history.reverted` marker.
- The v1 projection is KV-scoped, matching the plan. `HistoryState` is rebuilt by broadcast `fold` and keeps:
  - a folded sequence cursor,
  - a per-app/per-key change index,
  - current KV values for pure diff computation,
  - app-scoped timeline summaries with `record.actor`.
- `history.list` supports `kind:`, `key-prefix:`, and `actor:` filters. Revert also accepts an optional actor filter and uses `record.actor` from folded changes.
- Replay identity holds because the projection is entirely rebuilt from folded records and revert records are normal events.
- Compaction horizon is represented honestly as `from_seq` in `history.list`; current v1 has no compacted archive source yet, so the folded projection reports `from_seq: 1`.
- Public untrusted `history.revert` is grant-gated; public raw history queries remain refused because the current public query authorizer has no app-argument grant check. App backends can read their own granted `ctx.resource.history.*` surface.

## Plan Deviations

- Shell History panel was not implemented in this slice. The plan lists it as a later host phase; this implementation lands the deterministic capability, CLI, docs, and tests first.
- The index is held in `HistoryState` rather than persisted into reserved KV keys. It is still a rebuildable projection from the log and avoids coupling history writes into KV while the core has no compaction/archive storage surface yet.

## Shared Files Touched

- `Cargo.toml`
- `Cargo.lock`
- `docs/APP_API.md`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/interface.rs`
- `rust/crates/terrane-core/tests/cap/main.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`

## Test Proofs

- Capability happy paths and actor filtering:
  - `terrane-cap-history::list_key_and_at_cover_kv_history`
  - `terrane-cap-history::actor_filter_uses_record_actor`
- Revert compensations and replay identity:
  - `terrane-cap-history::revert_emits_compensating_events_and_replays_identically`
  - `terrane-cap-history::validation_rejects_bad_scope_and_future_seq`
- Core registration and resource/doc inventory:
  - `terrane-core cap::history::history_is_registered_in_default_core`
  - `terrane-core cap::interface::all_capability_docs_are_explicit_and_operational`
  - `terrane-core cap::host::app_api_doc_resource_section_is_generated`
- Host e2e:
  - `terrane-host cap::history::history_cli_dry_run_and_apply_revert`
- Public auth policy:
  - `terrane-host public_authz::public_command_inventory_covers_every_registered_command`
  - `terrane-host public_authz::public_query_inventory_covers_every_registered_query`
  - `terrane-host public_authz::grantable_command_inventory_requires_explicit_extractors_or_refusal`

## Validation Commands

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
