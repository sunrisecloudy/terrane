# Review 025 — `terrane-cap-native` plan

- **Date:** 2026-07-02
- **Scope:** `docs/terrane-cap-native-plan.md` in worktree `/Users/vehasuwat/.codex/worktrees/8cda/terrane` @ `5c9f9ef7`
- **Reviewer:** Claude (Fable 5)
- **Method:** read all ten files of `docs/cap-best-practice` (identical between `5c9f9ef7` and `main`), the framework capability grouping the plan was generated from, then verified the plan's claims about current behavior against the worktree code.

## Verdict

The direction is right and the plan is unusually doc-literate: it gets the hard
constraints correct (commit-only runtime writes, no free-form operation
smuggling, two permission layers, default-deny, honest platform claims, dedup
against existing caps), and its slices mirror the best-practice checklist
order. Approve the shape.

The substantive gaps are concentrated in one place: **the plan builds its
entire lifecycle on a request-queue pattern (`native.requested` → host
connector drain → trusted `native.complete`/`native.fail`) without once
mentioning `Decision::Effect`/`EdgeRunner` — the sanctioned non-pure shape it
bypasses** — and the queue it adopts is missing four design decisions
(mechanism rationale, replica affinity, stale-pending policy, result size)
that become compatibility-locked the moment Slice 2 ships event shapes.
"Logged facts are forever" (02-contract) makes issues 1–4 pre-Slice-2 work.

## Code grounding (claims verified against current code)

