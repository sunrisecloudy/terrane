# Step 1 — Intake capability and durable contract

## Decision

Add `rust/crates/terrane-cap-intake/` with namespace `intake`.

The capability owns deterministic intake state and routing-policy facts. The
macOS Photos and Vision work remains at the host edge. The built-in
`visual-intake` app owns the blob names used for previews and retained source
representations.

This split preserves Terrane's three core invariants:

1. replay reproduces the same state;
2. capabilities react to recorded facts instead of calling each other in fold;
3. OS and classifier work run once at the edge and record their outputs.

## Ownership model

- `intake` capability: source configuration, item lifecycle, classifications,
  suggestions, decisions, rules, and pattern counters.
- macOS host: PhotoKit authorization, asset observation, local Photos
  identifiers, iCloud retrieval, Vision execution, and trusted dispatch.
- `visual-intake` app: Visual Inbox UI, preview blobs, retained source blobs,
  and user actions.
- target app: its own linked blob name and its own `common.receive` result.

The physical CAS is content-addressed, so linking one hash into the intake app
and a target app does not duplicate bytes.

## Host-only source index

Photos local identifiers are device-local source handles and must not enter the
portable event log. Store them in a host-owned SQLite sidecar:

```text
visual-intake-sources.sqlite3
```

Minimum schema:

```text
photos_assets(
  source_key_hash TEXT PRIMARY KEY,
  local_identifier TEXT NOT NULL,
  added_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL,
  status TEXT NOT NULL
)
```

The log and intake state use only `source_key_hash`. The sidecar is excluded
from sync and app APIs. Backup behavior must be documented explicitly: a full
machine backup may include it, while cross-device sync must not.

The sidecar also stores a randomly generated source-key salt. Mint it once,
persist it before baseline hashing, and never place it in events or app APIs.
Losing or resetting the salt is an explicit source reset that establishes a new
baseline; silently minting a replacement would make old assets look new.

## State

```text
IntakeState {
  sources: BTreeMap<SourceId, SourceRecord>,
  items: BTreeMap<ItemId, IntakeItem>,
  rules: BTreeMap<RuleId, RoutingRule>,
  pattern_summaries: BTreeMap<PatternKey, PatternSummary>
}

SourceRecord {
  source_id,
  kind,                         // photos
  scope_kind,                   // all-new | album
  scope_ref_hash,
  enabled,
  baseline_at_ms,
  policy_version
}

IntakeItem {
  item_id,
  source_id,
  source_key_hash,
  added_at_ms,
  captured_at_ms,
  media_subtype,
  width,
  height,
  preview_blob,
  source_blob,
  classification,
  sensitivity,
  status,
  suggested_routes,
  selected_route,
  external_processing,
  policy_version
}
```

All maps and sets use deterministic ordered collections. Item retention limits
must be enforced in decide and fold without consulting wall-clock time.

## Commands and events

Trusted host commands are not exposed to ordinary apps or public MCP
`capability_command`.

| Command | Authority | Event or effect |
| --- | --- | --- |
| `intake.source.configure` | trusted host | `intake.source-configured` |
| `intake.source.pause` | trusted host | `intake.source-paused` |
| `intake.image.ingest` | trusted host | validate a previously CAS-stored preview reference, then `intake.image-ingested` |
| `intake.image.classify-complete` | trusted host/worker | record the completed local classifier output as `intake.image-classified` |
| `intake.image.classify-fail` | trusted host/worker | record bounded failure as `intake.image-classification-failed` |
| `intake.route.suggest` | trusted host/worker | `intake.route-suggested` |
| `intake.route.approve` | user-mediated trusted host | app-call effect, then `intake.route-completed` or `intake.route-failed` |
| `intake.route.correct` | user-mediated trusted host | `intake.route-corrected` |
| `intake.item.ignore` | user-mediated trusted host | `intake.item-ignored` |
| `intake.item.retain` | user-mediated trusted host | `intake.item-retained` |
| `intake.item.delete` | user-mediated trusted host | `intake.item-deleted` plus blob unlink/removal facts |
| `intake.rule.save` | user-mediated trusted host | `intake.rule-saved` |
| `intake.rule.disable` | user-mediated trusted host | `intake.rule-disabled` |
| `intake.pattern.record` | trusted worker | `intake.pattern-observed` |
| `intake.pattern.dismiss` | user-mediated trusted host | `intake.pattern-dismissed` |

