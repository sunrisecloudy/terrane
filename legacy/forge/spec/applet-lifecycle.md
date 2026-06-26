# Applet Lifecycle

Source of record: `prd-merged/01-core-runtime-prd.md` CR-7/CR-8/CR-9, the command catalog in `forge/spec/commands.md`, the installed applet and replay pinning paths in `forge/crates/core/src/workspace.rs`, and the suspended-dispatch expectation in `forge/fixtures/ui-events/suspended_applet_rejected.json`.

CR-7 defines applets as long-lived, UI-bearing, event-driven programs with lifecycle `install -> enable -> run -> suspend -> upgrade -> uninstall`. This spec pins the lifecycle contract before the full Rust state machine exists.

## Identity

An installed applet is identified by:

- `applet_id`: stable logical applet identity inside a workspace.
- `install_generation`: increments when an applet is uninstalled and later installed fresh under the same `applet_id`.
- `version`: monotonically increasing within an install generation.
- `code_hash`: hash of the compiled runnable entrypoint bytes. A recorded run is pinned to the exact `code_hash` and manifest snapshot it executed.
- `manifest_hash` and `sources_hash`: canonical hashes of the install payload used for idempotency and audit.

Version identity is `(applet_id, install_generation, version, code_hash)`. Replay identity is `(run_id, applet_id, code_hash, manifest_snapshot)`.

## Durable States

Durable lifecycle state lives on the active applet record:

- `enabled`: the applet is installed and may run or receive UI events.
- `suspended`: the applet remains installed, but new `runtime.run` and `ui.dispatch_event` requests are rejected before user code starts.
- `uninstalled`: there is no active applet record for the `applet_id`; retention policy determines whether applet-owned data remains.

`running` is not a durable applet state. It is an in-flight run or event-loop turn. Suspending or upgrading an applet affects future work, while an already-started run continues against its recorded version and `code_hash`.

## Commands

### `applet.install`

Install validates the manifest, verifies any supplied signature, compiles the entrypoint, computes `code_hash`, and stores the active applet record transactionally.

Rules:

- A successful first install creates `install_generation = 1`, `version = 1`, and durable state `enabled`.
- The response includes `applet_id`, `install_generation`, `version`, `code_hash`, warnings, and trust metadata.
- Installing the same canonical manifest and same `code_hash` over the active version is an idempotent no-op. It returns the existing version and does not mint a new version.
- Installing a different canonical payload while an active version exists is an upgrade operation and must go through `applet.upgrade`, so the atomicity and compatibility checks are explicit.
- Installing after uninstall creates a fresh install generation. Retained data from a prior keep-data uninstall is not treated as an active applet.

### `applet.enable`

`applet.enable` is the state transition that makes a suspended applet dispatchable again. The command catalog should add it before lifecycle wiring is complete.

Rules:

- `suspended -> enabled` succeeds.
- `enabled -> enabled` is idempotent.
- `uninstalled -> enabled` is rejected with a typed lifecycle error.

### `runtime.run`

`runtime.run` is allowed only when the active applet state is `enabled`.

Rules:

- The run loads the active version at start time and records its `code_hash` and manifest snapshot.
- Once started, the run is pinned to that version. A later suspend or upgrade must not change the in-flight run.
- The recorded run is replayed from its per-run program pin first, then the content-addressed `program/<code_hash>` fallback, and never from a later active version unless that legacy fallback is the only matching artifact.
- Running a suspended or uninstalled applet is rejected before user code starts and emits no host calls or UI patches.

### `ui.dispatch_event`

UI event dispatch uses the same lifecycle gate as `runtime.run`.

Rules:

- Dispatch is allowed only for `enabled` applets.
- A suspended applet rejects with `ui.applet_not_dispatchable`, matching the T034 vector.
- Rejection happens before handler lookup or applet code execution.
- A rejection leaves applet state, persisted data, and the current UI tree unchanged.

### `applet.suspend`

Suspend prevents new runs and event dispatch while retaining the active version and data.

Rules:

- `enabled -> suspended` succeeds.
- `suspended -> suspended` is idempotent and returns `changed: false`.
- Existing in-flight runs keep their recorded version and may finish.
- New `runtime.run` and `ui.dispatch_event` requests are rejected before applet code executes.

