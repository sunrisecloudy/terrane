# Step 0 — Product scope and user experience

## Goal

Turn new user images into a reviewable Terrane input stream that can recommend
and eventually automate delivery to specialized apps.

The product is not a universal photo organizer. It is an intake and delegation
layer between an OS image source and Terrane apps.

## Primary desktop journey

### First enable

1. The user opens Visual Intake.
2. Terrane explains that enabling the feature lets the desktop host inspect new
   Photos images for local routing.
3. The user selects a source mode:
   - all new Photos images;
   - one or more selected Photos albums;
   - manual-only, which leaves the automatic observer disabled.
4. macOS presents Photos authorization.
5. Terrane records the current source baseline.
6. The UI says `Watching new images` and does not backfill older assets.

The user request for an all-image stream is supported, but album-only mode
remains available as a narrower privacy choice.

### New item

For every qualifying asset added after the baseline:

1. The native host requests a bounded local representation.
2. Terrane stores a preview and records an intake item.
3. Local classification produces ranked intents.
4. The item appears in Visual Inbox.
5. The router chooses one of four dispositions:
   - recommend one destination;
   - recommend several destinations;
   - apply a previously approved automatic rule;
   - keep the item unclassified.

### User actions

Every pending item supports:

- **Send:** approve the recommended app.
- **Choose app:** override the recommendation.
- **Keep here:** retain without routing.
- **Ignore:** dismiss and release retained bytes according to policy.
- **Always do this:** save a rule only after an explicit confirmation that
  shows the source conditions, target interface, target app, and whether the
  destination may analyze automatically.
- **Delete from Terrane:** remove Terrane's preview and retained copy without
  changing Apple Photos.

Terrane never deletes, edits, favorites, hides, or moves the Photos asset.

## Visual Inbox states

| State | Meaning | Allowed next states |
| --- | --- | --- |
| `pending` | Imported; classification has not completed | `suggested`, `failed`, `ignored` |
| `suggested` | Ranked destination recommendation is available | `routed`, `retained`, `ignored` |
| `routing` | Approved delivery is running | `routed`, `route-failed` |
| `routed` | Destination acknowledged `common.receive` | `retained`, `deleted` |
| `retained` | Kept in Visual Inbox without delivery | `routed`, `ignored`, `deleted` |
| `ignored` | Hidden from active inbox | `retained`, `deleted` |
| `failed` | Ingest or classification could not complete | `pending`, `ignored`, `deleted` |
| `route-failed` | Target did not accept delivery | `suggested`, `routing`, `ignored` |

The durable state distinguishes ingest failure, classification failure, and
delivery failure. A generic `failed` UI may group them, but the event payload
and retry action must remain specific.

## Routing behavior

Recommended initial confidence policy:

- `>= 0.90`: strong recommendation; never auto-route without a saved rule.
- `0.60–0.89`: recommendation with alternatives.
- `< 0.60`: unclassified unless a deterministic metadata rule applies.
- sensitivity flag: disables automatic routing and external processing
  regardless of confidence until the user approves the specific item.

These thresholds are product defaults, not logged classifier truth. Store the
policy version with each routing decision.

## Initial intent taxonomy

Version the first taxonomy as `visual-intent-v1`:

- `food`
- `invoice`
- `receipt`
- `document`
- `whiteboard`
- `screenshot`
- `product`
- `artwork`
- `person`
- `place`
- `other`
- `unknown`

`invoice` and `receipt` remain separate because the eventual destination may
support one but not the other. Classifiers may return multiple intents.

## App recommendation

The router recommends only installed apps that explicitly declare a compatible
interface. It does not infer compatibility from an app name.

When no exact handler exists, the UI can:

1. recommend a related installed app with a clear `related, not exact` label;
2. keep the item in Visual Inbox;
3. offer App Builder after a repeated pattern meets the criteria in
   [04-routing-and-apps.md](04-routing-and-apps.md).

## Non-goals for desktop v1

- Importing the user's existing Photos history by default.
- Video or Live Photo motion/audio intake.
- Continuous operation while Terrane is not running.
- Face identification or person recognition.
- Medical diagnosis from images.
- Automatically sending every image to Health, Invoice, or an AI provider.
- Creating, installing, or granting a new app without explicit approval.
- Editing or deleting assets in Apple Photos.
- iOS implementation.

## Product acceptance

The UI is acceptable only if a user can always answer:

- Why did Terrane see this image?
- Was the original copied, or only a preview?
- Which classifier processed it?
- Why was this app recommended?
- Has any external provider received it?
- Which saved rule, if any, caused automatic routing?
- How can the route or rule be corrected or revoked?
