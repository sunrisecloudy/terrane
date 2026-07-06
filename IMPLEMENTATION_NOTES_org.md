# Implementation Notes: org

## Files changed

- `Cargo.toml`, `Cargo.lock`: added `terrane-cap-org` to the root workspace and dependency table.
- `rust/crates/terrane-cap-org/`: new deterministic org capability crate with command/query/event docs and integration tests.
- `rust/crates/terrane-cap-interface/src/abi.rs`: added `Effect::OrgKeygen` and `Effect::OrgRoleSign`.
- `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`: added `OrgState` to `State`/`StateStore` and registered `OrgCapability` in `default_registry`.
- `rust/crates/terrane-core/tests/cap/main.rs`, `rust/crates/terrane-core/tests/cap/org.rs`: added core org capability coverage.
- `rust/crates/terrane-host/Cargo.toml`, `rust/crates/terrane-host/src/edge.rs`, `rust/crates/terrane-host/src/cli.rs`: wired org edge effects and trusted CLI surface.
- `rust/crates/terrane-host/src/public_authz.rs`, `rust/crates/terrane-host/tests/public_authz.rs`: classified org membership commands as trusted-host-only and updated public inventory counts/assertions.
- `rust/crates/terrane-host/tests/cap/main.rs`, `rust/crates/terrane-host/tests/cap/org.rs`: added host e2e org coverage over the real edge secret store.
- `docs/APP_API.md`: documented org as a shared home with no v1 `ctx.resource` methods.

## Key design choices

- Org is modeled as a shared home fact set: `org.created`, `org.invited`, `org.invite.redeemed`, `org.member.granted`, and `org.member.left`.
- Private org and person keys stay at the host edge. The capability records public keys, token hashes, and person-signed role grants only.
- `org.member.granted` replay verifies the ed25519 signature against folded `person` state, preserving replay identity without keychain access.
- Org context reuses `ExecutionPrincipal { org, subject }`; no capability code writes actor/provenance.
- The CLI is only a thin adapter over dispatch/query. `terrane org invite` mints a token at the edge, records only its SHA-256 hash, and prints the token once.
- Public app-callable authz refuses org membership mutation commands; org queries are public-read classified with the existing inventory test.

## Deviations

- The plan's full two-person loopback sync role-enforcement scenario is not implemented in this slice. This v1 records and folds membership facts plus real edge signing; sync-route enforcement remains an edge policy follow-up over the folded org state, consistent with the plan's phased implementation and the share-invite stance.
- Premium provisioning is intentionally absent from Rust. The docs call out that Premium hosting is only a convenience over the same self-hosted org home.

## Shared files touched

- Workspace: `Cargo.toml`, `Cargo.lock`
- Core registry/state: `rust/crates/terrane-core/Cargo.toml`, `rust/crates/terrane-core/src/lib.rs`
- Shared ABI: `rust/crates/terrane-cap-interface/src/abi.rs`
- Host adapter/authz: `rust/crates/terrane-host/Cargo.toml`, `rust/crates/terrane-host/src/edge.rs`, `rust/crates/terrane-host/src/cli.rs`, `rust/crates/terrane-host/src/public_authz.rs`
- Public docs/tests: `docs/APP_API.md`, `rust/crates/terrane-core/tests/cap/main.rs`, `rust/crates/terrane-host/tests/cap/main.rs`, `rust/crates/terrane-host/tests/public_authz.rs`

## Tests proving properties

- Capability surface/docs/validation:
  - `terrane-cap-org::manifest_lists_org_commands_events_queries`
  - `terrane-cap-org::validation_helpers_reject_invalid_inputs`
  - `terrane-cap-org::doc_has_commands_events_and_internal_notes`
- Person-signed grants and tamper rejection:
  - `terrane-cap-org::role_grant_message_is_stable`
  - `terrane-cap-org::signed_role_grant_round_trips_and_rejects_tampered_role_or_member`
  - `terrane-cap-org::fold_member_granted_requires_known_signer_person`
  - `terrane-cap-org::fold_rejects_tampered_signer_in_member_granted`
  - `terrane-core cap::org_member_granted_with_tampered_signature_is_rejected_by_fold`
- Replay identity and command/query happy path:
  - `terrane-core cap::org_create_invite_join_role_set_leave_replays_from_public_events`
  - `terrane-host cap::org_create_invite_join_role_set_leave_replays_through_real_edge`
  - `terrane-host cap::org_create_is_idempotent_when_called_twice_for_the_same_founder`
- Public authz inventory:
  - `terrane-host public_authz::public_command_inventory_covers_every_registered_command`
  - `terrane-host public_authz::public_query_inventory_covers_every_registered_query`

## Gate

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
