---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/sync-provenance/*.json, forge/fixtures/sync-provenance/manifest.json, spec note appended to forge/spec/sync-rbac.md
---

# T029 — Sync provenance / transitive-trust validation vectors (SS-7, review 092 #1)

Codex review 092 #1 (P2) found a real SS-7 actor-identity bypass in the wired
sync apply path: once peer A imports a content-addressed chunk authored by peer C,
A can later re-export that same chunk to B. B currently authorizes it against
`peer:A` (the relay), not C (the original author). So if B trusts A as Owner but
does NOT trust C, C's write still lands at B via A. SS-7
(`prd-merged/03-sync-server-prd.md:21`) requires authorization against the
ORIGINAL actor identity.

We will fix the Rust apply path to preserve original author/source in the op
envelope and authorize re-exported chunks against the ORIGINAL actor (or fail
closed on a `record.remote_import` whose provenance is unknown). I need a vector
suite that locks the semantics so the fix and future transport/server sync code
cannot regress.

## Deliverables

1. A short spec section appended to `forge/spec/sync-rbac.md` titled
   "Provenance and forwarded chunks": define that every record/schema op carries
   an `author_actor_id` (the ORIGINAL writer) distinct from the `session_peer_id`
   (the peer that handed us the chunk this hop); authorization uses
   `author_actor_id` resolved against the receiver's trusted membership; a
   forwarded chunk whose original author is untrusted is rejected even if the
   relaying peer is fully trusted; and a chunk whose original provenance cannot be
   established fails closed.

2. `forge/fixtures/sync-provenance/<case>.json` + manifest. Each case models a
   multi-hop topology and the receiver's expected decision. Suggested shape:
   ```json
   { "case": "relayed_untrusted_author_rejected",
     "topology": "C writes -> A imports -> A re-exports to B",
     "receiver": "B",
     "trusted_membership": {
       "actor-A": { "role": "owner", "db_write": ["*"], "schema_write": true }
     },
     "incoming": {
       "session_peer_id": "peer:A",
       "author_actor_id": "actor-C",
       "metadata": { "resource_type": "record", "op": "insert", "collection": "tasks", "record_id": "t9" }
     },
     "expect": { "decision": "permission_denied",
                 "reason_contains": "original author actor-C is not a trusted member" } }
   ```

## Coverage (~10)

- C writes, A imports, B trusts A but NOT C, A->B forwards C's chunk -> rejected
  (the headline review-092 case).
- Same topology but B ALSO trusts C with db.write on the collection -> applied.
- A direct write by trusted A -> applied (author == session peer).
- A forwarded chunk whose `author_actor_id` is missing/unknown -> fail closed
  (rejected).
- A relay peer A that is itself untrusted forwarding a trusted author C -> decide
  per spec (the original author is trusted, but the session peer is not
  authenticated): pick fail-closed and document why.
- An author trusted as Viewer (read-only) whose write is relayed by an Owner relay
  -> rejected (author role gates the write, not the relay role).
- A self-escalation attempt where the incoming claim asserts a higher author role
  than the receiver's trusted membership row for that author -> rejected.
- A two-hop chain C -> A -> B where C is trusted as Editor with db.write on the
  exact collection -> applied; same but C only has db.write on a DIFFERENT
  collection -> rejected.
- A schema-change op authored by C (Maintainer at B) relayed by A -> applied;
  authored by C (Editor at B) -> rejected.

In `## Result`, state explicitly that the receiver resolves trust by
`author_actor_id` against its OWN membership table, and that the relay/session
peer identity never widens authorization (same trust boundary as review 048/050).

## Result

(codex fills this in)
