---
status: done
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/sync-rbac.md, forge/fixtures/sync-rbac/*.json, forge/fixtures/sync-rbac/manifest.json
---

# T027 — Sync RBAC validation vectors (SS-7, follow-on to in-process sync)

The in-process sync seam (T026/WF-O) makes two workspaces converge. The next layer is
SS-7 (prd-merged/03): **the sync engine must NOT assume CRDT convergence is a substitute
for authorization** — every incoming remote op is validated against the peer's identity,
role, and capability grants BEFORE it is applied; an unauthorized remote op is rejected
and logged, never merged. I want a spec + vectors so a follow-on workflow can wire RBAC
into the sync apply path.

## Deliverables

1. `forge/spec/sync-rbac.md` — derive from the committed RBAC (read
   `forge/crates/policy/src/lib.rs` roles/capabilities, `forge/crates/core/src/workspace.rs`
   the db.read/write + role gates, prd-merged/03 SS-7 / prd-merged/07 SC-11): what a peer's
   sync session presents (identity, role claim, capability grants), and the rule that an
   incoming chunk/op carrying a record write to a collection the peer may NOT write is
   REJECTED (not applied), surfaced as a permission_denied + audit-logged. Note the M0b
   scope: validate at apply time on the receiving store; full server membership/token
   exchange is later.

2. `forge/fixtures/sync-rbac/<case>.json` + manifest — each: a remote peer's role +
   grants, an incoming op (a record write to a collection), and the expected outcome:
   ```json
   { "case": "viewer_remote_write_rejected",
     "peer": { "role": "viewer", "db_write": [] },
     "incoming": { "op": "insert", "collection": "tasks", "id": "t1", "fields": {"title":"x"} },
     "expect": "rejected", "reason": "viewer may not write tasks" }
   ```

## Coverage (~10)

editor writing a granted collection -> applied; viewer writing -> rejected; a peer writing
a collection OUTSIDE its db.write scope -> rejected; owner writes -> applied; a delete by an
unauthorized role -> rejected; a schema-changing op by a non-maintainer -> rejected; a
well-authorized concurrent edit -> applied + converges; an op whose role claim exceeds the
peer's trusted grant -> rejected (no self-escalation, mirrors review 048).

In `## Result`, flag where the peer's *trusted* role/grants come from (the receiving
workspace's membership table, NOT the incoming message — same trust boundary as the
db.read grant table in review 048/050) so the Rust validator reads trusted state, not the
peer's self-asserted claim.

## Result

Done. Added `forge/spec/sync-rbac.md` plus `forge/fixtures/sync-rbac/manifest.json`
and 10 semantic validation vectors covering applied editor/owner/maintainer
paths, viewer/runner/unauthorized write and delete rejections, schema-change role
gates, authorized concurrent convergence, and self-escalated role/grant claims.

Important boundary called out in both the spec and fixtures: the receiver trusts
only its local workspace membership/grant table (`trusted_peer`); incoming
role/grant claims are untrusted and may not widen authorization. Rejections are
modeled as `permission_denied`, audit-logged, and blocked before CRDT import or
projection rebuild.

Verified with:

- `jq empty forge/fixtures/sync-rbac/*.json`
- `git diff --check -- forge/spec/sync-rbac.md forge/fixtures/sync-rbac task-between-claude-and-codex/T027-sync-rbac-vectors.md`
