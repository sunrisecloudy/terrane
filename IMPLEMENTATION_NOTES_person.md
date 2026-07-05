# Person Primitive Implementation Notes

## Files changed

- New capability crate: `rust/crates/terrane-cap-person/`
  - `src/lib.rs`: commands, events, fold/query state, validation, ed25519 signature verification helpers.
  - `src/doc.rs`: explicit capability docs for CLI/MCP/contract surfaces.
  - `tests/capability.rs`: public-surface integration tests.
- Shared wiring:
  - `Cargo.toml`, `Cargo.lock`
  - `rust/crates/terrane-cap-interface/src/abi.rs`
  - `rust/crates/terrane-core/Cargo.toml`
  - `rust/crates/terrane-core/src/lib.rs`
  - `rust/crates/terrane-host/Cargo.toml`
  - `rust/crates/terrane-host/src/edge.rs`
  - `rust/crates/terrane-host/src/lib.rs`
  - `rust/crates/terrane-host/src/cli.rs`
  - `rust/crates/terrane-host/src/public_authz.rs`
  - `rust/crates/terrane-host/src/secret_store.rs`
- Tests:
  - `rust/crates/terrane-core/tests/cap/person.rs`
  - `rust/crates/terrane-core/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/cap/person.rs`
  - `rust/crates/terrane-host/tests/cap/main.rs`
  - `rust/crates/terrane-host/tests/public_authz.rs`

## Key design choices

- `person_id` is `sha256(pubkey)` hex-16 prefix; the full ed25519 public key is recorded in `person.created`.
- `person.create`, `person.attest`, and `person.rotate` return edge effects:
  - `Effect::PersonKeygen`
  - `Effect::PersonSign`
  - `Effect::PersonRotate`
- The host edge stores the ed25519 seed in the existing connection secret store as `person-<person_id>.ed25519`. The event log records only public key material, attestation claims, and signatures.
- Replay verification is pure: `person.attested` verifies against the folded current public key; `person.rotated` verifies against the current key or an active `device-key` attestation.
- First-run host identity now ensures:
  - replica id exists,
  - person exists,
  - local replica is attested to that person,
  - legacy local-owner auth membership still exists.
- Public `capability_command` refuses all `person.*` mutations. CLI/trusted-host can run them.
- `secret_store::set_secret` now verifies keychain readback before accepting a keychain write; otherwise it writes the encrypted fallback. This prevents a write-success/read-fail keychain state from losing the person seed.

## Deviations / follow-up

- The broader auth subject migration from `user:local-owner` to `user:<person_id>` is not completed in this slice. The first-run path records the durable person and replica attestation, while the existing auth subject remains for compatibility with the current auth gate and tests.
- Premium countersign/recovery endpoint is outside this repo/worktree and remains a follow-up.
- Publish/org/actor consumers are not rewired here; the primitive and host edge are ready for those follow-on slices.

## Validation

- `scripts/with-cargo-cache.sh cargo test --workspace --locked`
- `scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings`
- `scripts/with-cargo-cache.sh cargo run -p terrane-host --bin terrane -- help`

## Property tests

- Command happy paths: `create_attest_revoke_and_query_public_identity_only`, `person_create_attest_rotate_replays_from_public_events`, `person_first_run_creates_replica_attested_identity_and_replays`
- Replay identity: `person_create_attest_rotate_replays_from_public_events`, `person_first_run_creates_replica_attested_identity_and_replays`
- Input validation / typed errors: `validation_errors_are_typed`, `rotate_accepts_old_key_signature_and_rejects_bad_signature`
- Security / limit rules:
  - `create_attest_revoke_and_query_public_identity_only` checks private seed absence from folded state/query output.
  - `person_first_run_creates_replica_attested_identity_and_replays` checks log output does not expose secret-store filenames and a copied public log can recognize the same person without key material.
  - `public_command_inventory_covers_every_registered_command` pins `person.*` public-command refusal.
