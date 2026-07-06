# Person Owner Rebind Implementation Notes

## Files changed

- `rust/crates/terrane-core/src/lib.rs`
  - Added state-aware `local_owner_subject` / `local_owner_principal` helpers.
  - Rebound default local-owner dispatch principals after a person exists.
  - Made `RuntimeResourceHost::new` derive its default principal from folded state.
- `rust/crates/terrane-cap-auth/src/lib.rs`
  - `auth.member.ensure-local-owner` now accepts an optional owner subject, defaulting to `user:local-owner`.
  - Added `member_exists`.
  - Auth grant lookup for person-based local users falls back to legacy `user:local-owner` grants so old homes keep working without log rewrites.
- `rust/crates/terrane-cap-auth/src/doc.rs`
  - Documented the optional subject parameter for `auth.member.ensure-local-owner`.
- `rust/crates/terrane-host/src/lib.rs`
  - First-run identity now ensures the owner member for `user:<person_id>` once the person exists.
- `rust/crates/terrane-host/src/{permission.rs,public_authz.rs,secret_store.rs,edge.rs,ffi.rs,preview.rs}`
  - Replaced direct bootstrap-principal auth checks with the state-aware local-owner principal.
- `host/web/src/admin.rs`
  - Default admin grant/revoke and agent-owner subjects now use the canonical local owner subject.
- Tests updated:
  - `rust/crates/terrane-core/tests/cap/person.rs`
  - `rust/crates/terrane-host/tests/cap/person.rs`
  - `rust/crates/terrane-host/tests/{abi.rs,broker_missing_set.rs,permission.rs}`
  - `host/web/tests/web.rs`

## Key design choices

- The bootstrap constant `LOCAL_OWNER_SUBJECT` remains unchanged and remains the default when no person exists.
- Rebinding is forward-only: existing log records keep their recorded `actor`, including `user:local-owner`.
- New commands submitted with the default local-owner principal are stamped as `user:<person_id>` after `person.created` is folded.
- Capabilities still never set `actor`; core commit stamping remains the single provenance boundary.
- Legacy grants to `user:local-owner` still authorize a rebound `user:<person_id>` local owner. This preserves existing homes where grants were recorded before the person rebind.
- New host/admin defaults create memberships and grants for `user:<person_id>` once the person exists.

## Deviations

- No deviation from the deferred follow-up intent. The only compatibility addition is legacy grant lookup fallback for person-based local owners, which avoids breaking existing homes without rewriting old auth events.

## Shared files touched

- `rust/crates/terrane-core/src/lib.rs`
- `rust/crates/terrane-cap-auth/src/lib.rs`
- `rust/crates/terrane-cap-auth/src/doc.rs`
- `rust/crates/terrane-host/src/lib.rs`
- `rust/crates/terrane-host/src/{permission.rs,public_authz.rs,secret_store.rs,edge.rs,ffi.rs,preview.rs}`
- `host/web/src/admin.rs`

## Proof tests

- `person::local_owner_rebinds_to_person_for_new_events_without_rewriting_old_log`
  - Proves pre-person events stay `user:local-owner`, post-person events are stamped `user:<person_id>`, replay is unchanged, person-subject grants resolve, and legacy grants still resolve.
- `person::person_first_run_creates_replica_attested_identity_and_replays`
  - Proves host first-run mints person identity and a subsequent CLI-authored event uses `user:<person_id>`.
- `open_at_home_seeds_local_owner_membership_once`
  - Proves first-run host identity seeds one owner membership for the canonical local owner subject.
- `broker_reports_missing_grant_then_none_after_grant`, ABI tests, and `admin_can_grant_missing_app_resource`
  - Prove permission/admin/FFI grant paths resolve after the rebind.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`
