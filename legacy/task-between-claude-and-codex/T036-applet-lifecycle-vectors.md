---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: forge/spec/applet-lifecycle.md, forge/fixtures/lifecycle/*.json, forge/fixtures/lifecycle/manifest.json
---

# T036 — Applet lifecycle vectors (CR-7)

Only applet.install exists today. CR-7: install -> enable -> run (long-lived,
event-driven) -> suspend -> upgrade (atomic) -> uninstall (with data-retention choice).
Spec + vectors before the Rust wiring; composes with versioning (T032) and the event loop.

## Deliverables
1. `forge/spec/applet-lifecycle.md` — derive from prd-merged/01 (CR-7), the install path in
   forge/crates/core/src/workspace.rs, and code_hash version identity. Define each state +
   the legal transitions, what enable/suspend mean for event dispatch (suspended applets
   reject ui.dispatch_event — ties to T034), atomic upgrade (new version installed
   transactionally; in-flight runs pinned to their recorded version), and uninstall with a
   retention choice (keep-data vs purge-data, the latter tombstoning records).
2. `forge/fixtures/lifecycle/<case>.json` + manifest. Each: a sequence of lifecycle ops and
   expected resulting state + side effects.

## Coverage (~12)
install->enabled; enable then run dispatches events; suspend then a dispatched event ->
rejected (applet suspended); re-enable resumes; upgrade v1->v2 is atomic (active=v2, v1
retained, a mid-upgrade failure rolls back to v1); uninstall keep-data (records survive,
applet gone); uninstall purge-data (records tombstoned); an illegal transition (e.g. run a
uninstalled applet) -> typed rejection; re-install of same code_hash is a no-op not a new
version; suspend an already-suspended applet -> idempotent; uninstall then install fresh.

In `## Result`, flag how upgrade stays atomic w.r.t. recorded runs (a recorded run replays
against its own code_hash, not the new active version).

## Result
Delivered `forge/spec/applet-lifecycle.md` and `forge/fixtures/lifecycle/` with 13 semantic JSON vectors plus `manifest.json`.

Contract decisions encoded:

- A successful first `applet.install` creates an enabled v1 applet (`install_generation = 1`, `version = 1`).
- `runtime.run` and `ui.dispatch_event` require an enabled active applet; suspended applets reject dispatch with `ui.applet_not_dispatchable` before handler execution, matching T034.
- `applet.enable` is the explicit re-enable transition T036 needs even though it is not yet in `forge/spec/commands.md`; the lifecycle spec calls out that the command catalog should add it before wiring.
- `applet.upgrade` is atomic: the active pointer moves to v2 only after validation/compile/schema work commits, and a staged failure leaves active v1, schema, records, and versions unchanged.
- Recorded runs stay pinned to their own per-run program artifact and recorded `code_hash`; replay after upgrade must use the run's v1 code hash, not the new active v2 code hash.
- `applet.uninstall` supports `keep_data` and `purge_data`; keep-data preserves applet-owned records, while purge-data tombstones them with `applet.uninstall:purge_data`.
- Reinstalling the same manifest/source/code hash over the active applet is an idempotent no-op, not a new version; reinstall after uninstall starts a fresh install generation.
- Implementation note: current `cmd_applet_install` in `forge/crates/core/src/workspace.rs` bumps the version on any reinstall; T036 intentionally pins the future lifecycle behavior where same-payload reinstall is a no-op and different active payloads flow through `applet.upgrade`.
