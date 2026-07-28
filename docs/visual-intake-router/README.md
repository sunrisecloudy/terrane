# Visual Intake Router

Status: desktop developer proof implemented and validated; durable intake,
delivery, and automatic-routing slices remain planned.

This directory defines the complete path for making new Apple Photos images a
first-class input stream to Terrane on macOS. The platform imports, classifies,
and recommends destinations for images; specialized apps receive accepted
items through the existing interop and `common.receive` contracts.

The first delivery target is the main macOS desktop app. The event and adapter
boundaries are intentionally portable, but iOS implementation is out of scope
for this plan.

## Product outcome

After a user explicitly enables Visual Intake:

1. Terrane establishes a baseline and considers every newly added Photos image.
2. A local-only classifier produces bounded routing evidence.
3. Terrane shows the image in a Visual Inbox with a recommended destination.
4. The user can approve, correct, ignore, or create a reusable routing rule.
5. An approved destination receives a blob through `common.receive`.
6. Repeated unresolved patterns can produce an App Builder recommendation.

Importing an image does not automatically invoke an external model. Health,
Invoice, or another destination performs its own analysis only under a
separately approved policy.

## Architectural decisions

- **First-class intake capability:** durable routing facts live in a new
  `intake` capability rather than Health-specific state.
- **Host-owned source adapter:** PhotoKit access exists only in the trusted
  macOS host. App JavaScript never receives ambient Photos access.
- **Built-in Visual Intake app:** `apps/visual-intake` owns preview and retained
  source blobs and provides the Visual Inbox UI.
- **Existing delivery contract:** routing uses app manifest interfaces,
  `interop`, and `common.receive("blob", payload)`.
- **Local classification first:** Apple Vision metadata, image classification,
  and OCR run on the Mac. External multimodal providers are never a prerequisite
  for intake or initial routing.
- **Progressive trust:** suggestions precede automatic routing. A route becomes
  automatic only after the user explicitly saves a rule.
- **No historical surprise:** enabling the source establishes a baseline.
  Existing Photos assets are not imported unless the user starts an explicit
  bounded backfill.
- **No always-running agent in v1:** Terrane observes while open and catches up
  on its next launch. A login item or LaunchAgent is a later decision.
- **Pixels stay out of the event log:** event payloads contain hashes, bounded
  metadata, classification evidence, and decisions. Bytes remain in the blob
  CAS.

These are proposed defaults for implementation, not claims that code has
shipped. Before Slice 1 begins, confirm or amend:

1. all new Photos images after enablement are the primary desktop source;
2. Apple Vision is the required local v1 classifier;
3. a first-class `intake` capability plus built-in Visual Intake app is the
   ownership boundary;
4. external model processing remains separately approved;
5. v1 catches up on relaunch instead of installing a background helper.

## Plan order

| Step | Plan | Deliverable |
| --- | --- | --- |
| 0 | [00-product-scope.md](00-product-scope.md) | User journey, states, and acceptance boundaries |
| 1 | [01-intake-contract.md](01-intake-contract.md) | Capability surface, events, state, replay, and blob ownership |
| 2 | [02-macos-photos-ingress.md](02-macos-photos-ingress.md) | PhotoKit authorization, baseline, observation, and catch-up |
| 3 | [03-local-classification.md](03-local-classification.md) | Local evidence schema, classifiers, sensitivity, and confidence |
| 4 | [04-routing-and-apps.md](04-routing-and-apps.md) | Interface discovery, app delivery, corrections, and pattern suggestions |
| 5 | [05-privacy-security-retention.md](05-privacy-security-retention.md) | Permission layering, threat model, external-model gate, and cleanup |
| 6 | [06-implementation-and-qa.md](06-implementation-and-qa.md) | Build slices, file map, tests, GUI acceptance, and rollout |
| 7 | [07-mobile-handoff.md](07-mobile-handoff.md) | Contract preserved for later iOS work; no mobile implementation |
| 8 | [08-session-archive.md](08-session-archive.md) | Implemented proof, validation receipt, boundaries, and continuation point |

Each step must land green before the next step depends on it. Use the repository
Cargo cache wrapper for Rust validation:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Existing foundations

This plan builds on shipped Terrane surfaces instead of duplicating them:

- `blob`: content-addressed bytes with metadata-only events.
- `media`: bounded image inspection and deterministic recorded transforms.
- `interop`: host-mediated app calls and interface discovery.
- `common.receive`: required inbound verb for every app.
- `model` blob parts: destination apps may analyze a routed blob after their
  own permission and policy checks.
- App Builder: user-invoked generation path for a proposed new app.

Relevant contracts:

- [cap-blob.md](../../plan-completed-cap/cap-blob.md)
- [cap-media.md](../../plan-completed-cap/cap-media.md)
- [cap-interop.md](../../plan-completed-cap/cap-interop.md)
- [cap-model-v2.md](../../plan-completed-cap/cap-model-v2.md)
- [Terrane capability best practice](../cap-best-practice/README.md)

## Definition of complete

Desktop Visual Intake is complete only when a fresh signed development build
passes this real workflow:

1. Enable Visual Intake and approve macOS Photos access.
2. Establish a baseline without importing existing assets.
3. Add a new food photo and a new receipt-like image to Photos.
4. Observe both items in Visual Inbox after live change delivery.
5. Quit Terrane, add another image, relaunch, and observe catch-up.
6. Approve the food recommendation and prove Health receives the correct blob.
7. Correct the receipt destination and prove the correction affects a later
   recommendation without silently creating an automatic rule.
8. Revoke Photos permission and prove intake pauses without data loss, retry
   loops, or a crash.
9. Prove no image is sent to an external model during ingestion or local
   classification.
10. Replay the Terrane log and obtain identical intake state without reopening
    Photos, rerunning Vision, or redelivering an app call.