Event names are stable once shipped. Payloads contain a schema version from the
first release.

The macOS connector performs byte acquisition and Vision work outside the
deterministic core. Its trusted ingest helper first dispatches the existing
`blob.put`/`BlobStore` path for `visual-intake`, then commits
`intake.image.ingest` with the verified folded blob reference. If the second
dispatch fails, the unreferenced CAS row is safe for normal blob GC and the
source-index reconciliation retries. Classification follows the native queue
shape: run once in the connector, then dispatch only the completed or failed
result. No intake command reruns PhotoKit or Vision inside decide or fold.

## Queries

| Query | Purpose |
| --- | --- |
| `intake.sources` | Source status and bounded configuration metadata |
| `intake.items` | Paginated inbox items with status filters |
| `intake.item` | One item by id |
| `intake.rules` | Saved routing rules |
| `intake.patterns` | Active app recommendations |
| `intake.status` | Counts, last successful observation, and paused reason |

Queries read folded state only. They do not open Photos, inspect the CAS, run
Vision, or call an app.

## Visual Intake resource

The built-in app receives an `intake` namespace grant with bounded methods:

```text
ctx.resource.intake.list(filterJson)
ctx.resource.intake.get(itemId)
ctx.resource.intake.route(itemId, targetApp)
ctx.resource.intake.correct(itemId, targetApp)
ctx.resource.intake.ignore(itemId)
ctx.resource.intake.retain(itemId)
ctx.resource.intake.delete(itemId)
ctx.resource.intake.saveRule(ruleJson)
ctx.resource.intake.disableRule(ruleId)
```

No resource method exposes Photos local identifiers or an unbounded image
history. Other apps do not receive the `intake` grant.

## Blob lifecycle

Names owned by `visual-intake`:

```text
__intake__/preview/<item-id>.jpg
__intake__/source/<item-id>.<ext>
```

Rules:

- Preview maximum edge: 768 px; JPEG quality 75 unless transparency requires
  PNG.
- Source materialization is deferred until retention, routing, or a
  destination-specific requirement.
- Classification uses the preview unless a classifier explicitly records a
  bounded reason for a larger representation.
- Routing links verified CAS bytes into the target app under a target-owned
  name such as `inbox/visual/<item-id>.<ext>`.
- `common.receive("blob", payload)` receives the target-owned name, hash, size,
  MIME, intake item id, intents, provenance, and external-processing flag.
- A target acknowledgment is recorded before the item becomes `routed`.

## Replay

Replay:

- folds source, item, classification, routing, rule, and pattern events;
- never reopens Photos;
- never reruns Vision;
- never redownloads an iCloud asset;
- never repeats an interop call;
- never recreates or transforms image bytes.

Missing CAS bytes are reported using the existing blob contract. They do not
change folded intake state.

## Limits

- Maximum preview: 4 MiB compressed and 16 megapixels decoded.
- Maximum materialized image: existing blob cap.
- Maximum 20 classification observations per item.
- Maximum 5 route candidates per suggestion.
- Maximum 500 active inbox items; older terminal items retain summary metadata
  while preview/source retention follows policy.
- Maximum 100 enabled rules.
- Maximum 50 active pattern recommendations.
- No image bytes, OCR body, location, or Photos identifier in `describe()`.

## Required tests

- Decide validation for every command and payload version.
- Trusted-host refusal for ingest/configuration commands.
- Resource grant enforcement.
- Fold lifecycle and illegal transition rejection.
- Replay identity across ingest, classification, suggestion, correction, and
  route completion.
- App removal disables affected rules and route candidates without deleting
  unrelated intake items.
- Duplicate `source_key_hash` is idempotent.
- Duplicate blob hash does not create a second physical CAS row.
- Public command policy explicitly refuses all host-only commands.
