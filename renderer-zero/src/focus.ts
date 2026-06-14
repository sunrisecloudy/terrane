/**
 * UI-7 focus-order emission + application, ported 1:1 from
 * `forge/crates/ui/src/focus.rs`.
 *
 * Phase 1 (`render.ts`) emits each node's ARIA role + accessible name. This
 * module is phase 2: given a parsed `Node` tree it computes the deterministic
 * **focus order** — the sequence in which a keyboard user reaches the focusable
 * elements — per the spec's per-container rules, then APPLIES that order onto a
 * rendered DOM (writing `tabindex` and marking the initial focus). The computed
 * order is asserted, stop-for-stop, against the committed a11y golden
 * (`.../golden/a11y/representative_screen.json` -> `focus_order`).
 *
 * The container rules (mirroring focus.rs):
 *  - Stack / Grid / Card — focusable descendants in source order.
 *  - Tabs — each tab is a `tab` stop in the tablist (kind `Tab`), then ONLY the
 *    active panel's focusables (inactive panels are not in the tab order).
 *  - Modal — an OPEN modal CONTAINS focus: the order holds only that dialog's
 *    own focusable descendants (addressed by their real render path, so a nested
 *    dialog keeps its Modal path prefix), `trapsFocus` is set, and initial focus
 *    moves to the first focusable child (or the dialog itself when it has none).
 *    A closed modal (`open: false`) hides its descendants.
 *  - UI-6 unknown fallback — itself never a tab stop, but its focusable KNOWN
 *    descendants stay in the order (the one exception is a Scroll declared
 *    `focusable: true`).
 *
 * Each stop is keyed by an index `Path` and tagged with its {@link FocusStopKind}
 * so a `tab` at `[1,0]` is never confused with the rendered panel child at
 * `[1,0]` — exactly the disambiguation focus.rs's `FocusStopKind` provides.
 */

import { render } from "./render.ts";
import { type DomElement } from "./dom.ts";
import { type Node, type Path, childrenOf, isUnknown } from "./wire.ts";
import { parse } from "./parse.ts";

/** What a {@link FocusStop}'s `path` addresses (mirrors `focus.rs::FocusStopKind`). */
export type FocusStopKind = "Element" | "Tab";

/** One stop in a tree's focus order: a focusable element (or Tabs tab) addressed
 * by its index path, with the role + accessible name it exposes. */
export interface FocusStop {
  path: Path;
  kind: FocusStopKind;
  role: string;
  name: string | null;
}

/** A tree's emitted focus order (UI-7): the ordered stops plus the
 * dialog-containment metadata the Modal rule requires. */
export interface FocusOrder {
  stops: FocusStop[];
  /** Whether focus is TRAPPED within this order (open-Modal containment). */
  trapsFocus: boolean;
  /** Where focus moves when the order becomes active — the first stop, or (for a
   * childless open Modal) the dialog box itself. `null` for an empty order. */
  initialFocus: FocusStop | null;
}

/**
 * Compute the deterministic focus order of `node` (UI-7), addressed by index
 * path from `node`. Mirrors `focus.rs::Node::focus_order`.
 */
export function focusOrder(node: Node): FocusOrder {
  // An open Modal anywhere captures focus for the whole screen: contain the
  // order to the topmost one, path-prefixed from `node`.
  const open = findOpenModal(node, []);
  if (open !== null) {
    const { modal, path: modalPath } = open;
    const stops: FocusStop[] = [];
    collectDescendants(modal, [...modalPath], stops);
    const initialFocus = stops.length > 0 ? stops[0]! : stopFor(modal, modalPath);
    return { stops, trapsFocus: true, initialFocus };
  }
  const stops: FocusStop[] = [];
  collect(node, [], stops);
  return { stops, trapsFocus: false, initialFocus: stops.length > 0 ? stops[0]! : null };
}

/**
 * Apply a focus order onto a rendered DOM (UI-7): write `tabindex` on every
 * Element stop in keyboard order (`0` for the initial-focus element, `-1`
 * elsewhere — the programmatic-focus / roving-tabindex convention), and mark
 * the initial focus with `data-initial-focus="true"` + `data-focus-trap` on the
 * root when the order traps focus. Tab-kind stops address the Tabs `tabs`
 * descriptor (not a rendered child), so they are recorded on the owning Tabs
 * element via `data-focus-order` rather than a tabindex.
 *
 * Returns the same root for chaining and so a test can read the focus attributes
 * straight back off the DOM.
 */
