# Capability: `share` — invite another user/replica into an app

New crate `rust/crates/terrane-cap-share/`, namespace `share`, registered in
`default_registry`. Builds directly on **sync v2** (`cap-sync-v2.md`) and the
existing `auth` capability: sync v2 makes two paired replicas exchange an app's
data; `share` decides *who is allowed to exchange what, in which direction*.
Without it, pairing is all-or-nothing; with it, a grant names an app and a
right, and the sync routes enforce it.

## Locked decision

**Enforcement is edge policy over folded state; the core stays policy-free.**
`share` records grants as ordinary events (folded into `ShareState`, mirrored
into `auth` via the existing `auth.grant`/`auth.revoke` commands so the admin
console and permission tooling see one truth). The `/sync/*` routes in the web
host read that folded state per request and refuse what isn't granted. The
core never says no — it just remembers who was told yes. This is the same
split `cap-blob.md` uses for bytes and the one rule requires.

## Rights model

Two rights, defined by what the **sync route** does for the grantee's replica:

| Right | Outbound (`GET /sync/<app>/…`) | Inbound (`POST` delta/events) |
| --- | --- | --- |
| `read` | served | **refused** (typed error names the missing right) |
| `write` | served | accepted |

`write` implies `read`. Grantee is either a paired replica
(`replica:<peer_hex>`, from `cap-sync-v2.md` pairing) or a member subject the
auth cap already knows — a member grant covers every replica that member
pairs later.

## Capability surface

### Commands

| Command | Args | Decision |
| --- | --- | --- |
| `share.invite` | `app, rights(read\|write), note?` | Validate app exists + rights; return `Decision::Effect(Effect::NewInviteToken)` — the edge mints a random token (same pattern as `Effect::NewReplicaId`), the event records only its **hash**. |
| `share.redeem` | `app, token_hash, grantee` | Validate an open invite with that hash; emit `share.redeemed`; the host follows up with `auth.grant` for the grantee (composed at the edge, two recorded dispatches). |
| `share.revoke` | `app, grantee` | Emit `share.revoked`; host follows with `auth.revoke`. |

### Events

| Kind | Payload | Fold |
| --- | --- | --- |
| `share.invited` | `{app, rights, token_hash, note}` | add open invite |
| `share.redeemed` | `{app, token_hash, grantee, rights}` | close invite; upsert `app → grantee → rights` |
| `share.revoked` | `{app, grantee}` | drop the share entry |
| (reacts) `app.removed` | — | drop the app's invites + shares |

Queries: `share.list <app>`, `share.invites <app>`. No app-facing resource in
v1 — sharing is an owner action through CLI/admin console, not something an
app backend triggers.

## Invite flow (out-of-band token)

1. Owner: `terrane share invite <app> --rights write` → prints the one-time
   token as text + QR (token exists only in that terminal and the peer's hand;
   the log holds the hash).
2. Peer, during or after pairing (`cap-sync-v2.md`): `terrane share redeem
   <peer-url> <token>` → POST `/sync/redeem {token, peer_hex}`. The serving
   host hashes the token, dispatches `share.redeem` + `auth.grant`, and from
   that moment the sync routes answer for that app.
3. Tokens are single-use, TTL 7 days, ≤ 5 failed redemptions burns the invite.

## Revocation semantics — honest version

`share.revoke` stops **future** sync: the next request from that grantee's
replica gets a typed refusal, long-polls are dropped, and this host stops
serving or accepting the app's data. It does **not** and cannot claw back
data already synced — the grantee's home has those events in its own log and
keeps them forever. Revocation is "you stop seeing new changes," never "you
forget." The doc says this in exactly those words so nobody ships a false
promise in UI copy.

Writes already accepted before revocation remain folded here too — history is
append-only; we do not rewrite logs to un-happen a formerly-authorized write.

## Security notes

- The token never appears in an event, `describe()`, or MCP dumps — hash only.
- `describe()` for `share.redeemed` prints app + grantee + rights, not hashes.
- Pairing without any share = no app data flows at all (sync v2 routes check
  `ShareState` for every app named in a request).
- Rights checks bind to the bearer-token→peer mapping from sync v2 pairing;
  a stolen bearer token is bounded by that replica's grants.

## Implementation plan

1. **Interface:** add `Effect::NewInviteToken` to
   `terrane-cap-interface::abi`; edge runner arm fills it with OS entropy and
   returns the `share.invited` event (constructor `invited_event()`), handing
   the plaintext token back through the command result only.
2. **Crate `terrane-cap-share`:** state, decide/fold/describe per the tables,
   `doc.rs` (rights table + the revocation paragraph); register in
   `default_registry`.
3. **Edge composition** in `terrane-host`: redeem/revoke helpers that dispatch
   `share.*` then the matching `auth.grant`/`auth.revoke`; CLI
   `terrane share invite|redeem|revoke|ls` (+ QR print).
4. **Enforcement** in the web host `/sync/*` routes: resolve bearer → peer →
   grantee subjects, consult folded `ShareState`, apply the rights table;
   typed refusals name the app and missing right.
5. **Admin console:** shares panel (list, revoke) reusing the existing
   permission-console patterns.
6. **Tests:** engine tests `terrane-core/tests/cap/share.rs` (invite/redeem/
   revoke folds, single-use + hash-only invariants, replay identity,
   app.removed); e2e `terrane-host/tests/cap/share.rs` — two temp homes over
   loopback HTTP: no-share ⇒ refused, read ⇒ pull-only (push refused),
   write ⇒ both, revoke mid-watch ⇒ next poll refused (default-run).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Per-key/per-container scoping inside an app (rights are app-granular),
re-sharing/delegation by grantees, expiring shares (only invites expire),
remote wipe of already-synced data, group invites.

## Decisions to confirm

- **Grant subject granularity** — recommendation: allow both `replica:<hex>`
  and member subjects, prefer member when the peer authenticates as a known
  member. Alternative: replica-only in v1 (simpler, but re-inviting every new
  device of the same person is tedious).
- **auth mirroring vs. share-only state** — recommendation: mirror into
  `auth.grant` so one permission surface exists. Alternative: keep shares
  solely in `ShareState` and teach the admin console a second store (two
  sources of truth — rejected unless auth's namespace-v1 shape can't express
  the sync grant cleanly).
- **Invite hash algorithm** — recommendation: SHA-256 lowercase hex, matching
  `cap-blob.md`. Alternative: none worth a second algorithm.
