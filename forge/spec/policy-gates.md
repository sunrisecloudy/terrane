# Policy gates (SC-10 seven-gate decision)

Source of record: `prd-merged/07-security-prd.md` **SC-10** and the
`DecisionContext` / `PolicyEngine` seam in `forge/crates/policy/src/lib.rs`.
This note is the semantic contract for the vectors in
`forge/fixtures/policy-gates/`; it is not a wire format.

> **SC-10.** A run is allowed only if **all** pass: actor role permits operation
> ∧ workspace policy permits capability ∧ manifest requests it ∧ run profile
> permits it ∧ platform permission granted ∧ resource matches allowlist ∧
> rate/resource limit available. Decisions are evaluated in the Rust policy
> engine on **every command and every remote sync op (SS-7)**; shells may
> **tighten, never loosen**.

A run (a `ctx.*` host call, or a remote sync op) is allowed **only if all seven
gates pass**. The decision is a conjunction: one failing gate denies the whole
call. No call is counted against the host-call budget unless every gate passes.

## The seven gates, in order

The policy engine evaluates the gates in the SC-10 conjunct order below. The
**first failing gate wins**: its error is the one surfaced, and the later gates
are not consulted. Each gate names itself and the capability category in its
reason so the denial is auditable.

| # | Gate | What it checks | Trusted source | Fail-closed default | Error on failure |
|---|------|----------------|----------------|---------------------|------------------|
| 1 | **actor-role** | The actor's role may run applet code at all (Owner/Maintainer/Editor/Runner; Viewer/Auditor/Reviewer are read-only). | `ActorContext.role`, resolved from trusted workspace membership. | A role that cannot run denies. | `PermissionDenied` (names "role") |
| 2 | **workspace-policy** | The workspace admin policy permits this capability *category*, independent of any one applet's manifest. | `WorkspacePolicy { allowed, denied }` — trusted workspace policy state. | A category absent from the allow list (or present in the deny list) denies. | `PermissionDenied` (names "workspace policy") |
| 3 | **manifest** | The applet's manifest declares (requests) this capability category. | `Manifest.capabilities` — the signed/enforced manifest. | An undeclared category denies (`CapabilityRequired`). | `CapabilityRequired` (undeclared) or `PermissionDenied` (declared but resource out of scope — see gate 6) |
| 4 | **run-profile** | The run's declared profile permits this capability (a locked-down profile, e.g. iOS review-safety SC-21, *narrows* what a run may do). | `RunProfile { name, permitted }` — trusted run state. | A category outside the profile bounds denies. | `PermissionDenied` (names "run profile") |
| 5 | **platform-permission** | The host OS has granted this capability (clipboard, camera, notifications…). | `PlatformPermissions { granted }` — trusted platform state the host reports. | A category the platform has not granted is **unavailable** (absent, not refused). | `PlatformUnavailable` (names "platform permission") |
| 6 | **resource-allowlist** | The concrete resource (storage key / db collection) matches a granted scope in the manifest. | `Manifest.capabilities` scopes (storage prefix globs, db collections). | A resource outside every granted scope denies. | `PermissionDenied` (names the resource and scope) |
| 7 | **rate/resource-limit** | A host-call budget remains (`manifest.limits.max_host_calls`, the SC-2 flood guard). | `Manifest.limits` — the signed/enforced limits. | An exhausted budget denies. | `ResourceLimitExceeded` |

Gates **3** (manifest declared) and **6** (resource allowlist) are two halves of
the manifest+resource subcheck (`CapabilityCheck` in the engine) and also carry
the immediate-revocation hook (CR-4: a revoked category denies before its
manifest grant is consulted).

### Engine evaluation order vs. the SC-10 conjunct order

The conjunction is commutative for the final allow/deny, so the only observable
effect of order is **which gate is named first** when more than one would fail.
The engine evaluates in this concrete order (`PolicyEngine::check`):

1. actor-role (gate 1)
2. **rate/resource-limit (gate 7)** — the budget is checked early so a hostile
   loop that has already flooded its budget cannot distinguish later denials by
   error code (the flood guard subsumes them).
3. the three `DecisionContext` gates together — **workspace-policy (gate 2),
   run-profile (gate 4), platform-permission (gate 5)** — in that order, via
   `check_context_gates`.
4. the manifest+resource subcheck — **manifest (gate 3) + resource-allowlist
   (gate 6)**.