| # | Plan claim | Status | Evidence |
|---|------------|--------|----------|
| 1 | `ctx.resource` writes inside JS/WASM may commit records only; effects/nested runtimes refused | **TRUE** | `terrane-core/src/lib.rs:710-712` — `Decision::Effect(_) \| Decision::Runtime(_)` → "effects and runtime calls are not allowed inside a runtime" |
| 2 | Resource grants are namespace-level at runtime through `namespace_granted` | **TRUE** | `terrane-cap-auth/src/lib.rs:883`; ungranted apps get an empty method table (06-permissions) |
| 3 | Every command must be explicitly classified in `public_authz.rs`; unclassified → refused | **TRUE** | `terrane-host/src/public_authz.rs:18` (`GrantGated { namespace, app_arg_index }`); `Unclassified` defaults to refuse (06) |
| 4 | Richer-than-bool query results need a deliberate `QueryValue` extension | **TRUE** | `QueryValue` is `Bool \| U64` only — `terrane-cap-interface/src/runtime.rs:10-13` |
| 5 | *(implicit)* The sanctioned non-pure shape is `Decision::Effect` + `EdgeRunner` — the plan never engages it | **EXISTS, unaddressed** | closed `Effect` enum `terrane-cap-interface/src/abi.rs:142`; runner `terrane-host/src/edge.rs` → issue 1 |
| 6 | *(context)* Sync ships only CRDT data today | **TRUE** | `terrane-host/src/sync.rs:23-111` — vv exchange + `crdt.merge` only → issue 2 |
| 7 | *(precedent)* Request → pending record → trusted resolution already exists in-repo | **TRUE** | `auth.permission.request` / `permission_requests` map / trusted approve-deny — `terrane-cap-auth/src/lib.rs:26,209-280` |
| 8 | `host/macos` exists for Slice 5's Swift/AppKit fallback | **TRUE** | `host/{cli,macos,mcp,web}`; `host/macos` has `Sources/`, `Tests/`, `project.yml` |
| 9 | Existing caps cover storage/HTTP/build rows (`kv`, `relational_db`, `net`, `build`, `builder`, `harness`) | **TRUE** | all registered in `default_registry()` (unchanged since review-001 claim #4) |

## Strengths

- **The review-024 lesson applied before the bug ships:** the non-goal banning
  a free-form `native.request(operation, payload)` blocks the exact
  smuggle-around-policy side-channel that got `app.import` refused.
- **Constraints section is accurate to the code, not aspirational** — every
  claim in it checks out (grounding table above).
- **Two permission layers cleanly separated** (Terrane grant vs OS permission),
  both outcomes recorded in native events.
- **Honest platform claims** — nothing is "supported" until a real platform
  runner has executed the connector tests.
- **Dedup against existing caps** prevents the classic parallel-store mistake;
  `kv`/`net`/`build` boundaries are stated up front.
- **Slice order mirrors the best-practice checklist** (catalog → crate →
  grants → connector contract → macOS MVP → docs/contract → expansion), each
  slice carrying its own tests.
- **Per-contract mechanics are right:** own `BTreeMap` state slice,
  `app.removed` cleanup, `replay_matches()` + reopen-log tests, cap-owned event
  constructors used by the connector.

## Substantive issues

### 1. (P1) The plan invents a second effect mechanism and never reconciles it with `Decision::Effect`

The entire lifecycle is queue-shaped: an app command commits
`native.requested`, a host connector layer "drains pending requests", results
return as **trusted commands** `native.complete`/`native.fail`. The sanctioned
non-pure shape — `decide` returns `Decision::Effect`, `EdgeRunner` runs it
once, the result commits (05-effects-and-runtimes) — appears nowhere, not even
to be rejected.

There are defensible reasons to prefer the queue: user-mediated operations
(file dialog, OS permission prompt) can block for minutes and must not stall
dispatch, and resource-initiated writes from JS are *forced* into commit-only
anyway (lib.rs:710). There is also in-repo precedent: the
`auth.permission.request` pending lifecycle (grounding #7). But the plan must
own the consequences explicitly:

- **Results are never available in the same backend run.** An app requests
  `dialog.openFile` and reads the answer on a *later* invoke via
  `ctx.resource.native.result(id)`. That is a fundamental programming-model
  statement for app authors; today it's implicit.
- **Two log records per native op** (requested + completed) versus one
  recorded-result event under the effect pattern — at clipboard-write
  frequency that doubles log growth (compounds issue 7).
- Instant machine-mediated ops on the *command* path (CLI
  `native clipboard write-text`) would fit `Decision::Effect` and return
  synchronously the way `net.fetch` does.

**Recommend:** promote the mechanism to a top-level design decision. Either
(a) queue-for-everything, stated, with the async model documented for app
authors — and, per 09, the new lifecycle pattern written into
`docs/cap-best-practice` in the same change series so future reviews don't
flag it as a contract deviation; or (b) hybrid — `Decision::Effect` for
instant ops, queue only for user-mediated classes. Open Decision #2 (who
drains, when) is this same decision: auto-drain-after-every-invoke is a
blocking effect with extra steps; an explicit trusted drain service is the
honest form of the queue.

### 2. (P1) Pending requests carry no executor affinity — sync will eventually double-execute them

`NativeRequestRecord` has no notion of *which host/replica* should execute a
request. Today that's latent: sync ships only CRDT data (grounding #6), so
`native.requested` never leaves the machine. But LAN transport / continuous
sync is the declared next design pass, and the moment native events propagate,
every replica's connector sees the same pending queue — the laptop requests a
dialog and the desktop shows it, or both execute one `external.openUrl`.
Payload shapes are forever (02), so **record the origin replica/host identity
in `native.requested` now** and define drain scope as own-replica-only. One
field today versus a `native.requested.v2` migration later.

### 3. (P1) No stale-pending policy; idempotence is only half-guarded

Slice 4 guards the completion side ("already completed/cancelled requests do
not execute twice"). Missing: requests that were *never* executed — host
crashed, machine slept a week, app removed mid-flight. A connector that drains
pending on open would execute last Tuesday's `dialog.openFile` at startup.
Mirror `terrane-cap-replica`'s two-sided guard and add an orphan policy:

- explicit orphan handling — cancel-on-open recorded as `native.cancelled`
  (reason: stale) or TTL expiry recorded as `native.failed`;
- decide-side: `native.complete`/`native.fail` refuse non-pending request ids;
- fold-side: a duplicated completion event cannot corrupt the record;
- drain re-checks pending-ness at execution time, including "app removed while
  pending" — extend the `app.removed` cascade test to cover an in-flight
  request.

### 4. (P1) Result payloads have no size class — media operations will not fit in the log

The lifecycle records results into the event log. Right for clipboard text and
picker paths; fatal for `screen capture`, `camera`, and `media picker` —
megabytes of binary appended forever per capture. `net` bounds body size;
nothing here says anything about result size. The Slice-1 catalog should carry
a **result-size class** per operation now, and media-class ops need a blob
strategy (host blob store + recorded reference + content hash, with the replay
story for the reference stated) *before* promotion. Deciding this after
`native.completed`'s payload shape ships is a compatibility event (02).

### 5. (P2) The single grant gates the command path too — and five safety classes map to only two dispositions

Open Decision #1 worries that one `native` grant exposes the whole
`ctx.resource.native` surface. The same is true of commands: every
`GrantGated { namespace: "native" }` command is unlocked by that one grant via
MCP `capability_command`. "Keep the first surface small" must bind the
*command* list, not just resources. Separately, the catalog defines five
safety classes (`safe-request`, `user-mediated`, `sensitive`, `admin-only`,
`release-only`) but the policy section only ever produces `GrantGated` or
`Refuse`. Pin the mapping: safe-request / user-mediated → `GrantGated`;
sensitive → `Refuse` until operation-level selectors exist; admin-only →
`Refuse` (trusted path only); release-only → not a command at all.

### 6. (P2) The catalog quietly re-admits what the Goals excluded

Goals: "packaging or signing stays in host/release tooling." Desktop group:
"updater/installer, signing." Mobile group: "store packaging, IAP/payments."
Those rows are release tooling; and windows / tray / app-menu / status-bar /
safe-areas / keyboard rows are host plumbing under 01-design's "is it a
capability at all?" test — they never traverse Request→Event→State on an app's
behalf. Marking everything "planned or trusted-only" leaves them latent
operations. **Give the catalog terminal classifications** —
`not-an-operation: release-tooling`, `not-an-operation: host-plumbing` — so a
later promotion requires deliberately re-running the 01 test. (App-controlled
tray/menus is a legitimate Electron-style gray zone; making it an operation
someday should be a recorded decision, not a default row.)

### 7. (P2) Unbounded growth: completed requests are retained for reads with no retention story

Every native op adds two events plus a `NativeRequestRecord` kept so
`result(requestId)` can read it — until when? `app.removed` cleans per-app,
but a long-lived app accumulating thousands of clipboard writes keeps them all
in state and log forever. Define retention: read-once results, keep-last-N per
app, or a housekeeping `native.pruned` fact. Compaction is already an open
item from the sync work; this design would be its biggest customer.

### 8. (P2) The binary e2e layer is missing from the slices

07-testing requires four layers. The slices cover unit, capability, engine,
and fake-connector host tests — but no
`rust/crates/terrane-host/tests/cap/native.rs`. Add it: default-run coverage
for validation/refusal paths through the real binary; `#[ignore = "…"]` for
anything touching real OS UI, per the `net`/`model` convention.

### 9. (P2) `native.supports` bootstrap and re-observation are unspecified

`supports` must answer from folded state (the `native.platform.observed`
facts) — never a live connector probe, which would be non-determinism in
`query`. Say that explicitly, then answer the consequences: a fresh log has no
observation, so everything reads unsupported until the trusted host records
one — when does it (every `Core::open`? once per host version?), and what
re-triggers observation when a host binary upgrade changes the supported set?
Without a rule, decide-time support-gating silently depends on host startup
ordering.

## Nits (P3)

- **Verification gate:** `UPDATE_DOCS=1 … app_api_doc` *regenerates*; the gate
  should run the drift check bare and reserve `UPDATE_DOCS=1` for intentional
  surface changes. The gate also omits the plain workspace `cargo test` that
  CLAUDE.md requires before any commit.
- **Headless hosts:** `terrane-host` underlies mcp/web too; feature-gate
  GUI-touching connector deps (clipboard, dialogs, notifications) so headless
  builds don't drag them in.
- **Command naming:** `native.clipboard.writeText` is camelCase; repo command
  convention is lowercase/kebab (`kv.storage.set`, `harness.generate-app`) →
  `native.clipboard.write-text`. camelCase remains correct for the JS resource
  methods.
- **`result(requestId)` JSON:** build with `serde_json`/`nanoserde`, never
  `format!` (03's past-review bug).
- **"Lifecycle facts" as events** is telemetry-in-the-log (every app start,
  forever). Record only facts something folds on; expose the rest as resource
  reads.

## Answers to the plan's Open Decisions

1. Namespace-wide grant is acceptable for v1 **iff** both the resource *and*
   command surfaces stay at the small safe subset (issue 5); anything in the
   `sensitive` class waits for operation-level selectors.
2. Explicit trusted drain service (poll/wait) — auto-drain-after-invoke
   reintroduces blocking-effect semantics with extra steps. Fold into issue
   1's mechanism decision.
3. Do not extend `QueryValue` for v1 — state-backed resource reads cover
   richer results; extend "deliberately, not casually" (04) only when a second
   capability needs it.
4. Windows next: the desktop connector semantics carry over and the CLI host
   already runs there; Android/iOS each need a new host workspace first.

## Bottom line

Approve the architecture (deterministic crate + host connector layer,
default-deny, catalog-first). Before Slice 2 lands any event shape, settle:
the queue-vs-`Decision::Effect` mechanism decision (1), origin/executor
identity in `native.requested` (2), the stale-pending policy (3), and the
result-size class in the catalog (4). Everything else can land inside the
existing slice structure.
