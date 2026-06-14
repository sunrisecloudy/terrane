# Sync RBAC validation

This note defines the M0b authorization envelope for applying remote sync data.
It is a semantic contract for the fixtures in `forge/fixtures/sync-rbac/`; it is
not a wire-format replacement for `sync-protocol.md`.

Normative PRDs:

- `prd-merged/03-sync-server-prd.md` SS-7: every remote op is validated against
  actor identity, role, resource type, operation, capability grants, and schema
  compatibility before application. Rejections are logged and surfaced as
  `permission_denied`.
- `prd-merged/07-security-prd.md` SC-9 through SC-12: membership grants are
  persisted by the workspace, policy decisions happen in Rust for every command
  and remote sync op, default roles are explicit, and denials are auditable.

## Trust boundary

A peer sync session presents an actor identity plus role/capability claims, but
incoming messages are never authoritative for authorization. The receiving
workspace resolves trusted membership locally before applying a chunk:

- `actor_id`: authenticated peer actor for the session.
- `role`: trusted role from the receiving workspace membership table.
- `db_read` / `db_write`: trusted collection grants from that membership table.
- `schema_write`: trusted schema-maintenance grant.

Incoming message claims may narrow the trusted grants for diagnostics or future
proof-carrying sync, but they must not widen them. If a message claims a role or
grant that exceeds the receiving workspace's trusted membership row, the receiver
rejects the operation with `permission_denied` before importing the chunk.

This mirrors the `forge-core` command boundary: capability grants are trusted
only when they come from the receiving workspace grant table, not from request or
message payloads.

The actor a chunk is authorized against is its **original author**, not whichever
peer most recently relayed it. A chunk a peer merely forwarded (it imported the
chunk from another peer and re-exports it) carries its original author's source —
and the record ids it touched — in its remote-import provenance; the receiver
resolves trusted membership for that original author and gates a concrete record.
This provenance is preserved across every relay hop: each import stamps the
remote-import oplog row with the chunk's original author (not the importing peer)
and its touched record ids, so after any number of hops the chunk is still gated
against the original author and still names its record. A peer the receiver trusts
therefore cannot launder a write from an untrusted peer through itself — the
relayed chunk is gated against the untrusted author and denied (`review 092 #1`).

If a forwarded chunk's original provenance is **unrecoverable** (its remote-import
row names no usable original author), the receiver must **fail closed** and reject
it rather than attribute it to the relay: a relay must not be able to launder a
write whose author it cannot prove by having the receiver fall back to the relay's
own (trusted) identity.

Every remote import — whether a batch apply or a single-chunk import API — writes
its `record.remote_import` row through one shared engine that takes the original
author and touched record ids as inputs. There is no second import path that can
write a provenance-poor remote-import row (no original author, no record ids):
losing provenance at import would reintroduce exactly the relay-laundering this
boundary forbids (`review 095`).

The public single-chunk import API additionally **validates provenance at the
import boundary**, BEFORE any store mutation: the effective original author (the
forwarded author when set, else the importing source) must be a non-blank peer id,
and a record import must name a **non-empty, blank-free** list of touched record
ids — every id must be non-blank after trim (the **strict reject-on-blank**
contract). An empty list, a list of only blank ids, OR a list that mixes a blank id
with a valid one (`["", "t1"]`) is rejected outright rather than silently filtered,
so no blank id can be smuggled past the floor. A call that fails either check is
rejected and leaves the store completely unchanged — no chunk and no oplog row. The
ids that do pass are persisted in canonical (trimmed) form. This keeps a
caller-supplied import from writing a provenance-poor row whose author or record
identity is empty (or whose recovered id list names nothing), which the next relay
hop would recover as an envelope the authorizer must deny as missing a record id
(`review 096`, `review 097`). The internal batch path is fed only by the trusted
sync seam, whose generic transact-group / unknown-op chunks legitimately carry no
single record id and are gated by the receiver's authorization envelope instead;
that seam also trims and drops any blank entry when recovering touched record ids
from a forwarded chunk's oplog, so every relay hop reconstructs a clean list.

## M0b scope

M0b validates authorization at apply time in the receiving store. Full server
membership exchange, token issuance, revocation propagation, and cross-device
session negotiation are later milestones. The M0b receiver still must make a
deterministic local decision before any CRDT import mutates state.

## Apply-time decision order

For each incoming `chunk_response` or `live_update` carrying record or schema
changes, the receiver does the following:

1. Resolve the trusted membership entry for the authenticated session actor.
2. Compare incoming role/grant claims to the trusted entry. Any self-escalation
   is rejected.
3. Validate envelope metadata before CRDT import: document id, resource type,
   operation, collection, record ids or schema id, and schema version. A record
   write carries a **non-empty list** of touched record ids — one for a
   single-record op, several for a legitimate multi-record transact group — and the
   collection grant gates the op as a whole. The metadata gate rejects only a truly
   empty / all-blank record-id list (an unknown record identity), never a valid list
   of one OR many ids (`review 093`).
4. Check the default role matrix for the operation.
5. Check trusted collection grants. `*` means all collections; otherwise grants
   are exact collection names.
6. Check schema compatibility and schema-maintenance grants for schema-changing
   operations.
7. Only after all checks pass, import the chunk and rebuild the local projection.

A rejection must:

- skip CRDT import,
- leave local projections unchanged,
- emit a sync-level `permission_denied` response,
- write an audit denial containing actor id, operation, resource, collection or
  schema id, trusted role, trusted grants, and denial reason.

## Default role matrix

The default roles from `forge-domain` map to sync apply permissions as follows:

| Operation | Roles that may pass the role check | Additional trusted grant |
| --- | --- | --- |
| Record insert, patch, delete | Owner, Maintainer, Editor | `db.write` for the collection |
| Schema change | Owner, Maintainer | `schema_write = true` |
| Read-only catch-up without writes | Owner, Maintainer, Editor, Runner, Viewer, Auditor, Reviewer | `db.read` for the collection |

Runner is execution-oriented and does not imply remote record write permission in
M0b. Viewer, Auditor, and Reviewer are read or oversight roles and cannot author
remote writes even if an incoming message claims write grants. Future custom
roles may map to the same capability checks, but trusted receiver-side grants
remain authoritative.

## Fixture semantics

Each fixture describes one incoming remote operation and the receiver's expected
decision. `trusted_peer` is the receiving workspace's local membership row.
`incoming_claim` is untrusted message/session metadata. `incoming` is the
semantic envelope that must be inspected before chunk import. `expect.apply`
states whether the opaque chunk may be imported and whether projections may be
rebuilt.

The fixtures intentionally avoid encoding real Loro bytes. Their purpose is to
lock the authorization and audit contract so the later sync apply path can wire
the policy decision before the CRDT import boundary.
