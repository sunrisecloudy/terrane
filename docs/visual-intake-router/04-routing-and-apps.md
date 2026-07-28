# Step 4 — Routing, destination apps, and repeated patterns

## Goal

Match classified images to explicit app capabilities, deliver approved blobs
through existing Terrane contracts, learn corrections safely, and recommend a
new app when no installed app fits a repeated need.

## Image interfaces

Apps advertise exact, versioned interfaces:

```json
{
  "interfaces": [
    "items",
    "image.food.v1"
  ]
}
```

Initial interface registry:

| Interface | Expected handler |
| --- | --- |
| `image.food.v1` | Food/nutrition intake |
| `image.invoice.v1` | Invoice extraction and review |
| `image.receipt.v1` | Receipt/expense intake |
| `image.document.v1` | General document intake |
| `image.whiteboard.v1` | Whiteboard/note extraction |
| `image.screenshot.v1` | Screenshot organization or analysis |
| `image.product.v1` | Product/catalog intake |
| `image.generic.v1` | Generic image inbox |

The registry belongs in `docs/APP_API.md` when implemented. Interface names
declare compatibility; app names and descriptions are not authoritative.

## Destination contract

The router invokes:

```text
common.receive("blob", payloadJson)
```

Payload v1:

```json
{
  "schema": 1,
  "name": "inbox/visual/<item-id>.jpg",
  "hash": "<sha256>",
  "size": 123456,
  "mime": "image/jpeg",
  "width": 1600,
  "height": 1200,
  "intakeItem": "<item-id>",
  "source": "photos",
  "capturedAtMs": 0,
  "intents": [
    { "name": "food", "confidence": 0.94 }
  ],
  "externalProcessingApproved": false
}
```

The blob name is already linked into the target app's namespace before the
call. The target does not receive the Photos identifier, location, OCR body,
or access to the Visual Intake app's blob namespace.

The target reply is bounded JSON:

```json
{
  "ok": true,
  "accepted": true,
  "item": "terrane://health/item/estimate-draft-42",
  "status": "queued"
}
```

The router records the reply through the existing interop effect. A malformed
or negative reply becomes `intake.route-failed`; it is not treated as success.

## Health integration

Health is the first real destination:

1. Add `image.food.v1` to `apps/health/manifest.json`.
2. Extend Health's `common.receive` implementation to validate a target-owned
   image blob payload.
3. Create a draft meal item without invoking the configured vision provider.
4. Show the image and an explicit `Analyze` action.
5. If a saved route separately allows automatic analysis, show that scope in
   the routing-rule confirmation and enforce Health's model grant/provider
   policy.
6. Prove the routed blob is the same verified content hash Visual Intake
   approved.

The existing Health manual upload remains available and must not regress.

## Invoice integration

There is no requirement to ship a production Invoice app in the first router
slice. Use a minimal test destination declaring `image.invoice.v1` to prove
the generic contract.

A production Invoice app becomes a separate product slice with:

- original image retention policy;
- OCR and extraction review;
- totals/currency validation;
- duplicate-invoice detection;
- export/accounting integrations;
- its own model and external-processing permissions.

The router must not embed invoice parsing logic.

## Recommendation algorithm

Candidate generation:

1. Map ranked intents to exact interface names.
2. Query `interop.apps` for installed handlers.
3. Apply enabled routing rules.
4. Rank candidates using:
   - exact interface match;
   - classifier confidence;
   - prior user corrections for the same evidence bucket;
   - target availability and current grant state.
5. Return at most five candidates.

Never recommend a target whose bundle does not declare the interface. A
`related app` suggestion is a separate UI section and is never auto-routable.

## Saved routing rules

Rule v1:

```json
{
  "schema": 1,
  "source": "photos",
  "sourceScopeHash": null,
  "intent": "food",
  "minimumConfidence": 0.94,
  "excludedSensitivity": [
    "review-required",
    "blocked-from-automatic-external-processing"
  ],
  "targetInterface": "image.food.v1",
  "targetApp": "health",
  "mode": "auto-deliver",
  "externalProcessing": "never"
}
```

Rules are explicit and revocable. A correction affects recommendation weights
but does not automatically create or broaden a rule.

If a target app is removed, loses its interface, or loses its interop grant,
the rule becomes disabled with a visible reason.

## Repeated-pattern recommendations

Pattern detection operates on bounded derived facts:

- intent/evidence bucket;
- user-selected destination;
- ignored/retained outcome;
- correction count;
- time bucket;
- classifier and taxonomy version.

It does not reread pixels or OCR text.

Minimum recommendation criteria:

- at least 8 similar items;
- activity on at least 3 separate days;
- at least 5 user corrections or retained unresolved items;
- no exact installed handler;
- no dismissed recommendation for the same pattern within its cooldown.

Example:

> You have kept 11 whiteboard images and manually sent 7 of them to notes.
> Create a Whiteboard Inbox app that extracts searchable notes and tasks?

## App Builder handoff

The handoff is always user initiated:

1. User opens a pattern recommendation.
2. Terrane shows the evidence summary and proposed app contract.
3. User chooses which example images, if any, to include.
4. Terrane prepares an App Builder prompt with:
   - proposed name and purpose;
   - required `image.*.v1` interface;
   - `common.receive` payload schema;
   - requested capabilities;
   - retention and privacy expectations;
   - acceptance tests.
5. App Builder opens with an editable draft.

No app is generated, installed, granted, or set as a route without subsequent
explicit user actions.

## Routing tests

- Exact interface discovery with zero, one, and multiple apps.
- Health receives food blob without automatic model invocation.
- Invoice fixture receives invoice blob through the same contract.
- Target cannot read Visual Intake's source blob name.
- Duplicate delivery is idempotent by intake item id and hash.
- Correction changes later ranking but does not create an automatic rule.
- Saved rule routes only within its exact source, intent, confidence, and
  sensitivity scope.
- App removal/interface removal disables the rule.
- Pattern threshold and cooldown tests.
- App Builder handoff includes only selected examples and no Photos identifiers.
