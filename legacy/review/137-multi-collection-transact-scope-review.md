# Commit Review: a329a00e

Reviewed commit: `a329a00e forge-core: reject multi-collection transact (DL-17 unsupported) + owner-scope db.unwatch (DL-16 review 131/132)`

Line references below are for the committed `a329a00e` snapshot, not the current dirty worktree.

## Findings

### P1 - Unsupported multi-collection `ctx.db.transact` is not recorded, so failed runs can fail replay

The new storage guard rejects multi-collection transact groups with an applet-facing `CoreError::QueryError` before any write is committed (`forge/crates/storage/src/crdt_write/mutation.rs:292-303`). But applet `ctx.db.transact` reaches that path through `RunRecorder::host_call` (`forge/crates/runtime/src/host/db.rs:144-149`), and `host_call` only appends a recorded call after `live()?` succeeds (`forge/crates/runtime/src/recorder.rs:221-231`). So a run that hits this new rejection records no `db.transact` event; replaying the same program will issue `ctx.db.transact` again and find no matching recorded call, surfacing replay divergence instead of reproducing the original rejection. That breaks the deterministic replay/audit promise for this new unsupported-but-reachable applet behavior.

Suggested fix: preflight the collection set in `HostContext::db_transact` before calling into the bridge, and record the rejection using the same denial/error-recording pattern used for policy denials, or add a generic recorded error envelope for host calls. Add a runtime run+replay regression where an applet attempts a multi-collection `ctx.db.transact` and the failed run replays to the same error.

### P2 - Normative PRD still describes DL-17 transact as a merged unit

This commit updates `forge/spec/query-dsl.md` and `forge/spec/live-queries.md` to say M0a `transact([...])` must target one collection, but the spec of record still says `transact([...])` "groups into one CRDT commit (atomic locally, merged as a unit)" in `prd-merged/02-data-layer-prd.md:70`. Since `AGENTS.md` makes `prd-merged/` normative for v1, the code now intentionally contradicts the active PRD unless that scope cut is recorded there.

Suggested fix: add an explicit M0a decision/backlog note in `prd-merged/DECISIONS.md` or revise `prd-merged/02-data-layer-prd.md` to scope current `transact` support to one collection and track multi-collection atomic sync as future DL-17 work.

## Verification

Not run: the current worktree has uncommitted follow-up edits in the same storage/live-query files, so local test results would not verify committed `a329a00e` exactly.
