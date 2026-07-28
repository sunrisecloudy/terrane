# Step 6 — Implementation sequence and QA

## Build strategy

Land small vertical slices. Each slice must preserve unrelated worktree changes
and end with focused tests before workspace-wide validation.

## Slice 1 — Contract skeleton

Deliver:

- `rust/crates/terrane-cap-intake/`;
- namespace, manifest, docs, state, event payloads, decide/fold/describe;
- source, item, rule, and pattern queries;
- core state wiring and default-registry registration;
- explicit public-command refusal;
- replay and lifecycle tests.

Expected files:

```text
Cargo.toml
rust/crates/terrane-cap-intake/
rust/crates/terrane-core/src/lib.rs
rust/crates/terrane-core/tests/cap/intake.rs
rust/crates/terrane-host/src/public_authz.rs
rust/crates/terrane-host/tests/public_authz.rs
docs/APP_API.md
```

Gate:

```sh
scripts/with-cargo-cache.sh cargo test -p terrane-cap-intake
scripts/with-cargo-cache.sh cargo test -p terrane-core --test cap intake
scripts/with-cargo-cache.sh cargo test -p terrane-host public_authz
```

## Slice 2 — Built-in Visual Intake app

Deliver:

- `apps/visual-intake/` bundle;
- Visual Inbox list/detail UI;
- source status and enable/pause controls;
- pending, suggested, routed, retained, ignored, and failed states;
- item actions wired to the intake resource;
- English strings in the app i18n bundle;
- packaging and common-API validation.

The app uses standard Terrane APIs. It does not call PhotoKit or read the
host-only source index.

Gate:

- bundle validation;
- `common.receive`, `common.list`, and `common.get` probes;
- macOS packaging tests;
- visual review at light and dark appearance;
- keyboard navigation and VoiceOver labels for route controls.

## Slice 3 — macOS Photos source

Deliver:

- `NSPhotoLibraryUsageDescription` in source and generated app configuration;
- Visual Intake PhotoKit service;
- host-only source-index sidecar;
- enable, baseline, observer, pause, revoke, and catch-up flows;
- Swift-to-Rust trusted intake bridge;
- preview CAS write and ingest dispatch;
- fake-source tests.

Expected macOS files:

```text
host/macos/Sources/AppDelegate.swift
host/macos/Sources/Info.plist
host/macos/Sources/VisualIntake/
host/macos/Tests/VisualIntakePhotoTests.swift
host/macos/Tests/AppPackagingTests.swift
```

Gate:

- baseline creates zero intake items for existing assets;
- one new asset creates exactly one item;
- relaunch catch-up imports a missed asset exactly once;
- revoked permission pauses without retry spam;
- no main-thread image decode.

## Slice 4 — Local classification

Deliver:

- Apple Vision adapter;
- deterministic evidence-to-intent mapper;
- OCR redaction;
- sensitivity policy;
- recorded classifier-version metadata;
- fixture corpus and accuracy/performance receipt.

Gate:

- no external network or model process during classification test;
- OCR body absent from event bytes, logs, and diagnostics;
- fixture-corpus top-3 recall reported;
- automatic routing remains disabled.

## Slice 5 — Suggestions and manual routing

Deliver:

- interface registry;
- candidate discovery through `interop.apps`;
- target blob linking;
- `common.receive("blob", payload)` call;
- route acknowledgment and failure handling;
- correction recording;
- minimal generic destination fixture.

Gate:

- zero/one/multiple target behavior;
- target namespace isolation;
- idempotent retry;
- replay does not redeliver;
- UI explains recommendation evidence.

## Slice 6 — Health vertical path

Deliver:

- `image.food.v1` Health interface;
- Health blob intake;
- draft meal preview;
- explicit analysis action;
- retained manual upload path;
- separate `auto-deliver` and `auto-analyze` permissions.

Gate:

1. Add a real food photo to Photos.
2. Observe local classification and Health recommendation.
3. Approve the route.
4. Verify the exact blob hash reaches Health.
5. Verify no external model ran before `Analyze`.
6. Run a real attached-image vision estimate after explicit analysis.
7. Review, edit, and save the result.

A text-only mock or direct invocation bypassing Photos is not acceptance.

## Slice 7 — Rules

Deliver:

- rule editor and confirmation;
- deterministic rule matching;
- sensitivity exclusion;
- target/interface/grant invalidation;
- audit explanation;
- disable/delete controls.

Roll out in two steps:

1. **shadow mode:** show `A saved rule would have routed this to Health`;
2. **active mode:** enable auto-delivery only after the user activates the rule
   following successful shadow observations.

Gate:

- at least five shadow decisions match user approval;
- no sensitivity-blocked fixture auto-routes;
- rule invalidates immediately after app/interface/grant removal.

## Slice 8 — Repeated-pattern and App Builder handoff

Deliver:

- bounded pattern summaries;
- thresholds and cooldowns;
- recommendation UI;
- editable App Builder specification;
- explicit example selection;
- dismissal and `do not suggest again` behavior.

Gate:

- no recommendation before the threshold;
- only derived metadata participates in clustering/counting;
- no app generation starts before user confirmation;
- no unselected image is attached to the builder request.

## Slice 9 — Retention and operational hardening

Deliver:

- preview/source cleanup policies;
- source-index tombstones;
- pause/resume/reset baseline;
- diagnostics and status counters;
- iCloud-offline recovery;
- corrupt/oversized input handling;
- migration/version tests;
- user-facing `Delete all Visual Intake data`.

Gate:

- cleanup changes Terrane only, never Photos;
- deleted previews remain explainable from summary events;
- catch-up does not reimport tombstoned assets unexpectedly;
- full data deletion removes sidecar identifiers, intake blobs, rules, and
  folded active state through recorded events.

## Full validation

Rust:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
```

macOS:

- run the canonical Swift/macOS test command documented by the host;
- produce a fresh arm64 development build;
- verify source and generated `Info.plist` both contain the Photos usage string;
- validate light/dark UI and accessibility;
- exercise a real Photos addition, relaunch catch-up, route, revoke, and cleanup.

## Rollout stages

| Stage | Behavior |
| --- | --- |
| Developer preview | Manual refresh, local classification, no auto-route |
| Internal preview | Live observer + catch-up, suggestions, Health manual route |
| User preview | Saved rules in shadow mode, retention controls |
| Stable desktop | Active user-approved rules, pattern recommendations |
| Later | Background helper decision and mobile adapters |

## Stop conditions

Do not advance rollout if any of these remain:

- an external provider receives images during ingestion/classification;
- baseline imports historical assets without an explicit backfill;
- replay reopens Photos, reruns Vision, or repeats routing;
- raw Photos identifiers or OCR body enter the event log;
- a target can read another app's blob namespace;
- Health analyzes automatically from an ordinary route approval;
- GUI acceptance was replaced by unit tests or direct backend invocation;
- full workspace test/clippy failures caused by the change are unresolved.
