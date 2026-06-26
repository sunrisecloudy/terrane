/** Public surface of renderer-zero (UI-13): the wire types, the renderer, the
 * patch applier, and the DOM shim + serializer. */
export * from "./wire.ts";
export * from "./dom.ts";
export { render } from "./render.ts";
export { parse } from "./parse.ts";
export { applyTree, applyDom, domAt, clone, PatchError } from "./patch.ts";
export { canonicalize, canonicalJson, treeEqual } from "./canonical.ts";
export {
  type EventKind,
  type EmittedEvent,
  type Dispatch,
  type Trigger,
  fireEvent,
  bindEvents,
} from "./events.ts";
export {
  type FocusStopKind,
  type FocusStop,
  type FocusOrder,
  focusOrder,
  applyFocus,
  renderWithFocus,
} from "./focus.ts";
