# Sync v2 Implementation Notes

## Files changed

- Added `rust/crates/terrane-cap-sync/` with `sync.pair`, `sync.unpair`, `sync.apply`, folded peer roster/cursors, docs, and Borsh batch helpers.
- Registered sync in `rust/crates/terrane-core/src/lib.rs`, `rust/crates/terrane-core/Cargo.toml`, root `Cargo.toml`, and `Cargo.lock`.
- Extended `rust/crates/terrane-host/src/sync.rs` with HTTP-oriented helpers for pairing, CRDT vv/delta exchange, sparse `kv.*` event batches, cursor lookup, blob refs, and blob byte copying.
- Added web routes in `host/web/src/routes.rs` for `/sync/pair`, `/sync/<app>/vv`, `/sync/<app>/delta`, `/sync/<app>/events`, `/sync/<app>/apply-events`, `/sync/<app>/cursor/<peer>`, `/sync/<app>/blobs`, `/sync/<app>/blob/<hash>`, and `/sync/<app>/wait`.
- Updated CLI parsing/help for `terrane pair <url> --code <code>` and `terrane sync <app> --peer <url> [--watch]`.
- Updated public authz inventory so `sync.*` commands are explicitly host-edge-only.
- Added tests in `rust/crates/terrane-core/tests/cap/sync.rs` and `rust/crates/terrane-host/tests/cap/sync.rs`.

## Key design choices

- `sync.apply` records `sync.applied` followed by accepted foreign `kv.*` events in arrival order; replay rebuilds cursors and folded `kv` state without network access.
- Origin sequences are sparse origin-log positions, not contiguous filtered-event positions. The cursor accepts strictly increasing `origin_seq` values above the current cursor.
- The core sync capability validates paired peer, existing app, batch byte/event caps, allowlisted event kind, monotonic cursor, and `kv` payload app scope.
- CRDT sync remains `crdt.update`/`crdt.merge`; the new HTTP helpers only move the existing vv/delta exchange to the web-host transport.
- Blob metadata sync uses host-side `blob.link` from exported refs, then copies missing CAS bytes by hash.
- Incoming event actors are not trusted from the wire. The transport envelopes carry origin peer/seq metadata; `Core::commit` stamps the local accepting principal on committed records.

## Deviations / remaining follow-up

- Full Bonjour/mDNS discovery and persisted `$TERRANE_HOME/sync-tokens.json` bearer-token storage are not implemented in this slice.
- `terrane pair <url> --code <code>` records durable pairing facts over HTTP, but the code is currently compatibility surface only; it does not enforce one-time 6-digit code TTL/attempt burn semantics yet.
- `/sync/<app>/wait` is present as a 204 polling endpoint, but it does not yet park up to 30 seconds or wake on log changes.
- Raw TCP `terrane serve` / non-HTTP `--peer` remains as the existing compatibility path.

## Shared files touched

- `Cargo.toml`, `Cargo.lock`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `host/web/Cargo.toml`
- `host/web/src/routes.rs`

## Test proof

- `sync::pair_unpair_and_cursor_queries_are_replayable`
- `sync::apply_records_foreign_kv_after_sync_applied_and_replays`
- `sync::apply_validates_monotonic_cursor_allowlist_and_app_scope`
- `sync::sync_v2_two_homes_converge_kv_crdt_and_blob_refs`
- `interface::all_capability_docs_are_explicit_and_operational`
- `public_command_inventory_covers_every_registered_command`
- `public_query_inventory_covers_every_registered_query`

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
