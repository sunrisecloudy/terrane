# /simplify — Rust codebase simplification plan (audit 2026-06-14)

Read-only architecture audit (WF) → target modular decomposition + ordered,
behavior-preserving, replay-gated refactor plan. Goal: the codebase is (1) easy to
understand fully, (2) fully tested, (3) structured so a new feature lands in ONE
focused place. **Master invariant for every step:** `cargo test --workspace` stays
green AND `cargo run -p forge-cli -- demo` stays `REPLAY IDENTICAL` — every step is a
PURE MOVE (no logic/rename/reorder of semantics), independently shippable, demo-gated.

## Target architecture
- **forge-core/workspace.rs (4312 LOC) → ~400-LOC facade**: WorkspaceCore state +
  factory injection + a thin `handle()` that authorizes then routes through a
  **command registry** (`commands/mod.rs` maps name→handler). Per-command modules:
  `commands/{applet,runtime_run,replay,ui,schema,query,workspace_export}.rs`.
  Cross-cutting layers as isolated modules: `auth.rs` (CR-A3 RBAC), `signing.rs`
  (SC-15/MP-4 verify+bind), `determinism.rs` (seed/run-id keying), `persistence.rs`
  (KV namespace). `sync_rbac.rs` already extracted — the proven pattern.
- **forge-runtime/host.rs (2759 LOC) → `host/mod.rs` hub + per-namespace handlers**
  (`host/{time,storage,db,net,files,ui,log}.rs`) sharing `host/budget.rs`
  (HostBudgets) + `host/policy.rs` (check_or_record_denial). The `HostBridge` trait
  (runtime/bridge.rs) is already the capability seam — a new `ctx.*` = one
  `host/<ns>.rs` + one trait method + one thin forwarder.
- **forge-storage/lib.rs (3218) → per-concern modules** `{store,kv,records,
  records_indexed,mutations,oplog,crdt,runs,query_exec}.rs` + shared `errors.rs`;
  `crdt_write.rs`/`export.rs`/`query.rs` → directory modules by sub-concern.
- **forge-pipeline/scan.rs (1754) → `scan/{models,alias,scopes,visitor,parse,mod}.rs`**
  preserving the multi-pass order.

## Extension seams (what makes features independent)
1. **Command registry** (`core/commands/mod.rs`) — new command = one registration + one module, never a match-arm edit.
2. **Capability/host-call trait** — `HostBridge` + per-namespace `host/<ns>.rs`; new `ctx.*` = one module.
3. **Per-feature core modules** — auth/signing/determinism/sync_rbac as isolated, auditable policy layers.
4. **Shared `storage/errors.rs`** — map_sql/map_json/is_busy used by all split modules.

## Dedup (unify duplicated logic)
HostBudgets (5 inline counters → one struct); check_or_record_denial (one orchestrator);
signed-rule normalization (net vs files → one generic canonicalizer); storage error
mappers (one errors.rs); oplog payload schema (local + remote → one OplogPayload);
envelope codecs (write+read together); JSON-path construction (one json_path.rs);
export table-copy ordering (one table_copy.rs); secret resolution (one path inside the
net closure); unknown-rejection chokepoints (one each in commands/ + signing).

## Ordered steps (low-risk first; each a separate commit, demo-gated)
1. **[low]** storage `errors.rs` — extract map_sql/map_json/is_busy/parse_counter_value. Pure move.
2. **[low]** pipeline `scan/` dir module — split preserving pass order.
3. **[low]** core `determinism.rs` — derive_seeds/fnv1a64/unique_run_id/seed_field (the replay-keying contract).
4. **[low]** core `auth.rs` — the CR-A3 RBAC layer (authorize/role gates/scope), order preserved.
5. **[med]** core `persistence.rs` — KV-schema (ui-tree/run-counter/lifecycle), atomicity ordering preserved.
6. **[med]** core `signing.rs` — verify→bind sources→bind manifest→reject-unknown pipeline + dedup net/files normalization.
7. **[high]** storage `lib.rs` split into per-concern modules; lib.rs becomes a re-export facade (surface byte-stable).
8. **[high]** storage `crdt_write/`+`export/`+`query/` dir modules (keep transact closure + table-copy order intact).
9. **[med]** runtime `host/{budget,policy,time,log,ui}.rs` — extract low-coupling pieces + unify HostBudgets.
10. **[high]** runtime `host/{storage,db,net,files}.rs` — heavy capability handlers as per-namespace modules.
11. **[high]** core `commands/` registry + feature modules; workspace.rs → ~400-LOC facade.

## Global risks (guardrails)
- **Record/replay byte-identity is the master invariant** — run the DEMO every step (tests passing isn't enough). High-exposure: 3,7,8,9,10.
- **Transaction atomicity** — DL-4 chunk+oplog+projection+FTS must stay in ONE `Store::transact` closure; keep `_tx` helpers with their owner.
- **Shared mutable state single-instance** — one HostBudgets, one RunRecorder, one PolicyEngine snapshot, one findings vec, one grant/membership table.
- **Fail-closed ordering** — signing / sync-RBAC / auth pipelines must never reorder/cache/defer/skip a guard.
- **Public re-export surface byte-stable** — facades `pub use` each moved symbol so downstream crates compile unchanged at every step.
- **Step independence** — never combine two megafile splits in one step; each is bisectable.
- **Test co-location** — in-file tests move with their module; integration tests in `tests/` stay and must stay green.
