# Step 5 — Privacy, security, and retention

## Trust boundary

Visual Intake asks for broad access to a highly sensitive source. Treat Photos
authorization as a platform-level permission, not an app capability grant.

Trust layers:

1. **macOS Photos authorization:** controls what the trusted Terrane host can
   read.
2. **Terrane intake setting:** controls whether the host observes and imports.
3. **Intake resource grant:** allows only the built-in Visual Intake app to
   operate the inbox.
4. **Interop target grant:** controls delivery from Visual Intake to a specific
   app/interface.
5. **Destination grants:** control what the target app can do with the image.
6. **External-processing approval:** separately controls sending pixels or
   derived text to a non-local provider.

No lower layer bypasses a missing higher layer.

## Permission UX

Before requesting macOS access, show:

- source scope (`all new images` or selected albums);
- baseline behavior;
- that local previews/classification are created;
- default retention;
- that Photos is never modified;
- that external analysis is off by default;
- pause and revoke controls.

Source status must distinguish:

- not enabled;
- awaiting macOS permission;
- watching;
- limited access;
- paused by user;
- paused because permission was revoked;
- unavailable because Photos/iCloud is offline;
- degraded because catch-up has failed.

## Data minimization

The default path records:

- source-key hash, never raw Photos local identifier;
- image hash, size, MIME, dimensions, dates, and subtype;
- bounded preview bytes in CAS;
- classification intent, evidence codes, and confidence;
- OCR body hash and counts, never OCR body;
- routing decisions and app acknowledgments.

The default path excludes:

- location;
- faces or biometric templates;
- person identity;
- original filename;
- album title in the event log;
- OCR body;
- model prompt containing image-derived private text;
- Photos asset bytes in the event log.

## External model policy

Ingestion and routing must work with all external model providers disabled.

An external model may receive an item only if:

- the destination app has its normal model grant;
- the route or item has explicit external-processing approval;
- sensitivity policy does not block it;
- the UI names the destination app and provider;
- the event records provider class, policy version, image hash, and approval
  provenance without recording credentials.

An `auto-deliver` rule does not imply `auto-analyze`. These are separate fields
and separate confirmations.

## Threats and controls

| Threat | Control |
| --- | --- |
| Silent full-library scan | Explicit enable, baseline-only initial fetch, visible status |
| Old library unexpectedly imported | No default backfill; bounded explicit backfill only |
| Image silently sent to cloud | Local classifier default; separate external-processing gate |
| Wrong app receives image | Exact declared interface, recommendation review, scoped rule |
| Sensitive document auto-routed | Sensitivity gate; review-required override |
| Target reads entire intake store | App-scoped blob names; target-owned link only |
| Photos identifier leaks into sync/log | Host-only salted sidecar |
| OCR text leaks into logs | Hash/count only; regression tests |
| Duplicate routing | Item id and hash idempotency |
| Deleted Photos asset causes loop | Terminal unavailable state or bounded retry |
| Malicious image decompression | Byte, dimension, pixel, and decoder limits |
| Removed app leaves active rule | `app.removed` reaction disables rule |
| App Builder receives private history | Derived summary plus user-selected examples only |

## Retention defaults

Recommended defaults:

- Pending/suggested preview: 30 days.
- Ignored preview: delete after 7 days.
- Routed intake preview: 7 days after destination acknowledgment.
- Retained source: until user deletion or a user-selected policy.
- Classification/routing summary: keep after byte deletion for explanation,
  learning, and replay, subject to bounded item-state retention.
- Host Photos source index: retain while the source is enabled; tombstone
  hashes prevent accidental reimport after preview deletion.

Retention execution is a recorded cleanup effect. Wall-clock selection happens
at the trusted worker edge; the resulting deletions are explicit events.

The user can choose:

- delete preview after routing;
- keep preview;
- retain a source-quality copy;
- delete all Terrane Visual Intake data;
- reset the source baseline;
- disable and remove all routing rules.

None of these actions changes Apple Photos.

## Telemetry

Default telemetry may include only aggregate operational counters:

- items attempted/succeeded/failed;
- classification latency buckets;
- route recommendation/approval/correction counts;
- permission state category;
- CAS byte totals.

It must not include:

- image or thumbnail bytes;
- hashes stable outside the user's Terrane home;
- OCR text;
- Vision labels for individual images;
- app-builder pattern descriptions;
- source album names;
- Photos identifiers.

## Security acceptance

- Public MCP cannot invoke trusted intake source or ingest commands.
- An ordinary app cannot obtain Photos authorization through `ctx.resource`.
- An ordinary app cannot list Visual Inbox items.
- A target can read only the blob linked into its own namespace.
- Revoking interop or destination grants blocks future routes.
- Revoking Photos permission stops new reads immediately.
- Replay performs no PhotoKit, Vision, network, or app-call side effects.
- Logs, `describe()`, error messages, and diagnostics contain no Photos local
  identifiers, OCR body, location, or image bytes.
