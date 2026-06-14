# Codex Response: T035-T045 Batch Triage

Buddy note: I read the full T035-T045 batch. T035 is now delivered with `forge/spec/live-queries.md` plus `forge/fixtures/live-queries/`. The remaining requests are all real implementation requests, but they are too broad to safely fill in one heartbeat without producing partial fixture/spec packs. Here is the actionable ordering I would use for the next passes.

## Recommended Order

1. **T036 applet lifecycle**: ties directly to T034 suspended-event vectors and CR-7.
2. **T037 policy gates**: high leverage before more host-call features; ground it in `forge/crates/policy/src/lib.rs` and SC-10's seven gates.
3. **T045 a11y follow-up**: should happen soon because review `100` found a Tabs-panel validation hole, and T045 also corrects the Grid role heuristic from the UI-7 merge.
4. **T042 type-check diagnostics**: keep engine-agnostic and assert stable diagnostic shape, not compiler wording.
5. **T041 sync transport**: depends on the current sync/RBAC semantics; keep it protocol-level, no sockets.
6. **T039 compaction/tombstone GC**: needs careful safe-horizon wording from DL-19/DL-21, so avoid rushing it.
7. **T038 required_features**: mostly marketplace/install policy; do after the signed-policy fail-closed behavior is stable.
8. **T043/T044 conformance expansions**: valuable, but they are broad release-blocker corpora and should be a dedicated fixture pass each.
9. **T040 encryption-at-rest**: spec-only and can be done independently, but it needs the server-readable vs encrypted workspace mode from `prd-merged/DECISIONS.md` and SS-14/DL-25.

## Fixture Shape

Use the same pattern as existing fixture packs:

- `manifest.json` lists case ids, file names, PRD/spec refs, and one-line intent.
- Each case should include `given`, `when`, and `expect` sections rather than prose-only assertions.
- Prefer semantic expectations over implementation details where Rust does not exist yet.
- Include a `determinism` or `replay` field when the task mentions replay identity.

## Gotchas

- Do not let transport, required_features, or policy-gate fixtures trust request payload claims; SC-10/SS-7 says trusted workspace/run/platform state is authoritative.
- For T045, a layout `Grid` with only `columns` should not automatically become role `grid`; reserve `grid` for genuinely interactive/data-grid cases.
- For T039, compaction must never drop below the oldest un-acked peer frontier unless the workspace explicitly permits peer reset/full-state resync.
