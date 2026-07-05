# Primitive: `person` — durable identity as a local keypair

New crate `rust/crates/terrane-cap-person/`, namespace `person`. The durable
"you" that replicas, org memberships, agents, and publishing attach to. Today
the user is the string constant `user:local-owner`; this makes it a real,
portable, verifiable identity — without requiring any account.

## Locked decision (user, 2026-07-05)

**Local keypair + attestations.** An ed25519 keypair minted on first run IS
the person (`person_id` = hash of the public key). Everything else —
the Premium/Google account, each device's replica, email addresses — attaches
as a signed, revocable **attestation**. Cloud login enriches identity; it
never defines it. Works fully offline (DXOS-HALO model).

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `person.create` | mints keypair at the edge (`Effect::PersonKeygen`) — private key → OS keychain ([cap-oauth-connections.md](cap-oauth-connections.md) store), recorded `person.created {person_id, pubkey}` |
| Command | `person.attest` | `{person_id, kind, claim}` signed by the person key at the edge → `person.attested {person_id, kind, claim, sig}`; kinds: `replica` (claim = peer id), `premium-account` (claim = account id, countersigned by Premium), `email`, `device-key` (a second device's pubkey) |
| Command | `person.revoke-attestation` | recorded; folded state drops the claim |
| Command | `person.rotate` | new pubkey signed by the old key **or** by a `device-key`-attested key (multi-device recovery) → `person.rotated {old, new, sig}` |
| Query | `person.whoami` / `person.get` | folded person + attestations |

Fold keeps `person_id → {pubkey, attestations, rotated_to?}`. Person events
sync like any events; another home verifies a person purely from signatures —
the pubkey is the global identity, no registry.

## Integration (the point of the exercise)

- **Auth subjects become person-based:** `user:local-owner` →
  `user:<person_id>`. First-run migration mints the keypair, creates the
  person, attests the local replica, and rebinds the owner member.
- **[cap-share-invite.md](cap-share-invite.md) pairing** attaches to persons,
  not bare replicas: an invite is redeemed by a person; their other attested
  devices inherit the share.
- **[cap-publish.md](cap-publish.md) converges:** the publisher signing key
  IS the person key (that plan minted its own ed25519 — delete that, one
  identity).
- **[primitive-org.md](primitive-org.md)** memberships are grants to
  person_ids.
- **[primitive-actor.md](primitive-actor.md)** actor strings use person_ids.

## Security

Private key never leaves the keychain; signing is an edge effect. Key loss
with only one device and no recovery attestation = identity loss (stated
honestly; Premium recovery attestation is the mitigation offered at login).
Attestation verification is pure (pubkey + sig in events) — replay-safe.

## Implementation plan

1. `Effect::PersonKeygen` / `Effect::PersonSign` in the interface; keychain
   storage via the connections store.
2. Crate: commands/fold/doc; signature verification helpers (ed25519-dalek,
   workspace dep shared with publish).
3. First-run migration in the host: mint → create → attest replica → rebind
   auth owner subject.
4. Premium countersign endpoint (terrane-premium) for the
   `premium-account` attestation kind.
5. Rework `LOCAL_OWNER_SUBJECT` call sites; shell "identity" panel (pubkey,
   devices, attestations).
6. Tests: engine (create/attest/rotate/verify, replay), e2e (two homes
   recognize the same person from synced events; recovery via device-key).

Gate: `cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Social recovery quorums, DID/W3C interop, key transparency logs, multiple
personas per home (one person per home in v1).

## Decisions to confirm

- **Premium recovery attestation on by default at login** — recommend yes
  (the "don't lose yourself" backstop) — alternative: explicit opt-in.
- **person_id format** — recommend `sha256(pubkey)` hex-16 prefix with full
  key in events — alternative: full pubkey as id (longer, zero collision
  thinking).