So the workspace-policy gate is always evaluated before the manifest subcheck
(matching SC-10's "workspace policy permits capability" *before* "manifest
requests it"). The budget gate is hoisted ahead of the others as a deliberate
flood-guard hardening; every other gate keeps the SC-10 relative order. The
fixtures assert the gate the engine actually surfaces first.

## The three trusted-source gates (the wired stubs)

SC-10's actor-role and rate-limit gates were already enforced directly by the
engine. Gates **2 (workspace-policy)**, **4 (run-profile)**, and **5
(platform-permission)** were previously `AllowAll` stubs behind the
`DecisionContext` seam. They are now wired to real trusted-source evaluation in
`ComposedDecisionContext`:

- **workspace-policy** → `WorkspacePolicy`: an explicit allow/deny over capability
  categories. `denied` wins on conflict; a category in neither set is denied
  fail-closed.
- **run-profile** → `RunProfile`: the run's declared profile and its permitted
  capability bounds. A category outside the bounds is denied fail-closed.
- **platform-permission** → `PlatformPermissions`: the OS-granted capability set.
  A category the platform has not granted yields `PlatformUnavailable` —
  distinct from a policy denial, because the capability is *absent*, not
  *refused*.

### Trusted source, never the request payload

Every one of these three gates reads **trusted workspace / run / platform
state** — `WorkspacePolicy`, `RunProfile`, `PlatformPermissions` — resolved at
the command boundary (and at the remote-sync boundary, SS-7). **They never read
the request payload.** This mirrors `review 048/050`: an applet (or a sync peer)
cannot widen its own grants by asserting them in the request; the policy
decision is made only against state the host trusts. Incoming claims may
*narrow* the trusted state but must never widen it.

### Live wiring (the gates are on the real decision path)

The three gates are **wired into the live runtime decision path**, not merely a
tested library. `forge-core` holds the trusted SC-10 inputs as a persisted
`RunPolicy`, set ONLY through the trusted `WorkspaceCore::set_run_policy` seam
(workspace configuration, mirroring `db_read_grants` / `sync_membership`) and
read only at the run boundary — never from a command's `payload`. On every
`runtime.run`, `ui.dispatch_event`, and live-query notification delivery,
`WorkspaceCore` builds a `ComposedDecisionContext` from that trusted state and
installs it on the run via the runtime's `record_run_with_context` /
`record_dispatch_with_context` / `record_notification_with_context` entry points,
so the gates are consulted on the run's actual `ctx.*` host calls. A configured
deny therefore **blocks a live command** (proven end-to-end in
`crates/core/tests/policy_gates_live.rs`, driving the real `WorkspaceCore::handle`
path).

An **un-provisioned** workspace (no `RunPolicy` set) installs the permissive
`AllowAll` context — the M0a spine baseline, so the demo and existing applets are
unaffected. A **provisioned** policy only ever *adds* gate denials relative to
that baseline: a gate the admin leaves unspecified defaults to permitting all
categories, so configuring a single deny (e.g. forbid `db`) restricts exactly
that gate (shells tighten, never loosen).

## Fail-closed default

Every gate denies when its input is **missing or ambiguous** — it never silently
allows:

- workspace-policy: a category neither allowed nor denied → deny (the workspace
  never positively granted it). The empty/default policy denies everything.
- run-profile: a category outside the profile's permitted bounds → deny. The
  empty/default profile permits nothing.
- platform-permission: a category the OS has not granted → `PlatformUnavailable`.
  The empty/default permission set makes every capability unavailable.

## First-failing-gate-wins

When a call would fail more than one gate, the engine surfaces the **first** gate
to fail in its evaluation order and stops. The reason names that gate. This is
asserted directly by the `order_*` fixtures (e.g. a workspace-policy deny on a
call whose resource is *also* outside the manifest scope surfaces the
workspace-policy denial, because gate 2 runs before the gate-6 resource check).

## Replay determinism

Gate decisions are made by the **live** `ComposedDecisionContext` only during the
**original** run, and the *outcome* is recorded. Replay rebuilds the engine from
the recorded `PermissionSnapshot` with the permissive `AllowAll` context
(`PolicyEngine::from_snapshot`) and replays the recorded decisions — it does
**not** re-consult the live workspace/run/platform sources. A call those gates
denied at record time was never recorded as allowed, and the runtime records a
context-only denial through the same channel as a manifest-scope denial
(`check_context_gates`). So a real workspace/run/platform deny replays
identically without re-imposing today's policy on a historical run, and the demo
stays `REPLAY IDENTICAL`.

## Shells tighten, never loosen

SC-10 is evaluated in the Rust policy engine. A platform shell may add denials on
top of a core allow (tighten) but can never remove a core denial (loosen): the
core decision is the floor. A `ComposedDecisionContext` can therefore only ever
*add* gate denials relative to `AllowAll`; it cannot turn a core denial into an
allow.
