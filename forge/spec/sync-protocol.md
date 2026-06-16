# In-Process Sync Protocol Fixtures

Source of record: `prd-merged/03-sync-server-prd.md` SS-1, SS-2, SS-7,
`prd-merged/02-data-layer-prd.md` DL-3, DL-4, DL-6, DL-9, and the current
`forge-crdt` / `forge-storage` APIs.

This document pins the M0b in-process peer sync seam that
`forge/fixtures/sync/*.json` expects Rust tests to exercise next. It describes
the local CI model before WebSocket transport, account auth, relay, or
server-side RBAC are introduced.

## Scope

M0b in-process scope:

- Two workspace replicas run in one test process.
- Each replica has a distinct CRDT peer id. Reusing one peer id across
  concurrent writers is invalid for Loro-backed documents.
- Each records collection is one CRDT document with `doc_id =
  "collection/<name>"`.
- A peer advertises the immutable update chunks it holds per `doc_id`.
- The other peer sends the missing chunks for that `doc_id`.
- The receiver imports those chunks into its local `RecordsDoc`, persists the
  accepted updates, and rebuilds or refreshes the derived projection.
- The exchange runs in both directions for bidirectional sync cases. A
  one-directional catch-up case is allowed when the source already has all
  writes.

Deferred to M0b+ / later:

- WebSocket frame transport, TLS, resumability, and live subscriptions.
- Role validation for remote operations. SS-7 remains normative: a future server
  must validate actor identity, role, resource, operation, capability grants,
  and schema compatibility before applying a remote operation. These fixtures
  assume all operations are already authorized.
- Snapshot compaction, peer reset policy, presence, and control-channel
  membership flows.

## Current Code Surface

`forge_crdt::RecordsDoc` provides the CRDT primitive:

- `version()` captures the current encoded Loro version vector.
- `export_updates_since(version)` exports the operations appended after a saved
  version.
- `export_all_updates()` exports all operations.
- `import_updates(bytes)` imports one opaque update blob.
- `from_updates(peer_id, chunks)` rebuilds a document from update chunks.
- `materialized()` returns the semantic document value used to compare
  convergence.

`forge_storage` persists record CRDT updates as immutable `crdt_chunks` rows and
derives queryable `records` rows from those chunks:

- `collection_doc_id(collection)` maps a collection to `collection/<name>`.
- `CHUNK_FORMAT` is currently `loro`.
- `Store::rebuild_projection` drops and reconstructs the visible projection from
  `crdt_chunks`.

The current M0a storage helper mints local sequence chunk ids such as
`chunk-0001`. That is sufficient for one local writer, but not sufficient as a
network-visible sync frontier: two disconnected peers can both create a
different `collection/tasks` chunk named `chunk-0001`. The M0b sync runner must
therefore use globally unique exchanged chunk identities, for example
`<origin_peer>/<local_chunk_id>` or a content-addressed digest, while still
preserving each chunk as immutable Loro update bytes.

## Protocol Model

The fixture runner can model SS-2 with these in-process messages:

1. `hello`: each peer identifies its protocol version and stable CRDT peer id.
2. `capabilities`: both peers declare support for `records_doc_updates`,
   `loro_chunks`, and `projection_rebuild`.
3. `frontier_summary`: each peer sends, per `doc_id`, the exchanged chunk ids it
   already holds.
4. `chunk_request`: each peer asks the other for missing chunk ids.
5. `chunk_response`: each peer sends the missing immutable chunks.
6. `ack`: the receiver confirms chunks imported and projection rebuilt.

The in-process runner does not need to exercise every SS-2 frame on day one,
but the frame enum should reserve the PRD names now: `snapshot_offer`,
`snapshot_response`, `live_update`, `conflict_notice`, `permission_denied`, and
`resync_required`. That keeps the test seam aligned with the later WebSocket
transport without forcing networking into M0b.

The frontier for these fixtures is a set of immutable exchanged chunk ids per
`doc_id`, not a scalar latest revision. A set makes duplicate delivery and
out-of-order delivery testable, and matches the Loro property that importing an
already-seen update is idempotent.

