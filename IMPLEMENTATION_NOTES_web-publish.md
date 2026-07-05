# web-publish implementation notes

## Files changed

- Added `rust/crates/terrane-cap-web-publish/` with `src/lib.rs`, `src/doc.rs`, and integration tests in `tests/capability.rs`.
- Registered the capability in `Cargo.toml`, `Cargo.lock`, `rust/crates/terrane-core/Cargo.toml`, and `rust/crates/terrane-core/src/lib.rs`.
- Added core and host e2e coverage in `rust/crates/terrane-core/tests/cap/web_publish.rs` and `rust/crates/terrane-host/tests/cap/web_publish.rs`, and included them from each `tests/cap/main.rs`.
- Added `publicVerbs` parsing/validation to `terrane-cap-js-runtime` and host bundle manifest structs.
- Added CLI routing/help for `terrane web-publish enable|disable|domain set|status` in `rust/crates/terrane-host/src/cli.rs`.
- Classified web-publish public authz in `rust/crates/terrane-host/src/public_authz.rs` and updated the public-authz inventory test.

## Design choices

- `web-publish` is a deterministic recorded-facts capability only. It records `web-publish.enabled`, `web-publish.disabled`, and `web-publish.domain.set`.
- The folded state stores one route per app: mode, slug, optional custom domain. Live tunnel health remains a transient host/Premium edge read, not replay state.
- `web-publish.enable` requires the app to exist and defaults to `static` mode. If the relay/host has not supplied a slug yet, the command uses a deterministic app-derived slug so replay remains stable.
- `web-publish.domain.set` requires the app to already be published.
- Relay credentials, tunnel tokens, visitor traffic, request logs, and anonymous invocation results are not recorded.
- `publicVerbs` is validated at manifest-load time with the plan limit of 16 entries and safe verb tokens.
- Public capability callers cannot mutate or query publish inventory: `web-publish.*` commands are trusted-host-only and `web-publish.status` is not public-query allowlisted.

## Deviations from the plan

- Relay service work in `../terrane-premium`, outbound WSS tunnel bridging, shell UI controls, and loopback fake-relay tunnel tests were not implemented in this worktree slice. This worktree is restricted to the Terrane repo checkout, so the Premium relay itself cannot be changed here.
- No inbound listener was added to the home host. The implemented host surface records owner intent only; relay/tunnel dialing remains an edge effect for the Premium host integration.

## Shared files touched

- `Cargo.toml`
- `Cargo.lock`
- `rust/crates/terrane-core/Cargo.toml`
- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-core/tests/cap/main.rs`
- `rust/crates/terrane-host/Cargo.toml`
- `rust/crates/terrane-host/src/cli.rs`
- `rust/crates/terrane-host/src/edge.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/public_authz.rs`
- `rust/crates/terrane-host/tests/cap/main.rs`
- `rust/crates/terrane-host/tests/public_authz.rs`
- `rust/crates/terrane-cap-js-runtime/Cargo.toml`
- `rust/crates/terrane-cap-js-runtime/src/bundle.rs`

## Proof tests

- Command happy paths: `web_publish::web_publish_enable_domain_status_and_replay_identity`, `web_publish::web_publish_cli_records_status_domain_and_disable`, `web_publish_records_enable_disable_and_domain_facts`.
- Replay identity: `web_publish::web_publish_enable_domain_status_and_replay_identity`, `web_publish::web_publish_validation_and_disable_are_replay_safe`, `web_publish_replay_identity_and_app_removal_hold`.
- Input validation and typed errors: `web_publish::web_publish_validation_and_disable_are_replay_safe`, `web_publish::web_publish_cli_rejects_bad_mode_before_recording`, `web_publish_decide_validates_app_mode_slug_and_domain`.
- Security/limit rules: `public_verbs_limit_and_safety_are_enforced`, `public_command_inventory_covers_every_registered_command`, `public_query_inventory_covers_every_registered_query`.
- Capability docs/registry: `interface::all_capability_docs_are_explicit_and_operational`, `interface::default_registry_manifest_is_valid`, `surface_is_derived_from_the_live_declarations`.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
