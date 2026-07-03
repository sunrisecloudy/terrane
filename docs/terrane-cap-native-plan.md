# Terrane Native Capability Plan

Status: planning, updated after `review-025-cap-native-plan.md`.

This plan maps the common, desktop, and mobile capability groups used by
Electron, Tauri, React Native, Compose Multiplatform, and Ionic/Capacitor onto
Terrane's capability model.

The target crate is `rust/crates/terrane-cap-native/`, with capability namespace
`native`.

## Goals

- Provide one Terrane-native capability namespace for app/native bridge work that
  is common across desktop and mobile hosts.
- Keep the app-facing interface stable while allowing each OS host to implement
  its own connector layer.
- Represent native work as replay-safe recorded facts: replay folds recorded
  native request/result events and never repeats an OS action.
- Make common, desktop, and mobile support discoverable through `cap info`,
  MCP `capability_info`, and generated app resource docs.
- Avoid duplicating existing Terrane capabilities. `kv` remains local app data,
  `net` remains recorded HTTP, `build` remains compiler helpers, and packaging
  or signing stays in host/release tooling unless it needs a runtime app API.

## Non-goals

- No ambient OS access from app JavaScript or WASM.
- No native I/O inside `fold`, `query`, or `read_resource`.
- No one free-form `native.request(operation, payload)` public command that can
  smuggle dangerous operations around policy.
- No auto-drain-after-invoke path that quietly turns an app invoke into a
  blocking native effect.
- No large binary media payloads in the event log.
- No claim that Windows, Linux, iOS, or Android behavior is live-verified from a
  macOS-only development run.
- No automatic promotion of every framework feature into a Terrane operation.
  Release tooling and host chrome stay out unless they pass the capability test.

## Source Constraints From Capability Best Practice

- The capability crate owns namespace, manifest, docs, command validation, event
  constructors, fold, and describe.
- OS work is edge work. It belongs behind host connectors, not inside
  deterministic capability code.
- `Decision::Effect` plus `EdgeRunner` is the sanctioned synchronous non-pure
  shape. `native` deliberately uses a queue for app-originated work because
  JS/WASM resource writes may commit records only, and user-mediated OS work can
  block for minutes.
- `ctx.resource` writes inside JS/WASM currently may commit records only. They
  cannot return `Decision::Effect`, and nested effects/runtimes are refused.
- Resource grants are currently namespace-level at runtime through
  `namespace_granted`. A grant for `native` would expose all
  `ctx.resource.native` methods, so the first resource and command surfaces must
  stay small or operation-level selector enforcement must land first.
- Every command must be explicitly classified in `public_authz.rs`; refused
  native operations must be refused before any connector can run.
- If the queue lifecycle becomes a reusable capability pattern, update
  `docs/cap-best-practice/` in the same implementation series.

## Architecture

Use a two-layer design.

### 1. Deterministic capability crate

Path:

```text
rust/crates/terrane-cap-native/
  Cargo.toml
  src/
    lib.rs
    commands.rs
    doc.rs
    events.rs
    operations/
      mod.rs
      common.rs
      desktop.rs
      mobile.rs
    resources.rs
    tests.rs
    types.rs
  tests/
    capability.rs
```

The crate owns:

- namespace `native`;
- an operation catalog with stable operation ids, schemas, support notes, safety
  classes, result-size classes, retention classes, and policy dispositions;
- app-scoped pending/completed/failed native request state;
- executor affinity data copied into every request event;
- event constructors used by host connectors;
- `CapabilityDoc` coverage for common, desktop, and mobile groups;
- `ctx.resource.native` methods that only record requests or read recorded
  results.

### 2. Host native connector layer

Path:

```text
rust/crates/terrane-host/src/native/
  mod.rs
  connector.rs
  requests.rs
  unsupported.rs
  macos/
  windows/
  linux/
  ios/
  android/
```

The connector layer owns OS work:

- selecting the active connector with `cfg(target_os = "...")`;
- checking whether an operation is supported on the current host;
- executing pending requests only when an explicit trusted drain/poll service is
  called;
- returning a recorded completion or failure event through
  `terrane-cap-native` event constructors;
- enforcing timeouts, output limits, path normalization, redaction, and OS
  permission preflights.

Headless `terrane-host` users, including MCP and web hosts, must not pay for GUI
connector dependencies unless the native connector feature is enabled.

Host-shell specific code can live below the concrete host when needed, for
example:

```text
host/macos/Sources/Native/
host/windows/Native/
host/ios/Native/
host/android/app/src/main/.../native/
```

Those folders should call the shared host/native connector contract rather than
define independent app-facing APIs.

## Native Lifecycle Decision

Use an explicit async request queue for app-originated native work:

```text
ctx.resource.native.* write
  -> native.requested
  -> later trusted host drain/poll service
  -> native.completed | native.failed | native.cancelled
  -> later app invoke reads ctx.resource.native.result(requestId)
```

