# Visual Intake Desktop Proof — Session Archive

Archived: 2026-07-28 (Asia/Bangkok)

## Outcome

This session implemented and exercised the smallest real macOS Visual Intake
proof:

- a built-in `visual-intake` app;
- trusted-host PhotoKit access and change observation;
- local Apple Vision classification, OCR, and face detection;
- bounded recommendations for Health, Invoice, and Search Notes;
- a native bridge limited to the selected app and its declared `photos`
  browser permission;
- explicit status showing that no external model was invoked and no route was
  executed.

The proof intentionally keeps analyzed results in memory. It does not yet add
the durable `intake` capability, blob-CAS ownership, `common.receive` delivery,
saved routing rules, relaunch catch-up, or repeated-pattern App Builder
recommendations described by the preceding plan files.

## Live acceptance receipt

A freshly built macOS app was signed with the available Apple Development
identity, Hardened Runtime, the Photos Library entitlement, and a unique proof
bundle identifier. It was launched through LaunchServices so macOS attributed
PhotoKit access to the application bundle.

The real Visual Intake UI then:

1. loaded the latest Photos image locally;
2. classified it as a screenshot with 99% confidence;
3. recommended Search Notes;
4. marked the result review-required;
5. reported that no external model or route execution occurred;
6. entered the live watching state; and
7. stopped watching cleanly.

No asset was added to, edited in, or removed from the user's Photos library.
The proof application was stopped after validation.

## Automated validation

- Full `TerraneHostE2ETests` macOS suite passed: 33 tests.
- Five focused Visual Intake routing, sensitivity, packaging, permission, and
  privacy-boundary tests passed.
- The normal macOS application build succeeded.
- The signed proof passed strict `codesign` verification with Hardened Runtime
  and the expected Photos entitlement.
- `git diff --check` passed for the Visual Intake implementation and plan.

The existing macOS linker warnings about XCTest's deployment target and the
existing `WKProcessPool` deprecation warning were unchanged by this work.

## Main promotion receipt

The bounded Visual Intake commit was promoted onto the then-current clean
`main` head, `8e88952a` (`feat(spending): add invoice accounting app`). The
feature branch itself was not merged because its older release history had
diverged from `main`.

Conflict resolution retained the existing native Photos picker and its
frameworks alongside Visual Intake, and updated the packaging assertion to the
17 checked-in app manifests present after promotion. The combined XCTest
bundle then passed 36 tests with zero failures, including the existing native
picker tests and all five Visual Intake tests.

## Files owned by this slice

- `apps/visual-intake/`
- `host/macos/Sources/VisualIntakePhotoService.swift`
- Visual Intake hooks in `AppDelegate.swift` and `TerraneBridge.swift`
- Photos/Vision build configuration and the Photos Library entitlement
- `host/macos/Tests/VisualIntakeTests.swift`
- the built-in app count update in `AppPackagingTests.swift`
- `docs/visual-intake-router/`

## Continuation point

The next opt-in live test is to add a synthetic image to Photos while Terrane is
watching and prove that the PhotoKit change observer delivers it. That test
modifies the user's Photos library and was deliberately left for explicit
authorization.

After that receipt, implement the durable slices in
`06-implementation-and-qa.md`: intake capability, blob storage, Visual Inbox,
approved app delivery, relaunch catch-up, corrections, and saved rules. The
iOS station should preserve the contracts in `07-mobile-handoff.md` rather than
copy this macOS host adapter.
