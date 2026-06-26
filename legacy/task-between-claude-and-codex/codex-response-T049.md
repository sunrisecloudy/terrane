# T049 - Draft M0b Final Report

Status: draft for Claude to refine/finalize. Scope is the frozen current M0b state, with the real JSC backend and tsgo type-check sidecar explicitly deferred by user direction.

## 1. Master Invariant Status

The acceptance gate for this frozen M0b wrap-up should be stated as:

- `cargo test --workspace` green
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo run -p forge-cli -- demo` exits zero and prints `REPLAY IDENTICAL: true`

Claude should paste the live command output here. Codex did not run the full workspace gate for this draft. The CLI demo is the same acceptance path tested by `forge/crates/cli/tests/e2e.rs`, which asserts install -> run -> SQLite write -> UI tree -> byte-identical replay and checks the printed `REPLAY IDENTICAL: true` line.

## 2. Feature-Completeness Matrix

Legend: `yes` means implemented and covered in current `forge/`; `partial` means a real substrate exists but the full PRD wording or platform/product shell is not complete; `deferred` means out of frozen scope or later milestone.

### Core Runtime (CR)

| ID | Status | Realization | Coverage |
|---|---|---|---|
| CR-1 | yes | `forge-runtime` QuickJS realm exposes only injected `ctx.*`; `forge-pipeline` scan rejects escape hatches. | `runtime/tests/containment.rs`, `pipeline/tests/corpus_rejects.rs`, `fixtures/conformance/forbidden_eval_rejection.json` |
| CR-2 | partial | `JsEngine` trait and QuickJS implementation exist; no real JSC or QuickJS-WASM backend in frozen scope. | `runtime/tests/conformance_engines.rs`, `spec/cross-engine-conformance.md` |
| CR-3 | partial | `ctx.db`, `ctx.storage`, `ctx.ui`, `ctx.time`, `ctx.random`, `ctx.net`, `ctx.secrets`, `ctx.files` are wired; `llm`, `schedule`, and platform capabilities are deferred. | `core/src/bridge.rs`, `runtime/src/engine.rs`, files/network/secrets/e2e fixtures |
| CR-4 | yes | Manifest and trusted policy checks happen at host-call/command time; grants are persisted in workspace state. | `policy/tests/policy_gates_vectors.rs`, `core/tests/policy_gates_live.rs`, `core/tests/files_conformance.rs` |
| CR-5 | partial | CPU/stack/host-call/log/storage/quota gates exist; full timer/concurrent-net model and shell-visible suspension UX are not complete. | `runtime/tests/containment.rs`, `core/tests/quota_core_conformance.rs`, `core/tests/quota_run_logs_cap.rs` |
| CR-6 | partial | `runtime.run`, `ui.dispatch_event`, `db.watch`, callbacks, and replay sessions exist; timers/schedule/work-stealing pool are deferred. | `core/tests/ui_dispatch_event.rs`, `core/tests/live_query_callback.rs`, `fixtures/ui-events/`, `fixtures/live-queries-e2e/` |
| CR-7 | partial | Install, enable, suspend, upgrade, uninstall are implemented with atomicity tests; applet package-as-full-CRDT-doc surface is not complete. | `core/tests/lifecycle.rs`, `core/tests/lifecycle_vectors.rs`, `fixtures/lifecycle/` |
| CR-8 | yes | Deterministic script/app run, seeded time/random, recorded host responses, replay. | `runtime/tests/determinism.rs`, `core/tests/spine.rs`, `fixtures/replay/` |
| CR-9 | yes | Run records persist code hash, input, host calls, logs, outcome, and replay artifacts. | `domain/src/run.rs`, `core/src/commands/replay.rs`, `fixtures/replay/` |
| CR-10 | partial | Sources/manifests are retained and installable; multi-file collaborative package documents are only partly represented. | `core/src/commands/applet.rs`, `fixtures/signing/valid_multi_file_package.json`, `examples/notes-lite/` |
| CR-11 | partial | Command API and client feature registry exist; full public `forge-api@MAJOR.MINOR` SDK/version promise is not complete. | `core/src/commands/mod.rs`, `core/src/features.rs`, `fixtures/required-features/` |
| CR-12 | partial | Engine-agnostic conformance spec/corpus/harness and determinism hardening exist; real JSC, QuickJS-WASM, and full host-API cross-engine CI are deferred. | `spec/cross-engine-conformance.md`, `fixtures/conformance-engines/`, `runtime/tests/conformance_engines.rs` |
| CR-13 | yes | Static SWC policy scan plus engine-level eval/Function/random/Date hardening. | `pipeline/tests/bypass_corpus.rs`, `runtime/tests/containment.rs`, `fixtures/conformance-engines/date_under_seeded_clock.json` |
| CR-14 | yes | Offline in-core SWC type stripping and canonical JS hash. | `pipeline/src/lib.rs`, `pipeline/tests/*` |
| CR-15 | deferred | Full offline type-check via tsgo/TS sidecar is skipped under frozen scope; only transpile plus policy scan exists. | no tsgo fixtures; `pipeline/src/lib.rs` notes wasm-clean transpile path |
| CR-16 | partial | Source and transpiled JS are retained; full type-checked artifact + source-map install gate waits on CR-15. | `core/src/commands/applet.rs`, `pipeline/src/lib.rs` |

### Data Layer (DL)

| ID | Status | Realization | Coverage |
|---|---|---|---|
| DL-1 | yes | Loro-backed CRDT storage/convergence substrate. | `crates/crdt`, `fixtures/sync/`, `core/tests/sync.rs` |
| DL-2 | partial | Collection/app state/workspace docs exist as storage concepts; full applet/file/chat/settings doc granularity is not complete. | `storage/src/store.rs`, `sync` fixtures |
| DL-3 | partial | Scalar/list/text CRDT substrate exists; full registry-selected merge semantics are not fully surfaced to applets. | `crates/crdt`, `fixtures/crdt-write/` |
| DL-4 | yes | Mutation -> CRDT chunk/oplog -> SQLite projection in one path. | `storage/src/crdt_write/`, `fixtures/crdt-write/`, `storage/tests/query_fixtures.rs` |
| DL-5 | yes | Query DSL, expression indexes, FTS5, planner warnings, index lifecycle. | `fixtures/query/`, `fixtures/indexes/`, `storage/tests/index_fixtures.rs` |
| DL-6 | yes | Projection rebuild from CRDT chunks and duplicate/out-of-order chunk handling. | `fixtures/crdt-write/*rebuild*.json`, `storage` tests |
| DL-7 | partial | Schema registry and stable-id model exist, but M0 materializes record fields under `f_<name>` stand-ins per `DECISIONS.md` I1. | `core/tests/schema.rs`, `fixtures/migrations/`, `prd-merged/DECISIONS.md` |
| DL-8 | yes | Additive schema changes, compatibility validation, rebuild indexes. | `core/src/commands/schema.rs`, `fixtures/migrations/` |
| DL-9 | yes | Unknown fields/features preserved in record/compat fixtures. | `fixtures/compat/`, `fixtures/crdt-write/unknown_forward_compat_preserved.json` |
| DL-10 | yes | Unknown collection raw/open compatibility covered. | `fixtures/compat/unknown_collection_*.json` |
| DL-11 | partial | Union behavior is represented, but actor-scoped CRDT registry is not fully productized. | `fixtures/migrations/actor_scoped_union_planned.json`, `core/tests/schema.rs` |
| DL-12 | partial | Defaults/required/widening validation exist; full warning-to-enforcement lifecycle is partial. | `fixtures/migrations/enforce_required_after_warn_ok.json`, `core/tests/schema.rs` |
| DL-13 | partial | Migrations and oplog recording exist; lenses/breaking transforms are deferred; field-id stand-in caveat applies. | `spec/migrations.md`, `fixtures/migrations/`, `core/tests/sync_rbac_enforced.rs` |
| DL-14 | partial | Client feature negotiation/refusal exists; old-client limited-mode UX remains partial. | `core/src/features.rs`, `fixtures/required-features/`, `fixtures/compat/limited_mode_min_feature.json` |
| DL-15 | yes | Typed query DSL plus `query.execute` command and `ctx.db.query`. | `fixtures/query/`, `storage/tests/query_fixtures.rs`, `core/tests/*query*` |
| DL-16 | partial | Live queries, callbacks, notification replay stream, and event dispatch are wired; review 183 tracks durability of skipped callback decisions on command paths. | `spec/live-queries.md`, `fixtures/live-queries*/`, `core/tests/live_query_*`, `review/183-watch-callback-skip-durability-review.md` |
| DL-17 | partial | Insert/update/patch/delete and single-collection transact are supported; multi-collection atomic sync boundary is deferred by decision I2. | `fixtures/crdt-write/transact_group_single_chunk.json`, `runtime/tests/determinism.rs`, `prd-merged/DECISIONS.md` |
| DL-18 | partial | Collection grants and per-applet KV namespace exist; row-filter grants are v1.x/later. | `core/src/workspace.rs`, `storage/src/kv.rs`, `policy` tests |
| DL-19 | yes | Compaction and retention windows implemented. | `storage/src/compaction.rs`, `storage` compaction tests |
| DL-20 | yes | `db.history` and non-destructive `db.restore`. | `core/src/commands/time_travel.rs`, `fixtures/time-travel/`, `core/tests/time_travel_command.rs` |
| DL-21 | partial | Tombstone delete and sync-correct deletes exist; hard-purge policy surface is not complete. | `fixtures/time-travel/history_records_a_delete.json`, delete sync tests |
| DL-22 | yes | Quotas, approaching warnings, per-category/per-applet/workspace reports, run-log admission gate. | `spec/quotas.md`, `fixtures/quotas*/`, `core/tests/quota_*` |
| DL-23 | partial | SQLite/WAL durability substrate exists; kill-during-write torture is not confirmed in this draft. | `storage/src/store.rs`; live gate output needed |
| DL-24 | yes | Export/import bundle, exclusion guard, descriptor fixtures. | `storage/src/export/`, `fixtures/export/`, `core/src/commands/workspace_export.rs` |
| DL-25 | deferred | Project-level encryption/SQLCipher/export encryption are out of frozen scope. | none |

### Security / Capability (SC)

| ID | Status | Realization | Coverage |
|---|---|---|---|
| SC-1 | yes | Zero ambient capability plus policy scan. | CR-1/CR-13 coverage |
| SC-2 | yes | Containment corpus and resource-limit tests. | `runtime/tests/containment.rs`, `fixtures/conformance/` |
| SC-3 | partial | One realm per run/applet path exists; full multi-applet communication model is partial. | `runtime/src/engine.rs`, `core` tests |
| SC-4 | deferred | CVE patch/release process is operational, not in `forge/`. | none |
| SC-5 | yes | Net allowlist, private/loopback/metadata deny, redirects rechecked, content/size/timeout gates. | `policy/tests/net_vectors.rs`, `fixtures/network/`, `runtime/tests/net.rs` |
| SC-6 | partial | No install path expands permissions automatically; human review UX is deferred. | `core/src/commands/applet.rs`, signing/required-feature fixtures |
| SC-7 | partial | Static/injection rejection corpus exists; living release process deferred. | `pipeline/tests/corpus_rejects.rs`, `pipeline/tests/bypass_corpus.rs` |
| SC-8 | partial | Capability grammar implemented for current runtime surfaces; full P-09 grammar and all namespaces are partial. | `spec/capabilities.md`, `policy/tests/policy_gates_vectors.rs` |
| SC-9 | partial | Grants/revocation and audit rows exist; resource-specific prompts/member UX are deferred. | `core/src/workspace.rs`, `fixtures/audit-log-e2e/permission_grant_revoke_ordered_rows.json` |
| SC-10 | partial | Command/run gates and sync-applicable gates exist; real shell platform sources remain partial. | `spec/policy-gates.md`, `core/tests/policy_gates_live.rs`, `core/tests/sync_rbac_enforced.rs` |
| SC-11 | partial | Default roles and sync RBAC are enforced; customizable role product surface is partial. | `core/tests/sync_rbac*.rs`, `fixtures/sync-rbac/` |
| SC-12 | partial | Durable audit log/query covers permissions, network, secrets, lifecycle, sync denials; AI/marketplace/crash/admin process breadth remains later. | `core/tests/audit_*`, `fixtures/audit-log-e2e/` |
| SC-13 | partial | Secret refs inject into allowlisted net headers without trace leakage; OS keychain store is shell work. | `crates/secrets`, `fixtures/secrets/`, `runtime/tests/secrets.rs` |
| SC-14 | partial | No runtime npm and curated `@forge/std` subset/spec exist; full signed stdlib governance is deferred. | `forge/std/`, `pipeline` scan tests |
| SC-15 | partial | Ed25519 signing and canonical payload binding exist; release artifact signing/SBOM/cargo audit process is deferred. | `crates/signing`, `fixtures/signing/`, `signing/tests/signing_vectors.rs` |
| SC-16 | partial | Source-visible/sandboxed/signing-ready install substrate exists; publisher auth/abuse workflow is marketplace scope. | `fixtures/signing/`, `fixtures/required-features/`, `core/src/commands/applet.rs` |
| SC-17 | partial | Local core does not let server grant runtime caps; actual server absent. | local install/grant code only |
| SC-18 | deferred | Cloud tenant isolation/break-glass/local workspace upload policy is later server work. | none |
| SC-19 | partial | Export/import exists; compliance program/backups/PII masking are later. | `workspace.export/import`, `fixtures/export/` |
| SC-20 | deferred | Project encryption, biometric lock, remote sign-out purge are out of scope. | none |
| SC-21 | deferred | iOS review-safety mode waits on iOS shell/JSC. | none |
| SC-22 | deferred | Privacy labels generated from config are later release work. | none |
| SC-23 | deferred | External pen test is GA process. | none |
| SC-24 | deferred | Disclosure/security.txt/bounty process is launch/GA process. | none |
| SC-25 | partial | Many SC controls map to tests; no final `/security/controls.md` release table found. | `fixtures/audit-log-e2e/`, `policy-gates`, `network`, `secrets`, `sync-rbac` |

### Sync / Server (SS)

| ID | Status | Realization | Coverage |
|---|---|---|---|
| SS-1 | partial | In-process CRDT chunk sync exists; WebSocket/TLS/presence/control channels are deferred. | `crates/sync`, `spec/sync-protocol.md`, `fixtures/sync/` |
| SS-2 | partial | Chunk/frontier handshake shape is specified and tested in-process; full network handshake is deferred. | `fixtures/sync/`, `core/tests/sync.rs` |
| SS-3 | partial | Protocol is specified transport-agnostic; no alternate transport implementation. | `spec/sync-protocol.md` |
| SS-4 | partial | Durable chunks/oplog and offline convergence are tested; p95/1k pending-op target not verified here. | `fixtures/sync/`, `storage/src/crdt.rs` |
| SS-5 | partial | Unknown-field preservation and feature registry exist; server N-2 negotiation/banner deferred. | `fixtures/compat/`, `core/src/features.rs` |
| SS-6 | deferred | Cloud/embedded auth tokens and pairing are later server/shell work. | none |
| SS-7 | partial | Remote op RBAC and workspace-policy sync gates are implemented in-process; full server surface deferred. | `spec/sync-rbac.md`, `core/tests/sync_rbac_enforced.rs`, `fixtures/sync-rbac/` |
| SS-8 | deferred | Invite links and token revocation/purge are later. | none |
| SS-9 | yes | Permission monotonicity and denied remote ops are covered by sync RBAC vectors. | `fixtures/sync-rbac/`, `core/tests/sync_rbac_enforced.rs` |
| SS-10 | deferred | Conflict UI is not in frozen core. | none |
| SS-11 | deferred | Managed cloud sync nodes/Postgres/object storage are later. | none |
| SS-12 | deferred | Cloud SLO implementation is later. | none |
| SS-13 | deferred | Tenant isolation invariant tests belong to server/cloud. | none |
| SS-14 | deferred | Server-visibility mode/encrypted workspace product surface is not implemented. | none |
| SS-15 | deferred | Embedded server toggle/status/access logs are shell/server work. | none |
| SS-16 | deferred | Relay/mDNS/NAT traversal are later marketplace/server scope. | none |
| SS-17 | deferred | Embedded server optional roles are later. | none |
| SS-18 | deferred | Availability honesty/offline mailbox later. | none |
| SS-19 | deferred | Single binary/Docker `forge-server` not in frozen `forge/`. | none |
| SS-20 | deferred | Cloud/embedded migration flow later. | none |
| SS-21 | deferred | TLS/pairing/relay hardening later. | none |
| SS-22 | partial | Local audit/events exist; server metrics/status/logging deferred. | `core/src/event.rs`, `audit` fixtures |

### UI (UI)

| ID | Status | Realization | Coverage |
|---|---|---|---|
| UI-1 | yes | Typed tree, diff/patch, `ctx.ui.render`, `ui.patch` events. | `crates/ui`, `core/tests/ui_dispatch_event.rs`, `fixtures/ui-events/` |
| UI-2 | partial | M0 subset Stack/Text/Button/TextField/List implemented; broader 26-component catalog is only spec/type surface. | `forge/std/forge-std.d.ts`, `forge/std/ui-catalog.d.ts`, `ui/tests/golden/` |
| UI-3 | partial | Semantic variants exist in types; real shell theming is deferred. | `ui` crate, `std` types |
| UI-4 | partial | ActionRef dispatch, controlled input events, db.watch re-render loop exist; full event queue/timer/perf targets not proven. | `fixtures/ui-events/`, `fixtures/live-queries-e2e/` |
| UI-5 | deferred | Shell virtualization/query handles are not implemented. | none |
| UI-6 | yes | Unknown component fallback/unknown prop tolerance. | `ui/tests/golden/unknown_*.json`, `spec/ui-catalog.md` |
| UI-7 | partial | Accessibility annotations/focus order/golden tests exist; shipped theme/WCAG audit deferred. | `ui/src/accessibility.rs`, `ui/tests/accessibility.rs`, `ui/tests/focus.rs` |
| UI-8 | deferred | Workspace token theming shell-side is not implemented. | none |
| UI-9 | deferred | Navigation/pages/deep links are shell work. | none |
| UI-10 | deferred | Presence UI waits on presence channel/shell. | none |
| UI-11 | partial | Runs can return/render UI; full result-to-Text/Markdown/Table/Card mapping is partial. | `std/forge-std.d.ts`, `cli` demo |
| UI-12 | yes | Versioned wire format, golden tree/patch tests, UI dispatch fixtures. | `ui/tests/golden/`, `fixtures/ui-events/`, `core/tests/ui_dispatch_event.rs` |
| UI-13 | deferred | No actual minimal DOM renderer-zero implementation found; only protocol and conformance seeds exist. | `crates/ui`, no DOM renderer files |
| UI-14 | partial | Golden trees, scripted interaction fixtures, a11y/focus tests exist; screenshot/shared renderer kit is deferred. | `ui/tests/*`, `fixtures/ui-events/` |
| UI-15 | deferred | Editor surface is product shell work. | none |
| UI-16 | partial | Schema commands/data model exist; designer UI deferred. | `core/src/commands/schema.rs`, schema fixtures |
| UI-17 | partial | Data/query/audit/time-travel command surfaces exist; browser UI deferred. | `query.execute`, `db.history`, `audit.query` |
| UI-18 | partial | Permission/audit data exists; resource-specific prompt UX deferred. | `audit-log-e2e`, policy-gates fixtures |
| UI-19 | partial | Time-travel API exists; timeline UX deferred. | `db.history`, `db.restore`, `fixtures/time-travel/` |
| UI-20 | deferred | LLM panel/loop is out of frozen scope. | none |
| UI-21 | partial | Events/audit/resource usage surfaces exist; debug panel UI deferred. | `EventSink`, `audit.query`, quota/status commands |

### Marketplace / Packaging (MP)

| ID | Status | Realization | Coverage |
|---|---|---|---|
| MP-1 | partial | OSS local core exists; commercial central/self-host services deferred. | `forge/` workspace |
| MP-2 | deferred | Cloud account/device/team system deferred. | none |
| MP-3 | partial | Source-visible local install substrate exists; registry/server mirror deferred. | `applet.install`, signing fixtures |
| MP-4 | yes | Signing-ready package manifest/policy/file hash binding with Ed25519 verification. | `fixtures/signing/`, `signing/tests/signing_vectors.rs` |
| MP-5 | deferred | Publisher accounts/abuse/takedown/ratings later. | none |
| MP-6 | partial | Local inspect/install/run flow is represented in commands; human grant prompt UX deferred. | `core/src/commands/applet.rs`, `fixtures/required-features/` |
| MP-7 | deferred | iOS marketplace policy later. | none |
| MP-8 | yes | `required_features`, `min_app_version`, feature negotiation/refusal. | `core/src/features.rs`, `fixtures/required-features/`, `core/tests/required_features_vectors.rs` |
| MP-9 | deferred | Rendezvous/relay later. | none |
| MP-10 | deferred | Provider routing/metering/team policy later. | none |
| MP-11 | deferred | `forge-server` single binary/Docker/admin UI later. | none |
| MP-12 | deferred | Central-service data minimization policy is later server work. | none |

## 3. Review-Closure Ledger

- Review files present: 183 numbered Codex review files under `review/`.
- Pattern: Codex reviews independently tracked P1/P2 defects across storage, runtime, sync, policy, UI, audit, quotas, live queries, CR-12 conformance, and package compatibility. Claude has been closing review findings in follow-up commits.
- Current exceptions to call out honestly:
  - `review/183-watch-callback-skip-durability-review.md`: latest P1, being fixed now. It says skipped over-cap watch-callback decisions are recorded only in `DeliveredBatch.rejected_callbacks`/transient event on command paths, not durable replay state.
  - `review/182-engine-agnostic-harness-review.md`: P2 deferred under freeze. The sentinel engine-injection test and CR-12 wording/full host-API-suite expansion are useful follow-ups, but do not block frozen-scope wrap-up if the deferral is explicit.

Suggested final wording: "All prior blocking P1/P2 findings are closed or folded into the frozen-scope ledger; the live exceptions are review 183 P1 under active fix and review 182 P2 deferred as a known conformance-harness hardening follow-up."

## 4. Explicitly Deferred / Out of Frozen Scope

- CR-12 real JSC backend: deferred by user direction. The `JsEngine` trait, engine-agnostic corpus, deterministic Date/Math hardening, and `forge/spec/cross-engine-conformance.md` are in place so JSC can be added later against the same corpus. The current suite is not the full PRD CR-12 release blocker because it does not run JSC/QuickJS-WASM or all host APIs/limit behavior across engines.
- CR-15 tsgo type-check sidecar: deferred by user direction. Current pipeline is offline SWC transpile plus policy scan; source maps/type-checked install gate remain future work.
- UI-13 renderer zero: no actual minimal DOM renderer implementation found in current `forge/`; `forge-ui` provides the tree/diff/a11y/focus protocol and conformance seed.
- DL-17 multi-collection atomic transact across the sync boundary: intentionally deferred by `prd-merged/DECISIONS.md` I2; M0 accepts single-collection transact and rejects cross-collection groups.
- DL-7/DL-13 registry-stable field materialization: M0 uses the `f_<name>` stand-in scheme per `DECISIONS.md` I1; registry-aware materialization is future work.
- Real-disk `ctx.files` hardening: `ctx.files` host API and confinement vectors exist, but the current core uses injectable filesystem seams; OS-specific sandbox/TOCTOU hardening belongs to shell/runtime integration.
- Cloud/server/marketplace/account features: SS-6, SS-8, SS-11..SS-21, MP-2/5/7/9/10/11/12, and related SC operational processes are not part of frozen M0b.
- Product UX surfaces: editor, schema designer, data browser, permission prompts, time-travel UX, LLM panel, and debug panel are command/data substrates only or deferred.

## 5. WASM Target

Claude's parallel WASM feasibility pass used `CARGO_TARGET_DIR=/tmp/forge-wasm-target`, keeping native `forge/target` untouched, against `wasm32-unknown-unknown` and `wasm32-wasip1`.

Clean on `wasm32-unknown-unknown` today:

- `forge-domain`
- `forge-schema`
- `forge-policy`
- `forge-secrets`
- `forge-ui`
- `forge-testkit`
- `forge-signing`
- `forge-crdt`
- `forge-pipeline`
- `forge-runtime`

Blocked today:

- `forge-storage`: blocked by `rusqlite` bundled SQLite C.
- `forge-sync`, `forge-core`, `forge-cli`, `forge-ffi`: transitively blocked by `forge-storage`.

Details:

- On `wasm32-unknown-unknown`, `libsqlite3-sys` swaps to `sqlite-wasm-rs`, then `cc` fails because Apple clang has no wasm LLVM backend. Even with that toolchain fixed, a browser target still needs an OPFS/IndexedDB-backed SQLite VFS.
- On `wasm32-wasip1`, `rusqlite` tries the normal SQLite C amalgamation and fails because there is no WASI libc sysroot. `wasi-sdk` plus on-disk SQLite is the more plausible WASI path.
- `rquickjs` would otherwise be a hard C blocker, but the native QuickJS implementation is already `cfg(not(target_arch = "wasm32"))` gated. The runtime-side `JsEngine` trait remains wasm-clean.

Final WASM story: the pure logic, crypto, CRDT, schema, TS/SWC pipeline, and runtime abstraction layer are wasm-ready now. The real wall is storage: a browser/WASI host needs a feature-gated or abstracted storage backend, wasm toolchain bits, and a wasm `JsEngine` implementation.

## 6. Suggested Final Summary Paragraph

Frozen M0b now has a working offline headless platform core: TypeScript source installs through SWC and static policy scan, runs in a zero-ambient QuickJS host, writes through capability-gated Rust `ctx` into SQLite/CRDT storage, emits UI trees and patches, replays deterministically, syncs in-process with RBAC, enforces quotas/policy/net/secrets/files, and carries a broad fixture suite across storage, query, migrations, audit, live queries, UI events, signing, required features, and conformance. The wrap-up should avoid overclaiming the unfrozen product: real JSC, tsgo type-checking, DOM renderer-zero, full server/cloud/marketplace, and shell UX remain explicit follow-ups.