Consequences:

- Results are not available in the same backend run that requested them. App
  authors must generate a `requestId`, return a pending status to the UI, and
  read the result on a later invoke.
- Every completed operation normally records at least two facts:
  `native.requested` plus a terminal event. Keep v1 operations low-frequency and
  small-result.
- The connector drain service is trusted host plumbing, not a public
  `capability_command`. It may be wrapped later by CLI or MCP workflow tools,
  but it dispatches only trusted completion/failure/cancel commands.
- Do not auto-drain on `Core::open` or after every app invoke. Startup drain
  risks executing stale UI prompts, and after-invoke drain reintroduces a
  blocking effect through the side door.
- `Decision::Effect` remains available for a future trusted synchronous command
  path, such as an operator-only CLI command, but it is not the v1 app-facing
  model.

## Capability Shape

`terrane-cap-native` should be a stateful, resource-owning capability backed by
an async native request queue.

State:

```text
NativeState {
  platform: BTreeMap<HostId, NativePlatformObservation>,
  requests: BTreeMap<AppId, BTreeMap<RequestId, NativeRequestRecord>>
}

NativeRequestRecord {
  request_id: RequestId,
  app: AppId,
  operation_id: String,
  status: pending | completed | failed | cancelled,
  executor_host_id: HostId,
  origin_replica: Option<u64>,
  sequence: u64,
  input_json: String,
  result_size_class: ResultSizeClass,
  result_json: Option<String>,
  error_json: Option<String>
}
```

`executor_host_id` comes from a trusted `native.platform.observed` fact. The cap
must refuse app request commands until the current host has recorded an
observation. `origin_replica` should come from the folded `replica.peer` query
when available. Connectors drain only requests whose executor matches the local
host observation.

Events:

- `native.platform.observed`: trusted host recorded platform/support metadata,
  connector version, host id, and supported operation ids. This is the source for
  `native.supports`; queries never probe the live connector.
- `native.requested`: app requested a specific native operation. Payload must
  include request id, app, operation id, executor host id, origin replica,
  deterministic sequence, input JSON, result-size class, and retention class.
- `native.completed`: host connector completed the request and recorded a
  replayable result. Inline result payloads must be bounded and JSON encoded.
- `native.failed`: host connector tried the request and recorded a replayable
  failure, including OS permission denial.
- `native.cancelled`: app or trusted host cancelled a pending request. Stale
  pending cleanup records this event rather than silently dropping state.
- `native.pruned`: optional future housekeeping fact if keep-last-N state
  retention needs explicit audit.

Commands:

- App-callable request commands are explicit by operation, for example
  `native.clipboard.write-text`, `native.external.open-url`,
  `native.notification.show`, and `native.dialog.open-file`.
- Trusted host result commands are explicit and refused publicly:
  `native.complete`, `native.fail`, `native.cancel`,
  `native.platform.observe`, and any future `native.prune`.
- High-risk desktop/mobile operations start trusted-only or omitted from
  resources until operation-level authorization exists.

Queries:

- `native.supports` returns `QueryValue::Bool` for a specific operation id based
  only on folded `native.platform.observed` state. A fresh log with no platform
  observation answers unsupported.
- Richer platform/support details are exposed through state-backed resource reads
  or capability docs, not by extending `QueryValue` in v1.

Resources:

- `ctx.resource.native.result(requestId)` reads a recorded result JSON string or
  a missing/null value if pending, failed, cancelled, or pruned.
- `ctx.resource.native.pending()` reads pending request ids for the app.
- Small, safe write methods can map to explicit commands. Examples:
  `clipboardWriteText(requestId, text)`,
  `externalOpenUrl(requestId, url)`,
  `notificationShow(requestId, title, body)`,
  `dialogOpenFile(requestId, optionsJson)`.

Do not run OS calls in these resource methods. They record `native.requested`;
the trusted host connector drain service records terminal results later.

## Pending, Idempotence, and Retention

Pending requests are durable until a terminal event exists.

- Decide-side: `native.complete`, `native.fail`, and `native.cancel` refuse
  unknown, completed, failed, or cancelled request ids.
- Fold-side: duplicated terminal events never corrupt state. First terminal
  status wins unless a future migration explicitly versions the lifecycle.
- Drain-side: the connector re-checks pending-ness and executor identity
  immediately before executing OS work.
- App removal: folding `app.removed` cancels or drops that app's pending native
  state according to an explicit test. The preferred implementation records
  `native.cancelled` before app removal when the connector sees in-flight work;
  fold still defensively removes the app slice.
- Stale pending: connectors do not execute old pending requests on startup.
  A trusted sweep may record `native.cancelled` with reason `stale` for requests
  the operator/host chooses not to execute.