Because a Loro update blob is opaque, SS-7 authorization cannot be recovered
after import. Any future `chunk_response` or `live_update` that carries remote
writes must include enough envelope metadata to authorize before application,
including at least `actor_id`, role/capability claims, `doc_id`, resource type,
operation kind, collection, touched record ids, schema version, and the opaque
chunk id/payload. Unauthorized chunks must be rejected as `permission_denied`
before `import_updates` or projection rebuild runs.

## Merge Semantics

The converged projection is the byte-stable semantic read surface after all
authorized updates have been exchanged and projections rebuilt.

- Independent collections merge.
- Different records in the same collection merge.
- Different scalar fields of the same record both survive.
- The same scalar field written concurrently resolves by Loro map LWW semantics.
  The exact winner is implementation-defined, but both peers must agree on the
  same winner after sync.
- Whole-record delete is represented by CRDT history and the record is absent
  from the normal visible projection after the delete wins or is the only
  operation touching that record.

Semantic conflict UI is out of scope for these fixtures. SS-10 still applies
later for higher-level ambiguities such as uniqueness, schema rename, or
delete-vs-edit policy conflicts.

## Fixture Shape

Each case is a semantic vector. It does not contain real Loro update bytes; the
Rust runner should generate those bytes by applying the operations to
`RecordsDoc` / storage, then exchange generated chunks according to `sync`.

```json
{
  "version": 1,
  "case": "same_collection_different_records_merge",
  "peer_a_id": 101,
  "peer_b_id": 202,
  "seed": [],
  "peer_a": [
    {"op": "insert", "collection": "tasks", "id": "t1", "fields": {"title": "a"}}
  ],
  "peer_b": [
    {"op": "insert", "collection": "tasks", "id": "t2", "fields": {"title": "b"}}
  ],
  "sync": {
    "directions": ["a_to_b", "b_to_a"],
    "frontier": "exchanged_chunk_id_set_per_doc",
    "duplicate_chunks": false,
    "reorder_chunks": false
  },
  "expect_converged": {
    "tasks": [
      {"id": "t1", "fields": {"title": "a"}},
      {"id": "t2", "fields": {"title": "b"}}
    ]
  },
  "assertions": ["peer_a_projection_equals_peer_b_projection"]
}
```

Operation forms:

- `insert`: visible record create or recreate.
- `patch`: merge supplied fields into an existing record and preserve omitted
  fields.
- `delete`: remove the whole visible record.

When `seed` is non-empty, fixtures use `"seed_mode": "shared_history"`: apply
the seed once, then clone or import that same CRDT history into both peers
before partitioned peer operations. Do not replay the seed independently on
each peer, because that creates two different CRDT histories/frontiers and
breaks cases such as `already_in_sync_noop`. The seed lets fixtures describe
concurrent patches or deletes against a shared baseline.

For implementation-defined LWW cases, `expect_converged` may use this marker:

```json
{"one_of": ["from-a", "from-b"], "agreement_required": true}
```

The test should assert that both peers agree and that the chosen value is one of
the listed values. It must not assert a specific winner unless the implementation
later documents a stable tie-break as public contract.

## Result

The fixture suite covers ten M0b convergence cases:

- Disjoint collections merge.
- Different records in the same collection merge.
- Concurrent patches to different fields of the same record both survive.
- Concurrent writes to the same scalar field converge with agreement only.
- A delete on one peer propagates while unrelated peer writes survive.
- One-directional catch-up from peer A to peer B.
- Empty peer syncs from populated peer.
- Already-synced peers no-op.
- Duplicate and reordered chunk delivery remains idempotent.
- Bidirectional multi-document round trip converges.

Ambiguous case:

- `same_scalar_concurrent_lww_agreement` intentionally leaves the scalar winner
  implementation-defined. The Rust test should assert peer agreement and
  allowed-value membership, not a hard-coded value.

Implementation caution:

- Before these fixtures can become storage-level sync tests, exchanged chunk ids
  must be peer-scoped or content-addressed. Local `chunk-NNNN` ids alone are not
  a safe sync frontier for disconnected peers.
- Seeded fixtures require shared baseline history. Replaying the same semantic
  seed independently on peer A and peer B is not equivalent.
