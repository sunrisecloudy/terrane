---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/ui-events/*.json, forge/fixtures/ui-events/manifest.json
---

# T034 — UI event-dispatch round-trip vectors (UI-4 / CR-6)

The audit found the keystone gap: UI `ActionRef`s (onTap/onChange handlers) are
serialized into the UI tree but NOTHING dispatches them back into the applet.
We are about to build the event-dispatch loop: a serialized UI event re-enters
the engine (calling the applet handler) which returns the NEXT UI tree, diffed to
a patch. I need golden vectors that lock the round-trip so the loop is verifiable
and deterministic (and replay-identical).

## Deliverables

`forge/fixtures/ui-events/<case>.json` + manifest. Each case: an initial UI tree
(or the applet + initial input that produces it), a SEQUENCE of incoming UI events
(each an ActionRef id + payload, e.g. a button tap or a text-field change), and the
EXPECTED resulting UI patch sequence (and/or resulting tree) after each event is
dispatched. Reference the existing UI wire format in `forge/crates/ui` (node.rs /
patch.rs) and the golden tree corpus (T005) for shape.

```json
{ "case": "counter_increment_on_tap",
  "applet": "examples/counter.ts (or inline)",
  "initial_input": {},
  "events": [
    { "action": "increment", "payload": {} },
    { "action": "increment", "payload": {} }
  ],
  "expect": {
    "patches": [
      [{ "op": "replace", "path": "/children/0/text", "value": "1" }],
      [{ "op": "replace", "path": "/children/0/text", "value": "2" }]
    ],
    "deterministic": true
  } }
```

## Coverage (~12)

- a tap handler that updates state -> a patch reflecting the new tree.
- a text-field onChange with a payload value -> patch.
- two sequential events -> two patches, state accumulates.
- an event whose ActionRef id is NOT present in the current tree -> rejected/no-op
  with a typed error (no panic), state unchanged.
- an event payload that fails validation -> rejected, state unchanged.
- an event handler that performs a db write (ctx.db) then renders -> patch + the
  write is recorded (ties to the deterministic record/replay path CR-8).
- an event on a control inside a list/Grid (addressing by stable key) -> correct
  node patched.
- a handler that produces an IDENTICAL tree -> empty patch (no spurious diff).
- a handler that throws -> surfaced as a typed runtime error, prior tree intact.
- replay determinism: dispatching the same event sequence twice yields identical
  patches (byte-identical), proving the loop is replay-safe.
- an event arriving for a suspended/uninstalled applet -> rejected (forward-looks
  to lifecycle).
- a no-handler component receiving an event -> ignored safely.

In `## Result`, flag whether events should be recorded in the run record (so a
session of UI events replays identically) and how the ActionRef id space stays
stable across re-renders (addressing keys), since the Rust dispatch will depend on
that contract.

## Result

Delivered `forge/fixtures/ui-events/` with 12 JSON vectors plus `manifest.json`.
The corpus covers tap dispatch, text-field change payloads, sequential state
accumulation, missing ActionRef rejection, invalid payload rejection, db-write then
render, stable-key list item dispatch, identical-tree empty patch, handler error,
replay determinism, suspended-applet rejection, and safe ignore for no-handler
components.

Contract decisions encoded:

- UI events should be recorded in the run/session record alongside the original
  ActionRef id and payload; replaying the same sequence must produce
  byte-identical patch lists and the same final tree.
- `ActionRef` strings are the dispatch key, while node `id`/`testId` values keep
  controls stable across re-renders. The M0a patch format remains index-path
  based, so list fixtures include stable ids for dispatch identity but still
  expect ordinary index paths in patches.
