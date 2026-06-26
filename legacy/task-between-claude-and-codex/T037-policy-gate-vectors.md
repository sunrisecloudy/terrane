---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/spec/policy-gates.md, forge/fixtures/policy-gates/*.json, forge/fixtures/policy-gates/manifest.json
---

# T037 — SC-10 seven-gate decision vectors

The audit found SC-10's seven-gate decision has 3 gates stubbed AllowAll
(workspace-policy, run-profile, platform-permission). I want vectors to drive wiring
each stub to a real trusted decision source.

## Deliverables
1. `forge/spec/policy-gates.md` — derive from prd-merged/07 (SC-10) and the existing
   DecisionContext seam in forge/crates/policy/src/lib.rs. Enumerate all 7 gates in order,
   what each checks, its trusted source, and the fail-closed default. Focus on the 3 stubs:
   workspace-policy (workspace-level allow/deny of a capability), run-profile (the run's
   declared profile bounds), platform-permission (OS-granted capability availability).
2. `forge/fixtures/policy-gates/<case>.json` + manifest. Each: a host-call request, the
   trusted gate inputs (workspace policy, run profile, platform grants), and the expected
   gate decision (which gate, allow/deny, reason).

## Coverage (~12)
all gates pass -> allow; workspace-policy denies a capability -> deny at gate; run-profile
excludes a capability -> deny; platform-permission unavailable (e.g. no camera) ->
PlatformUnavailable; capability not in manifest -> deny at the manifest gate; RBAC role
lacks permission -> deny; resource limit exceeded -> deny; the ORDER of gate evaluation is
asserted (first failing gate wins); a fail-closed default when a gate input is missing.

In `## Result`, note that all 3 stubbed gates read TRUSTED workspace/run/platform state,
never the request payload (mirrors review 048/050).

## Result
(codex fills this in)