- V1 state retention: keep all pending records and the last bounded number of
  terminal records per app in folded state. Older terminal records may become
  unavailable through `result(requestId)` even though their events remain in the
  log until a future compaction story exists.

## Result Size and Blob Strategy

The event log may store only bounded replay facts.

Result-size classes:

- `none`: operation has no useful result beyond success/failure.
- `inline-small`: bounded JSON string, suitable for clipboard text, URL-open
  status, notification ids, permission status, and picker path lists.
- `blob-ref`: large media or binary output stored outside the event log, with the
  event recording a stable reference, byte length, media type, and content hash.
- `unsupported-large`: operation is documented but not implemented until a blob
  store and replay story exist.

V1 should implement only `none` and `inline-small`. Screen capture, camera,
media picker, audio recording, and similar operations stay planned until
`blob-ref` storage is designed.

All JSON result strings must be produced with `serde_json` or `nanoserde`, never
hand-built with `format!`.

## Native Operation Catalog

Each operation spec should include:

- stable id, such as `clipboard.writeText` or `dialog.openFile`;
- operation kind: `native-operation`, `existing-cap`, `release-tooling`,
  `host-plumbing`, or `not-v1`;
- group: `common`, `desktop`, or `mobile`;
- safety class: `safe-request`, `user-mediated`, `sensitive`, `admin-only`, or
  `release-only`;
- policy disposition: `grant-gated`, `refuse`, `trusted-only`, or `not-command`;
- app-facing method name if any;
- command name if any;
- input schema id and result schema id;
- result-size class and retention class;
- required Terrane grant verbs;
- likely OS permissions;
- supported platforms;
- connector implementation status.

Suggested initial operation groups:

- Common native v1 candidates: platform observation, `native.supports`,
  clipboard write text, open external URL, local notification, open file dialog,
  pending request reads, and result reads. Implement a small safe subset first
  and mark the rest planned.
- Common existing-cap rows: local storage maps to `kv` and `relational_db`;
  networking maps to `net`; build helpers map to `build`.
- Common planned rows: secure storage, native SDK hooks, OS permission request,
  push notifications, and lifecycle snapshots. These need more policy and host
  design before app-facing release.
- Desktop native-operation candidates: app-controlled tray/menu/status surfaces,
  global shortcuts, drag/drop, rich clipboard, hardware integration, screen
  capture, printing, and power/idle. Most are `sensitive` or `trusted-only` until
  scoped selectors and platform tests exist.
- Desktop terminal non-operations: updater, installer generation, code signing,
  notarization, and release packaging are `release-tooling`, not `native` app
  operations. Window chrome, standard app menu, safe areas, keyboard avoidance,
  dock/taskbar defaults, and host lifecycle are `host-plumbing` unless an app
  needs a recorded, app-controlled operation.
- Mobile native-operation candidates: camera, media picker, microphone,
  geolocation, biometrics, contacts, calendar/reminders, share sheet, in-app
  browser, sensors, haptics, NFC/BLE, maps, SMS/phone/email intents, and mobile
  payments. These remain planned behind iOS/Android connectors until platform
  runners validate them.
- Mobile terminal non-operations: app store packaging, `.ipa`, `.apk`, `.aab`,
  store submission, and signing are `release-tooling`, not runtime capability
  operations.

Existing Terrane caps cover some framework rows:

- local storage: `kv` and `relational_db`;
- networking/HTTP: `net`;
- build helpers: `build`;
- app build/generation: `builder`, `harness`, and host tooling.

## Authorization Policy

Start default-deny.

- Declare `GrantResourceSpec::namespace_v1("native", &["read", "write"], ...)`
  only while the public `ctx.resource.native` and app-callable command surfaces
  are intentionally small.
- Classify each app-callable v1 command as `GrantGated { namespace: "native",
  app_arg_index: 0 }`.
- Classify connector/admin commands as `Refuse`.
- Safety-class mapping:
  - `safe-request` and `user-mediated`: may be `GrantGated` in v1 if the
    operation is in the small surface.
  - `sensitive`: `Refuse` until operation-level selectors exist.
  - `admin-only`: `Refuse`; trusted host path only.
  - `release-only`: not a runtime command.
- Add public-authz tests that prove all registered `native.*` commands are
  classified and no allowlisted/grant-gated command can emit high-risk result
  events outside its documented operation.
- Before exposing broad desktop/mobile operations through app resources, add
  operation-level selector enforcement or split the surface so one `native`
  grant does not imply shell, screen, process, contacts, location, and camera.

There are two permission layers:

- Terrane grant: whether this app may use `native` at all.
- OS permission: whether the host OS/user allows the specific native action.

Both outcomes must be visible in recorded native events and docs.

## Implementation Slices

### Slice 1: Planning and Catalog

- Add the operation catalog in docs or in `terrane-cap-native/src/operations/`
  as pure metadata.