export function applyFocus(root: DomElement, order: FocusOrder): DomElement {
  if (order.trapsFocus) root.setAttribute("data-focus-trap", "true");
  // Record the keyboard order index on each reachable element/tab so the DOM is
  // introspectable and the order is recoverable from the rendered tree alone.
  order.stops.forEach((stop, i) => {
    if (stop.kind === "Tab") {
      const tabs = domAtTabs(root, stop.path);
      if (tabs) {
        const prior = tabs.getAttribute("data-tab-order");
        tabs.setAttribute("data-tab-order", prior ? `${prior},${i}` : String(i));
      }
      return;
    }
    const el = elementAt(root, stop.path);
    if (el === null) return;
    el.setAttribute("data-focus-order", String(i));
    // Roving tabindex: the entry point is 0, every other stop is reachable only
    // programmatically (-1) until focus moves to it.
    const isInitial =
      order.initialFocus !== null &&
      order.initialFocus.kind === "Element" &&
      samePath(order.initialFocus.path, stop.path);
    el.setAttribute("tabindex", isInitial ? "0" : "-1");
    if (isInitial) el.setAttribute("data-initial-focus", "true");
  });
  // A childless open Modal focuses the dialog box itself (an Element initial
  // focus that is not in `stops`); mark it so a renderer knows where to land.
  if (
    order.initialFocus !== null &&
    order.initialFocus.kind === "Element" &&
    !order.stops.some((s) => s.kind === "Element" && samePath(s.path, order.initialFocus!.path))
  ) {
    const el = elementAt(root, order.initialFocus.path);
    if (el) {
      el.setAttribute("tabindex", "-1");
      el.setAttribute("data-initial-focus", "true");
    }
  }
  return root;
}

/** Render `node` then apply its focus order in one step (UI-7 convenience). */
export function renderWithFocus(node: Node): { dom: DomElement; order: FocusOrder } {
  const order = focusOrder(node);
  const dom = applyFocus(render(node), order);
  return { dom, order };
}

// --- order computation (mirrors focus.rs) --------------------------------

/** Whether `node` is itself a keyboard tab stop (`focus.rs::is_focusable`). */
function isFocusable(node: Node): boolean {
  if (isUnknown(node)) {
    // Only an independently-focusable Scroll region qualifies.
    return node.type === "Scroll" && node["focusable"] === true;
  }
  return node.type === "Button" || node.type === "TextField";
}

/** Collect focusable stops from `node` (inclusive when it is itself a stop). */
function collect(node: Node, path: Path, out: FocusStop[]): void {
  if (isFocusable(node)) out.push(stopFor(node, path));
  descend(node, path, out, collect);
}

/** Collect only `node`'s focusable DESCENDANTS (the Modal-contents rule). */
function collectDescendants(node: Node, path: Path, out: FocusStop[]): void {
  descend(node, path, out, collect);
}

/** Walk `node`'s ordered children honoring the per-container traversal rules. */
function descend(
  node: Node,
  path: Path,
  out: FocusStop[],
  visit: (n: Node, p: Path, o: FocusStop[]) => void,
): void {
  // A CLOSED Modal is off-screen: its descendants are excluded.
  if (isClosedModal(node)) return;
  if (isTabs(node)) {
    descendTabs(node, path, out, visit);
    return;
  }
  orderedChildren(node).forEach((child, i) => visit(child, [...path, i], out));
}

/** Tabs traversal: each tab is a `Tab` stop in the tablist, then ONLY the active
 * panel's focusables (`focus.rs::descend_tabs`). */
function descendTabs(
  node: Node,
  path: Path,
  out: FocusStop[],
  visit: (n: Node, p: Path, o: FocusStop[]) => void,
): void {
  if (!isUnknown(node)) return;
  const tabs = unknownChildNodes(node, ["tabs"]);
  tabs.forEach((tab, i) => out.push(tabStop(tab, [...path, i])));
  const panels = unknownChildNodes(node, ["panels", "children"]);
  if (panels.length === 0) return;
  const active = Math.min(activeTabIndex(node), panels.length - 1);
  visit(panels[active]!, [...path, active], out);
}

/** Ordered child nodes a container exposes for focus traversal, in source order.
 * An unknown container re-parses its verbatim `children`/`items` arrays. */
function orderedChildren(node: Node): Node[] {
  if (isUnknown(node)) return unknownChildNodes(node, ["children", "items"]);
  return childrenOf(node);
}

