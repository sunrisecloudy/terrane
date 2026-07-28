# Step 2 — macOS Photos ingress

## Goal

Implement a trusted macOS adapter that considers every qualifying new Photos
image while Terrane is running and catches up after relaunch.

Apple references:

- [PhotoKit](https://developer.apple.com/documentation/photokit)
- [PHPhotoLibraryChangeObserver](https://developer.apple.com/documentation/photos/phphotolibrarychangeobserver)
- [PHImageManager image data](https://developer.apple.com/documentation/photos/phimagemanager/requestimagedataandorientation%28for%3Aoptions%3Aresulthandler%3A%29)
- [NSPhotoLibraryUsageDescription](https://developer.apple.com/documentation/bundleresources/information-property-list/nsphotolibraryusagedescription)

## Host module

Add:

```text
host/macos/Sources/VisualIntake/
  PhotoLibraryIntakeService.swift
  PhotoLibraryAuthorization.swift
  PhotoLibrarySourceIndex.swift
  PhotoAssetLoader.swift
  PhotoAssetNormalizer.swift
  VisualIntakeStatus.swift
```

`AppDelegate` owns the service lifecycle. The service receives a narrow bridge
that can:

- dispatch trusted `intake` commands;
- write/link verified CAS bytes through Rust host FFI;
- query folded intake state;
- publish status to the Visual Inbox UI.

Swift does not mutate the Terrane log or SQLite CAS directly.

## Authorization

Add a specific usage string to the macOS app configuration and generated
`Info.plist`, for example:

> Terrane watches new photos you add so it can privately recommend apps and
> workflows. Images stay on this Mac unless you approve another action.

Requirements:

- Request Photos read authorization only after the user enables Visual Intake.
- Show the Terrane explanation before the macOS prompt.
- Treat `.authorized` and `.limited` distinctly.
- Treat `.denied` and `.restricted` as paused states, not errors to retry.
- Link directly to the relevant System Settings page when permission has been
  denied and the OS allows it.
- Never infer authorization from the picker-only Photos integration.

Full-library access supports the requested all-new-images stream. Limited
access is accepted but the UI must explain that newly added assets may not be
visible without additional selection.

## Baseline

On first enable:

1. Fetch qualifying images for the configured scope.
2. Insert their hashed source keys into the host-only source index with status
   `baseline`.
3. Record `intake.source-configured` with `baseline_at_ms`.
4. Do not request image bytes and do not create intake items.
5. Register the PhotoKit change observer using the retained fetch result.

The baseline operation is cancellable and reports progress for large
libraries. It must use bounded batches and must not block the main thread.

## Qualifying assets

Desktop v1 includes still image assets and the still representation of a Live
Photo. It excludes video and audio.

Metadata recorded before byte retrieval:

- hashed source key;
- asset added date and creation date when present;
- width and height;
- media subtype flags such as screenshot and Live Photo;
- source scope identifier hash.

Do not read or record:

- location;
- face/person data;
- album titles in events;
- original filename unless a target later requires and the user approves it.

## Observation

`PhotoLibraryIntakeService` adopts `PHPhotoLibraryChangeObserver` and holds the
fetch result for the configured source.

On change:

1. Obtain `PHFetchResultChangeDetails`.
2. Replace the retained fetch result with `fetchResultAfterChanges`.
3. Collect inserted assets only.
4. Serialize processing through a bounded actor/queue.
5. Hash each local identifier with a host-local source salt.
6. Skip keys already present in the source index.
7. Load a bounded preview.
8. Dispatch trusted ingest and classification work.
9. Mark the source index only after the durable ingest event succeeds.

Change callbacks may arrive off the main thread. UI updates return to the main
actor; PhotoKit and image processing remain off it.

## Catch-up after relaunch

The v1 catch-up algorithm favors clarity over relying on one OS-specific
persistent token:

1. Open the source index.
2. Fetch assets added after the last successful observation with a conservative
   overlap window.
3. Hash and compare source keys.
4. Ingest only missing keys.
5. Update the successful observation marker after the batch commits.

The overlap plus source-key idempotency handles clock skew and interrupted
shutdown. A later implementation may adopt PhotoKit persistent change tokens,
but changing cursor implementation must not change the intake event contract.

## Preview loading

Use `PHImageManager` with:

- network access allowed only when the user has enabled iCloud retrieval;
- a bounded target representation suitable for 768 px classification;
- orientation applied;
- current rendered edits rather than silently discarding the user's Photos
  adjustments;
- cancellation when intake is paused or Terrane terminates.

If the asset is iCloud-only and network retrieval is disabled, record a
retryable `source-unavailable` status. Do not busy-loop.

## Source materialization

When routing or retention requires more than the preview:

1. Resolve the local identifier through the host-only source index.
2. Verify that the asset still exists and authorization still permits access.
3. Request the largest bounded rendered representation required by the target.
4. Normalize unsupported formats to JPEG or PNG at the edge.
5. Strip location and unnecessary EXIF metadata.
6. Write the bytes to CAS and record the resulting hash, size, MIME, dimensions,
   transform description, and source preview hash.

The edge records the output hash. Replay never reruns the encoder.

## Failure behavior

| Failure | Behavior |
| --- | --- |
| Permission revoked | Pause source; preserve inbox and rules |
| Asset deleted before load | Mark item/source unavailable; no crash |
| iCloud offline | Retryable status with backoff |
| Unsupported/corrupt image | Terminal ingest failure with bounded reason |
| Oversized/decompression bomb | Reject before full decode |
| CAS write failure | Do not mark source index complete |
| Core dispatch failure | Retry from source index reconciliation |
| Observer invalidated | Re-fetch, reconcile, and re-register |

## No background daemon in v1

PhotoKit observation is active only while Terrane is running. Catch-up makes the
stream complete across ordinary app restarts. Do not add a login item,
LaunchAgent, or privileged helper in this phase.

## Tests

- Unit tests for authorization-state mapping and source-key hashing.
- Sidecar migration, uniqueness, interruption, and corruption tests.
- Fake PhotoKit adapter tests for baseline, insertion, deletion-before-load,
  and overlapping catch-up.
- Fixture tests for HEIC, JPEG, PNG, screenshot subtype, rotated image, edited
  image, and Live Photo still.
- Real manual test with iCloud-only asset and network enabled/disabled.
- GUI test proving the permission explanation appears before the OS prompt.
- Relaunch test proving a photo added while Terrane was closed is ingested once.
