# Step 7 — Mobile handoff boundary

## Scope

This file preserves the desktop decisions that a later iOS implementation
should reuse. It does not authorize or plan immediate iOS code changes in this
repository.

## Portable contract

Keep these concepts platform-neutral:

- `intake` event and state schemas;
- source ids and hashed source keys;
- intent taxonomy and classifier result schema;
- Visual Inbox item states;
- routing rules and sensitivity gates;
- image interface names;
- `common.receive("blob", payload)` destination contract;
- App Builder pattern summary;
- external-processing approval semantics.

Platform adapters translate OS inputs into the same trusted intake command.

```text
macOS Photos adapter ─┐
iOS source adapter   ─┼─> intake.image.ingest ─> classifier ─> router ─> apps
Share extension      ─┘
```

## What must not enter the shared contract

- `PHAsset.localIdentifier`;
- macOS paths;
- `NSImage`, `UIImage`, or platform image classes;
- Photos authorization enum raw values;
- AppKit or SwiftUI presentation state;
- assumptions that an observer remains alive in the background;
- platform-specific iCloud error objects.

Adapters convert these into stable source status, error codes, hashes, dates,
dimensions, MIME types, and recorded edge results.

## Likely iOS source options

A later iOS project should evaluate separately:

- foreground PhotoKit observation;
- Share extension or Photos share-sheet entry;
- explicit capture-to-Terrane flow;
- selected album monitoring;
- background delivery limits;
- app-group storage and secure handoff to the main app.

Do not promise continuous background ingestion on iOS until it is proven
against current platform lifecycle rules on real devices.

## Desktop responsibilities before mobile starts

Freeze and fixture-test:

1. intake event schema v1;
2. destination blob payload v1;
3. `visual-intent-v1`;
4. rule schema v1;
5. source adapter conformance tests;
6. idempotency behavior;
7. external-processing policy;
8. source-status/error-code registry.

Export representative, synthetic conformance fixtures that the iOS adapter can
consume without access to desktop Photos or personal images.

## Cross-device identity

Do not assume the same Photos asset has the same local identifier on Mac and
iPhone. Cross-device deduplication should use verified content hashes after
materialization, with source-key hashes used only for per-device ingestion
idempotency.

If both devices ingest the same image before sync, the shared CAS hash may
deduplicate bytes later, but the product must decide whether to merge or show
two source observations. That decision is deferred until real mobile sync
behavior exists.

## Mobile start gate

Begin the mobile adapter only after desktop proves:

- real new-photo observation;
- catch-up and idempotency;
- local classification;
- manual interface-based routing;
- Photos permission revocation;
- retention cleanup;
- replay identity;
- one real Health attached-image workflow.

The mobile project can proceed independently on shell and source-adapter
scaffolding, but it should not invent a competing event, interface, or routing
contract.