- Classify every attached common/desktop/mobile item as one of:
  `native-operation`, `existing-cap`, `release-tooling`, `host-plumbing`, or
  `not-v1`.
- Pin each operation's safety class, policy disposition, result-size class,
  retention class, and support status.
- Decide the first safe subset. Recommended: platform observation,
  `native.supports`, clipboard write text, open external URL, local
  notification, open file dialog, pending reads, and result reads.
- Update `docs/cap-best-practice/` if the async request queue becomes an
  accepted capability pattern.

### Slice 2: Core Capability Crate

- Add `terrane-cap-native` to the root `Cargo.toml`.
- Add `NativeState` to `terrane-core::State`, both `StateStore` arms, and
  `default_registry()`.
- Implement platform observation, explicit request commands, trusted terminal
  commands, `app.removed` cleanup, `doc()`, and `describe()`.
- Include executor affinity, origin replica, result-size class, and retention
  class in event payloads before any event shape lands.
- Add unit, capability, and engine tests with `replay_matches()` and reopen-log
  replay.
- Test duplicate terminal events, non-pending completion refusal, stale
  cancellation, app removal with pending requests, unsupported fresh-log
  `native.supports`, and keep-last-N state retention.

### Slice 3: Resource Surface and Grants

- Add the small `ctx.resource.native` surface.
- Add `native` to grant inventory and verb tests.
- Regenerate `docs/APP_API.md` intentionally, then keep the bare drift check in
  the verification gate.
- Add JS/WASM runtime tests proving native resources are absent before grant and
  request/result methods appear after grant.
- Document clearly that requested native results are read on a later invoke, not
  the same backend run.

### Slice 4: Host Connector Contract

- Add `NativeConnector` and an `unsupported` connector that records a stable
  `native.failed` result.
- Add an explicit trusted drain/poll service. Do not auto-drain on open or after
  app invoke.
- Add host tests with a fake connector that drains one pending request and
  records exactly one completion.
- Ensure connector drain is idempotent: already completed/cancelled requests do
  not execute twice.
- Feature-gate GUI-touching connector dependencies for headless host builds.

### Slice 5: Binary E2E

- Add `rust/crates/terrane-host/tests/cap/native.rs` and wire it into the cap
  e2e test module.
- Default-run validation/refusal tests through the real binary.
- Mark real OS UI tests ignored with a reason, following the `net` and `model`
  effect-test convention.

### Slice 6: macOS MVP

- Implement the macOS connector for the chosen safe subset.
- Keep Swift/AppKit code under `host/macos/Sources/Native/` only when Rust host
  APIs cannot provide the behavior.
- Add focused macOS tests where available and label broader platform behavior as
  OS-gated.

### Slice 7: MCP, CLI, and Public Contract

- Keep raw generic `capability_command` safe through `public_authz.rs`.
- Consider a higher-level `native_request`, `native_drain`, or `native_result`
  MCP workflow only if raw capability help is not enough for agents.
- Update `docs/SERVER_API.md`, `host/mcp/docs/CAPABILITY_OPERATIONS.md`,
  `host/mcp/docs/SECURITY.md`, and `host/mcp/docs/AGENT_PLAYBOOK.md`.
- Run contract export/verification when MCP, resources, or docs change.

### Slice 8: Desktop and Mobile Expansion

- Add one OS connector folder at a time: `windows/`, `linux/`, `ios/`,
  `android/`.
- Prefer fake connector conformance tests in Rust plus platform-gated live tests
  on actual OS runners.
- Promote each operation from planned to supported only after docs, policy, and
  platform tests agree.
- Treat Windows as the next likely conformance target after macOS because the
  desktop connector semantics and CLI host are closest. Android and iOS need
  dedicated host adapter work first.

## Verification Gate

From the repo root:

```sh
cargo test --workspace --locked
cargo test -p terrane-cap-native
cargo test -p terrane-core --test cap native
cargo test -p terrane-core --test cap grant_spec_inventory
cargo test -p terrane-core --test cap grant_verbs_match_specs
cargo test -p terrane-host --test cap native
cargo test -p terrane-host --test public_authz
cargo test -p terrane-host --test contract
cargo test -p terrane-core --test cap app_api_doc
cargo clippy --workspace --all-targets --locked -- -D warnings
```

When resource docs intentionally change:

```sh
UPDATE_DOCS=1 cargo test -p terrane-core --test cap app_api_doc
```

When `host/macos` changes:

```sh
cd host/macos && swift test
```

For non-macOS connectors, do not claim live support until the matching platform
runner has executed the connector tests.

## Remaining Open Decisions

- The exact operation-level selector model for `sensitive` native operations.
- The blob store and replay-reference design for `blob-ref` results.
- The state-retention bound for v1 terminal records per app.
- The concrete Windows connector test runner and packaging expectations.
