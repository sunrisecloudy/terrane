/**
 * Renderer-side event EMISSION (UI-4 / CR-6): the interactive half of the loop
 * that turns a user gesture on the rendered DOM into a serialized
 * `{ action: ActionRef, payload }` event handed to a host-supplied dispatch
 * callback.
 *
 * In the live system the core owns the *handler* half — it receives the emitted
 * event, runs the applet handler, and replies with a patch set (that
 * `forge/crates/ui/src/patch.rs`, mirrored by {@link applyTree}, applies). This
 * module owns the renderer half: it is what a real host's `onclick` /
 * `oninput` plumbing would call, reduced to its essence so it is testable
 * against the committed `forge/fixtures/ui-events/` vectors.
 *
 * The contract (from `node.rs`'s `Button.onTap` / `TextField.onChange` and the
 * ui-events fixtures):
 *
 *  - A `tap` on a rendered Button whose `onTap` ActionRef is present emits
 *    `{ action: onTap, payload }` — `payload` is whatever the host attaches
 *    (the fixtures carry `{}` or e.g. `{ id: "b" }`).
 *  - A `change` on a rendered TextField whose `onChange` ActionRef is present
 *    emits `{ action: onChange, payload }`; the host carries the new value in
 *    `payload.value` (the `textfield_on_change` vector).
 *  - A gesture on a node with NO matching ActionRef (the `no_handler` vector,
 *    a Text with no handler) emits NOTHING — it is silently ignored, never an
 *    error and never an empty/`null`-action emit.
 *
 * Emission is deliberately separate from dispatch: the renderer only SERIALIZES
 * and routes the event. Whether the resulting action is then accepted
 * (`unknown_action_rejected`), validated (`invalid_payload_rejected`), or the
 * handler throws (`handler_throws`) is the core's concern, not the renderer's —
 * so this layer faithfully emits the event for those vectors too, exactly as a
 * host would, and lets the core be the one to reject it.
 */

import { type DomElement } from "./dom.ts";

/** The two wire event kinds a control surfaces (`Button.onTap`/`TextField.onChange`). */
export type EventKind = "tap" | "change";

/** The DOM attribute carrying each kind's ActionRef (written by `render`). */
const ACTION_ATTR: Record<EventKind, string> = {
  tap: "data-action-tap",
  change: "data-action-change",
};

/**
 * A serialized UI event as it leaves the renderer for the host dispatcher —
 * the exact `{ action, payload }` shape the `ui-events` vectors expect to be
 * dispatched.
 */
export interface EmittedEvent {
  /** The ActionRef the triggered control declared (`onTap`/`onChange`). */
  action: string;
  /** Host-attached payload (e.g. `{}`, `{ value }`, `{ id }`). */
  payload: unknown;
}

/** A host's event sink: receives every emitted event, in order. */
export type Dispatch = (event: EmittedEvent) => void;

/** How a gesture targets a rendered control. */
export interface Trigger {
  /** Which wire event kind fired. */
  kind: EventKind;
  /**
   * Target selector. Exactly one is used, in this priority:
   *  - `testId` — match the control's `data-test-id` (how the fixtures address
   *    nodes that re-render across events);
   *  - `action` — match the control's ActionRef directly (handy when a vector
   *    only names the action);
   * When neither resolves to a control carrying the kind's ActionRef, the
   * gesture is a no-op (no emit) — the `no_handler` contract.
   */
  testId?: string;
  action?: string;
  /** Host-attached payload to carry on the emitted event. */
  payload?: unknown;
}

/**
 * Simulate a user gesture against a rendered tree and emit the resulting event
 * to `dispatch`, returning the emitted event (or `null` when the gesture hit no
 * matching handler and nothing was emitted).
 *
 * This is the renderer's emission entry point: a real host's click/input
 * handler is exactly this — locate the targeted element, read its ActionRef for
 * the gesture kind, and (only if present) hand `{ action, payload }` to the
 * dispatcher. It never throws on a missing handler; it returns `null` so the
 * caller can see "nothing fired" without a control-flow exception.
 */
export function fireEvent(
  root: DomElement,
  trigger: Trigger,
  dispatch: Dispatch,
): EmittedEvent | null {
  const attr = ACTION_ATTR[trigger.kind];
  const target = locateTarget(root, trigger, attr);
  if (target === null) return null;
  const action = target.getAttribute(attr);
  // Present-and-non-empty ActionRef is the only thing that fires. A node with
  // no handler for this kind (Text, or a Button with no `onTap`) emits nothing.
  if (action === null || action === "") return null;
  const event: EmittedEvent = { action, payload: trigger.payload ?? {} };
  dispatch(event);
  return event;
}

/**
 * Locate the rendered element a trigger targets. `testId` wins (stable across
 * re-renders); otherwise the control carrying the kind's ActionRef equal to
 * `trigger.action`. Returns `null` when no element matches — the caller treats
 * that as "no handler, no emit".
 */
function locateTarget(
  root: DomElement,
  trigger: Trigger,
  attr: string,
): DomElement | null {
  if (trigger.testId !== undefined) {
    const byTestId = findBy(root, (el) => el.getAttribute("data-test-id") === trigger.testId);
    // A testId target only fires if it actually carries this kind's ActionRef;
    // a testId'd Text (the no_handler vector) resolves but has no action, so the
    // caller's action===null guard makes it a silent no-op.
    return byTestId;
  }
  if (trigger.action !== undefined) {
    return findBy(root, (el) => el.getAttribute(attr) === trigger.action);
  }
  return null;
}

/** First element (pre-order, self first) satisfying `pred`, else `null`. */
function findBy(el: DomElement, pred: (el: DomElement) => boolean): DomElement | null {
  if (pred(el)) return el;
  for (const child of el.childElements) {
    const found = findBy(child, pred);
    if (found) return found;
  }
  return null;
}

/**
 * Bind a stateful event emitter over a rendered tree: returns a `fire` closure
 * pre-wired to a single `dispatch` sink, plus the running log of everything
 * emitted. This mirrors how a host installs one dispatcher for an applet's
 * whole surface and lets a test replay a sequence of gestures and read back the
 * exact ordered `{ action, payload }` stream the renderer produced.
 */
export function bindEvents(root: DomElement, dispatch: Dispatch): {
  fire: (trigger: Trigger) => EmittedEvent | null;
  emitted: ReadonlyArray<EmittedEvent>;
} {
  const emitted: EmittedEvent[] = [];
  const sink: Dispatch = (event) => {
    emitted.push(event);
    dispatch(event);
  };
  return {
    fire: (trigger: Trigger) => fireEvent(root, trigger, sink),
    emitted,
  };
}