/** Re-parse the nested node arrays an unknown carries verbatim under any `keys`. */
function unknownChildNodes(node: Node, keys: string[]): Node[] {
  const out: Node[] = [];
  for (const key of keys) {
    const arr = (node as Record<string, unknown>)[key];
    if (Array.isArray(arr)) for (const v of arr) out.push(parse(v));
  }
  return out;
}

/** Build a `FocusStop` from a focusable node, taking role+name from the renderer
 * (the single accessibility source of truth — read straight off the DOM). */
function stopFor(node: Node, path: Path): FocusStop {
  const el = render(node);
  return { path: [...path], kind: "Element", role: el.getAttribute("role") ?? "", name: accessibleName(el) };
}

/** A focus stop for a Tabs `tab` descriptor: always role `tab`, named by its
 * `label`/`title`/`ariaLabel` (`focus.rs::tab_stop`). */
function tabStop(tab: Node, path: Path): FocusStop {
  const props = tab as Record<string, unknown>;
  let name: string | null = null;
  for (const k of ["label", "title", "ariaLabel"]) {
    const v = props[k];
    if (typeof v === "string" && v.trim() !== "") {
      name = v;
      break;
    }
  }
  return { path: [...path], kind: "Tab", role: "tab", name };
}

/** The accessible name a rendered element exposes: `aria-label`, else its text
 * content, else `null` — matching how the a11y golden reports `name`. */
function accessibleName(el: DomElement): string | null {
  const aria = el.getAttribute("aria-label");
  if (aria !== null) return aria;
  const text = el.textContent;
  return text !== "" ? text : null;
}

// --- modal / tabs predicates (mirrors focus.rs) --------------------------

/** Find the topmost open Modal at/below `node`, with its index path. */
function findOpenModal(node: Node, path: Path): { modal: Node; path: Path } | null {
  if (isOpenModal(node)) return { modal: node, path: [...path] };
  for (const [i, child] of reachableChildren(node)) {
    const found = findOpenModal(child, [...path, i]);
    if (found) return found;
  }
  return null;
}

/** The `(renderIndex, child)` pairs focus traversal descends into (a Tabs exposes
 * only its active panel at its real render index). */
function reachableChildren(node: Node): [number, Node][] {
  if (isTabs(node) && isUnknown(node)) {
    const panels = unknownChildNodes(node, ["panels", "children"]);
    if (panels.length === 0) return [];
    const active = Math.min(activeTabIndex(node), panels.length - 1);
    return [[active, panels[active]!]];
  }
  return orderedChildren(node).map((c, i): [number, Node] => [i, c]);
}

function isTabs(node: Node): boolean {
  return isUnknown(node) && node.type === "Tabs";
}

/** A Modal is OPEN unless it carries `open: false` (absent `open` ⇒ open). */
function isOpenModal(node: Node): boolean {
  return isUnknown(node) && node.type === "Modal" && (node as Record<string, unknown>)["open"] !== false;
}

function isClosedModal(node: Node): boolean {
  return isUnknown(node) && node.type === "Modal" && (node as Record<string, unknown>)["open"] === false;
}

/** Active tab index (`activeTab`/`active`, default 0). */
function activeTabIndex(node: Node): number {
  const props = node as Record<string, unknown>;
  for (const k of ["activeTab", "active"]) {
    const v = props[k];
    if (typeof v === "number" && Number.isInteger(v) && v >= 0) return v;
  }
  return 0;
}

// --- DOM addressing for applyFocus ---------------------------------------

/**
 * Resolve the rendered element at a focus-order render path. Reuses the patch
 * layer's `domAt`-style descent (List items unwrap their `<li>` wrapper; Tabs
 * panels are the Tabs element's rendered children).
 */
function elementAt(root: DomElement, path: Path): DomElement | null {
  let cur: DomElement | null = root;
  for (const idx of path) {
    if (cur === null) return null;
    cur = renderChildren(cur)[idx] ?? null;
  }
  return cur;
}

/** The Tabs element a `Tab` stop addresses: the stop path is `[...tabsPath, i]`,
 * so the owning Tabs element is at the parent path. */
function domAtTabs(root: DomElement, tabPath: Path): DomElement | null {
  return elementAt(root, tabPath.slice(0, -1));
}

/** Rendered-child elements of a container, unwrapping List `<li>` wrappers
 * (mirrors `patch.ts::renderChildren`). */
function renderChildren(el: DomElement): DomElement[] {
  if (el.getAttribute("data-forge-type") === "List") {
    return el.childElements
      .map((li) => li.childElements[0])
      .filter((c): c is DomElement => c !== undefined);
  }
  return el.childElements;
}

function samePath(a: Path, b: Path): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}