### `applet.upgrade`

Upgrade installs a new version over an active applet.

Rules:

- Upgrade stages manifest validation, signature verification, source compilation, policy checks, and schema additions before switching the active pointer.
- The active pointer changes from version N to N+1 only after all staged work commits.
- Failure at any stage rolls back the whole upgrade: active version, lifecycle state, records, schema registry, indexes, replay pins, and audit state remain as they were before the command.
- Prior versions are retained for audit and replay.
- In-flight and previously recorded runs remain pinned to the version and `code_hash` they started with.
- Upgrade does not implicitly resume a suspended applet. If the applet was suspended before a successful upgrade, the new active version remains suspended.

### `applet.uninstall`

Uninstall removes the active applet and requires a retention policy:

- `keep_data`: remove the active applet record, retain applet-owned records/storage, retain run records, and keep replay artifacts needed for audit/replay.
- `purge_data`: remove the active applet record and tombstone applet-owned records/storage with an uninstall purge reason. Run records and replay artifacts remain as audit evidence unless a later privileged hard-purge policy deletes them.

Rules:

- After uninstall, `runtime.run`, `ui.dispatch_event`, `applet.suspend`, `applet.enable`, and `applet.upgrade` are rejected for that `applet_id` until a fresh install succeeds.
- Uninstall is atomic: active applet removal and any data retention or tombstone work commit together.
- Reinstalling after uninstall creates a new `install_generation` and starts at `version = 1`.

## Lifecycle Errors

Lifecycle rejections are typed so shells and conformance tests can distinguish them from manifest or runtime failures.

Recommended codes:

- `lifecycle.applet_not_installed`: no active applet exists.
- `lifecycle.applet_suspended`: command requires an enabled applet.
- `ui.applet_not_dispatchable`: UI event dispatch reached a non-dispatchable lifecycle state.
- `lifecycle.invalid_transition`: command is not legal for the current state.

Lifecycle errors are fail-closed: the command must not start user code, mutate records, or emit UI patches.

## Events

Lifecycle commands emit audit events after commit:

- `applet.installed`
- `applet.enabled`
- `applet.suspended`
- `applet.upgraded`
- `applet.uninstalled`
- `runtime.run.rejected`
- `ui.dispatch_event.rejected`

Event payloads include `applet_id`, `install_generation`, `version` where applicable, `code_hash` where applicable, and the lifecycle state before/after the command.

## Fixture Suite

The semantic vectors live in `forge/fixtures/lifecycle/`. They pin:

- install creates an enabled v1 applet;
- enable allows run and event dispatch;
- suspend rejects event dispatch before handler execution;
- re-enable resumes dispatch;
- upgrade success retains v1 and activates v2 atomically;
- upgrade failure rolls back to v1;
- recorded runs replay against their own `code_hash` after upgrade;
- uninstall keep-data retains applet-owned records;
- uninstall purge-data tombstones applet-owned records;
- uninstalled applets reject illegal run transitions;
- reinstalling the same `code_hash` is an idempotent no-op;
- suspending an already suspended applet is idempotent;
- uninstall then install creates a fresh generation.

## Result

Pinned CR-7 lifecycle semantics:

- successful install defaults to `enabled`;
- `runtime.run` and `ui.dispatch_event` are allowed only for enabled applets;
- suspended applets reject dispatch before handler execution and leave state/tree unchanged;
- upgrade is atomic and rolls back all staged work on failure;
- prior versions and per-run program pins are retained so recorded runs replay against their own `code_hash`, not the new active version;
- uninstall supports `keep_data` and `purge_data`, with purge represented as tombstoned applet-owned records;
- same-payload reinstall is idempotent and does not create a new version.

## Legacy webapp package registry (shell `platform.sqlite`)

Distinct from v1 workspace applets above. Native shells still maintain a file-based
webapp registry (`apps` / `app_versions` / `app_installations`) until a full
`applet.install` cutover. Status and trust tokens for that registry are defined in
`forge-domain` as `PackageAppStatus`, `PackageVersionStatus`, and `TrustLevel`, and
exported to `forge/data/app-status-enums.json` and `forge/data/trust-levels.json`.
Lifecycle commands for that registry use the `package.*` namespace (see
`forge/spec/commands.md`), not `applet.*`.
