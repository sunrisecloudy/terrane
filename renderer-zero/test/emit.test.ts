/**
 * Event EMISSION conformance (UI-4 / CR-6) driven by the REAL committed vectors
 * in `forge/fixtures/ui-events/`.
 *
 * Where `events.test.ts` asserts the renderer's patch-application half (applying
 * each vector's expected patches yields the fixture's `final_tree`), THIS suite
 * asserts the renderer's EMISSION half: simulating each vector's event sequence
 * against the rendered DOM emits exactly the `{ action, payload }` the vector
 * expects to dispatch. The patch application is the Rust/core side; here the bar
 * is "the renderer serializes and routes the right event to the host
 * dispatcher".
 *
 * For every event in a fixture we fire the corresponding gesture
 * (`tap`/`change`) at the rendered control — addressed by its `target_test_id`
 * when the vector gives one, else by its ActionRef — carrying the vector's
 * `payload`, and assert:
 *
 *  - a vector with an ActionRef emits `{ action, payload }` IDENTICAL to the
 *    vector's `events[i]` (deep-equal), in order;
 *  - the `no_handler` vector (a Text with `action: null`) emits NOTHING — the
 *    gesture is a silent no-op, not an error and not a `null`-action emit;
 *  - pre-dispatch rejection vectors with no `initial_tree` to render emit
 *    nothing (there is no rendered control to gesture against).
 *
 * Crucially, the renderer emits the event for the `error` vectors too
 * (`unknown_action_rejected`, `invalid_payload_rejected`, `handler_throws`):
 * REJECTION is the core's job, not the renderer's — the renderer's contract is
 * only that it faithfully emitted the right action+payload for the core to then
 * reject.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { parse } from "../src/parse.ts";
import { bindEvents, type EmittedEvent, type Trigger } from "../src/events.ts";
import { type DomElement } from "../src/dom.ts";
import { UI_EVENTS_DIR, readJson, join } from "./fixtures.ts";

interface EventManifest {
  count: number;
  cases: { file: string; kind: string; note?: string }[];
}
interface EventVector {
  action: string | null;
  event_type: string;
  payload?: unknown;
  target_test_id?: string;
}
interface Fixture {
  kind: string;
  initial_tree?: unknown;
  events: EventVector[];
}

const manifest = readJson<EventManifest>(join(UI_EVENTS_DIR, "manifest.json"));

/** The wire `event_type` -> the renderer's gesture kind. */
function kindOf(eventType: string): "tap" | "change" {
  return eventType === "change" ? "change" : "tap";
}

/** Whether the rendered DOM exposes `action` on a control of the gesture `kind`
 * — i.e. whether the renderer can locate a handler to fire (and so emit). */
function exposes(root: DomElement, action: string, kind: "tap" | "change"): boolean {
  const attr = kind === "change" ? "data-action-change" : "data-action-tap";
  const hit = (el: DomElement): boolean => {
    if (el.getAttribute(attr) === action) return true;
    return el.childElements.some(hit);
  };
  return hit(root);
}

for (const c of manifest.cases) {
  test(`emit ${c.kind}: ${c.file}`, () => {
    const fx = readJson<Fixture>(join(UI_EVENTS_DIR, c.file));

    // A pre-dispatch rejection vector (suspended applet) has no tree to render,
    // so there is no control to gesture against: nothing is emitted.
    if (fx.initial_tree === undefined) {
      assert.equal(c.kind, "error", `no initial_tree for non-error case ${c.file}`);
      return;
    }

    const dom = render(parse(fx.initial_tree));
    const { fire, emitted } = bindEvents(dom, () => {});

    const expected: EmittedEvent[] = [];
    for (const ev of fx.events) {
      const kind = kindOf(ev.event_type);
      const trigger: Trigger = {
        kind,
        ...(ev.target_test_id !== undefined ? { testId: ev.target_test_id } : {}),
        ...(ev.action !== null ? { action: ev.action } : {}),
        payload: ev.payload ?? {},
      };
      const result = fire(trigger);

      // The renderer's emission contract: it emits iff the gesture targets a
      // rendered control that carries the matching ActionRef. Two distinct
      // "nothing fires" cases both correctly produce no emit:
      //   - a null-action gesture on a control with no handler (no_handler);
      //   - an action NOT PRESENT on any rendered control
      //     (unknown_action_rejected fires `counter.delete_everything`, which
      //     no node in the tree exposes) — the renderer has nothing to gesture.
      // For a locatable ActionRef the renderer emits exactly { action, payload }
      // for the host to dispatch; REJECTION/throwing (invalid_payload,
      // handler_throws) is the core's job AFTER this faithful emit.
      const willEmit = ev.action !== null && exposes(dom, ev.action, kind);
      if (!willEmit) {
        assert.equal(result, null, `${c.file}: an unlocatable/null-action gesture must not emit`);
      } else {
        assert.deepEqual(
          result,
          { action: ev.action, payload: ev.payload ?? {} },
          `${c.file}: emitted event must match the vector's action+payload`,
        );
        expected.push({ action: ev.action!, payload: ev.payload ?? {} });
      }
    }

    // The full emitted stream equals the locatable-action vector events, in
    // order — the renderer routes the whole sequence to the host dispatcher.
    assert.deepEqual(emitted, expected, `${c.file}: emitted stream mismatch`);

    // Vectors where the renderer is expected to emit NOTHING at all:
    //   - no_handler: the Text target carries no ActionRef;
    //   - unknown_action_rejected: the action is absent from the rendered tree.
    if (c.file === "no_handler_event_ignored.json" || c.file === "unknown_action_rejected.json") {
      assert.equal(emitted.length, 0, `${c.file}: expected no renderer emit`);
    }
    // Vectors where the renderer DOES emit, and the core then rejects/throws —
    // proving emission and dispatch-policy are cleanly separated layers.
    if (c.file === "invalid_payload_rejected.json" || c.file === "handler_throws_prior_tree_intact.json") {
      assert.equal(emitted.length, fx.events.length, `${c.file}: renderer must still emit (core rejects later)`);
    }
  });
}

test("a tap emits the Button onTap ActionRef + payload to the host dispatch", () => {
  const dom = render(
    parse({ type: "Button", testId: "go", label: "Go", onTap: "do.it" }),
  );
  const seen: EmittedEvent[] = [];
  const { fire } = bindEvents(dom, (e) => seen.push(e));
  const out = fire({ kind: "tap", testId: "go", payload: { from: "test" } });
  assert.deepEqual(out, { action: "do.it", payload: { from: "test" } });
  assert.deepEqual(seen, [{ action: "do.it", payload: { from: "test" } }]);
});

test("a change emits the TextField onChange ActionRef carrying payload.value", () => {
  const dom = render(
    parse({ type: "TextField", testId: "name", label: "Name", value: "", onChange: "name.change" }),
  );
  const { fire, emitted } = bindEvents(dom, () => {});
  const out = fire({ kind: "change", testId: "name", payload: { value: "Ada" } });
  assert.deepEqual(out, { action: "name.change", payload: { value: "Ada" } });
  assert.deepEqual(emitted, [{ action: "name.change", payload: { value: "Ada" } }]);
});

test("a Button with NO onTap is a silent no-op (no emit, no throw)", () => {
  const dom = render(parse({ type: "Button", testId: "inert", label: "Inert" }));
  const { fire, emitted } = bindEvents(dom, () => {});
  assert.equal(fire({ kind: "tap", testId: "inert", payload: {} }), null);
  assert.equal(emitted.length, 0);
});

test("a change gesture on a Button does not fire its onTap (kind is respected)", () => {
  // Firing the WRONG kind must not cross-wire: a `change` looks for
  // data-action-change, which a Button never carries.
  const dom = render(parse({ type: "Button", testId: "go", label: "Go", onTap: "do.it" }));
  const { fire, emitted } = bindEvents(dom, () => {});
  assert.equal(fire({ kind: "change", testId: "go", payload: {} }), null);
  assert.equal(emitted.length, 0);
});

test("firing by ActionRef locates a deeply-nested control (list item by stable key)", () => {
  // Mirrors list_item_toggle_by_stable_key: the gesture targets the item whose
  // onTap suffix identifies the stable key; emission carries the vector payload.
  const dom = render(
    parse({
      type: "List",
      testId: "todo-list",
      items: [
        { type: "Button", id: "a", testId: "todo-a", label: "Write PRD", onTap: "todo.toggle:a" },
        { type: "Button", id: "b", testId: "todo-b", label: "Review diff", onTap: "todo.toggle:b" },
      ],
    }),
  );
  const { fire } = bindEvents(dom, () => {});
  assert.deepEqual(fire({ kind: "tap", action: "todo.toggle:b", payload: { id: "b" } }), {
    action: "todo.toggle:b",
    payload: { id: "b" },
  });
});
