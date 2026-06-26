/**
 * Focus-order conformance (UI-7) driven by the REAL committed a11y golden
 * `forge/crates/ui/tests/golden/a11y/representative_screen.json`.
 *
 * The golden gives, per screen, the role/name/path/focusable `annotations` and
 * the expected `focus_order` (`initial_focus` + ordered `stops`, each tagged
 * `kind: Element | Tab`, plus `traps_focus`). The golden does NOT ship the
 * source tree, so this suite RECONSTRUCTS each screen's `Node` tree from the
 * annotations (and synthesizes the Tabs `tab` descriptors from the focus_order's
 * `Tab` stops, since tabs are not rendered nodes), then asserts:
 *
 *  1. `focusOrder(tree)` reproduces the golden order EXACTLY — same stops in the
 *     same order, each with the golden's path/kind/role/name, plus the golden's
 *     `initial_focus` and `traps_focus` (Modal containment).
 *  2. `applyFocus` renders that order onto the DOM: every focusable element
 *     carries a `tabindex` and a `data-focus-order`, the initial-focus element
 *     is `tabindex="0"` + `data-initial-focus`, and a focus-trapping (open Modal)
 *     screen marks the root `data-focus-trap`.
 *
 * Reconstruction fidelity is itself checked: every annotation's role/name must
 * match what the renderer emits for the reconstructed node, so the tree we build
 * is a faithful stand-in for the screen the golden was generated from.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { focusOrder, applyFocus, renderWithFocus, type FocusStop } from "../src/focus.ts";
import { type Node } from "../src/wire.ts";
import { type DomElement } from "../src/dom.ts";
import { A11Y_DIR, readJson, join } from "./fixtures.ts";

interface Annotation {
  type: string;
  role: string;
  name: string | null;
  path: number[];
  focusable: boolean;
}
interface GoldenStop {
  kind: "Element" | "Tab";
  name: string | null;
  path: number[];
  role: string;
}
interface GoldenFocusOrder {
  initial_focus: GoldenStop;
  stops: GoldenStop[];
  traps_focus: boolean;
}
interface Screen {
  annotations: Annotation[];
  focus_order: GoldenFocusOrder;
}

const screens = readJson<Record<string, Screen>>(join(A11Y_DIR, "representative_screen.json"));

// --- tree reconstruction from annotations + focus_order ------------------

/** Build a leaf/known node carrying the annotation's accessible name so the
 * renderer derives the annotated role+name. Containers are filled with children
 * by `buildTree`. */
function leafNode(ann: Annotation): Record<string, unknown> {
  const name = ann.name ?? "";
  switch (ann.type) {
    case "Stack":
      return { type: "Stack", direction: "v", children: [] };
    case "List":
      return { type: "List", items: [] };
    case "Text":
      return { type: "Text", text: name };
    case "Button":
      return { type: "Button", label: name || "x" };
    case "TextField":
      return { type: "TextField", value: "", label: name || "Field" };
    case "Modal":
      return { type: "Modal", ...(ann.name ? { title: ann.name } : {}), children: [] };
    case "Tabs":
      return { type: "Tabs", ...(ann.name ? { ariaLabel: ann.name } : {}), tabs: [], panels: [] };
    case "Grid":
      return {
        type: "Grid",
        ...(ann.role === "grid" ? { interactive: true } : {}),
        children: [],
      };
    default:
      return { type: ann.type };
  }
}

/** The child-array key a container node places its children under. */
function childKeyFor(node: Record<string, unknown>): string {
  switch (node["type"]) {
    case "List":
      return "items";
    case "Tabs":
      return "panels";
    default:
      return "children";
  }
}

/**
 * Reconstruct the screen's `Node` tree from its flat `annotations` (placed by
 * index path) and the focus_order's `Tab` stops (synthesized as the Tabs node's
 * `tabs` descriptors, since a tab is not a rendered node). The result renders to
 * the exact role/name/structure the golden annotates.
 */
function buildTree(screen: Screen): Node {
  const byPath = new Map<string, Record<string, unknown>>();
  // Stable order: shallower paths first so a parent exists before its children.
  const anns = [...screen.annotations].sort((a, b) => a.path.length - b.path.length);
  for (const ann of anns) {
    const node = leafNode(ann);
    byPath.set(JSON.stringify(ann.path), node);
    if (ann.path.length === 0) continue;
    const parent = byPath.get(JSON.stringify(ann.path.slice(0, -1)));
    assert.ok(parent, `annotation at ${JSON.stringify(ann.path)} has no parent node`);
    const key = childKeyFor(parent);
    const arr = (parent[key] ??= []) as unknown[];
    arr.push(node);
  }
  // Synthesize Tabs `tabs` descriptors from the focus_order's Tab stops: each
  // Tab stop at [...tabsPath, i] becomes the i-th descriptor of the Tabs node at
  // tabsPath.
  for (const stop of screen.focus_order.stops) {
    if (stop.kind !== "Tab") continue;
    const tabsPath = stop.path.slice(0, -1);
    const tabsNode = byPath.get(JSON.stringify(tabsPath));
    assert.ok(tabsNode && tabsNode["type"] === "Tabs", `Tab stop ${JSON.stringify(stop.path)} has no Tabs owner`);
    const tabs = (tabsNode["tabs"] ??= []) as unknown[];
    tabs[stop.path[stop.path.length - 1]!] = { type: "Tab", label: stop.name ?? "" };
  }
  const root = byPath.get(JSON.stringify([]));
  assert.ok(root, "no root annotation (path [])");
  return root as unknown as Node;
}

function toGolden(stop: FocusStop): GoldenStop {
  return { kind: stop.kind, name: stop.name, path: stop.path, role: stop.role };
}

// --- conformance ---------------------------------------------------------

test("a11y focus golden: reconstructed trees render the annotated roles + names", () => {
  let checked = 0;
  for (const [screenName, screen] of Object.entries(screens)) {
    const byPath = new Map<string, Node>();
    indexTree(buildTree(screen), [], byPath);
    for (const ann of screen.annotations) {
      const node = byPath.get(JSON.stringify(ann.path));
      assert.ok(node, `${screenName}: no reconstructed node at ${JSON.stringify(ann.path)}`);
      const el = render(node);
      assert.equal(el.getAttribute("role"), ann.role, `${screenName} @ ${JSON.stringify(ann.path)} role`);
      const name = el.getAttribute("aria-label") ?? (el.textContent !== "" ? el.textContent : null);
      // Container nodes carry their (rebuilt) children, so their textContent is
      // not their own name; only assert names for leaf-named nodes.
      if (ann.name !== null && (ann.type === "Text" || ann.type === "Button" || ann.type === "TextField" || ann.type === "Modal" || ann.type === "Tabs")) {
        assert.equal(name, ann.name, `${screenName} @ ${JSON.stringify(ann.path)} name`);
      }
      checked++;
    }
  }
  assert.ok(checked >= 10, `expected to check many annotations, checked ${checked}`);
});

for (const [screenName, screen] of Object.entries(screens)) {
  test(`a11y focus golden: ${screenName} focus order matches`, () => {
    const tree = buildTree(screen);
    const order = focusOrder(tree);

    // 1) Stops match the golden, stop-for-stop (path + kind + role + name).
    assert.deepEqual(
      order.stops.map(toGolden),
      screen.focus_order.stops,
      `${screenName}: focus stop sequence mismatch`,
    );

    // 2) traps_focus (Modal containment) matches.
    assert.equal(order.trapsFocus, screen.focus_order.traps_focus, `${screenName}: traps_focus`);

    // 3) initial_focus matches the golden (kind-tagged, so a Tab and an Element
    //    at the same numeric path are never confused).
    assert.ok(order.initialFocus, `${screenName}: missing initial focus`);
    assert.deepEqual(toGolden(order.initialFocus!), screen.focus_order.initial_focus, `${screenName}: initial focus`);
  });
}

test("applyFocus writes tabindex + initial focus onto the rendered DOM", () => {
  const screen = screens["page"]!;
  const { dom, order } = renderWithFocus(buildTree(screen));

  // The page does not trap focus (no open Modal).
  assert.equal(order.trapsFocus, false);
  assert.equal(dom.getAttribute("data-focus-trap"), null);

  // Every Element stop carries a data-focus-order index and a tabindex.
  let initialFound = false;
  order.stops.forEach((stop, i) => {
    if (stop.kind !== "Element") return;
    const el = elementAt(dom, stop.path);
    assert.ok(el, `no DOM element at ${JSON.stringify(stop.path)}`);
    assert.equal(el!.getAttribute("data-focus-order"), String(i));
    const ti = el!.getAttribute("tabindex");
    assert.ok(ti === "0" || ti === "-1", `tabindex must be 0 or -1, got ${ti}`);
    if (el!.getAttribute("data-initial-focus") === "true") initialFound = true;
  });
  // The page's initial focus is a Tab (Overview), not an Element, so no Element
  // is marked initial — the tablist's tab is the entry point.
  assert.equal(order.initialFocus!.kind, "Tab");
  assert.equal(initialFound, false, "page initial focus is a Tab, no Element should be marked");
});

test("applyFocus traps focus and sets tabindex=0 on a Modal's first focusable", () => {
  const screen = screens["modal"]!;
  const order = focusOrder(buildTree(screen));
  const dom = applyFocus(render(buildTree(screen)), order);

  // An open Modal traps focus.
  assert.equal(order.trapsFocus, true);
  assert.equal(dom.getAttribute("data-focus-trap"), "true");

  // Initial focus = the first focusable child (Cancel at [1]) -> tabindex 0.
  const initial = order.initialFocus!;
  assert.equal(initial.kind, "Element");
  const initialEl = elementAt(dom, initial.path);
  assert.ok(initialEl);
  assert.equal(initialEl!.getAttribute("tabindex"), "0");
  assert.equal(initialEl!.getAttribute("data-initial-focus"), "true");

  // The second stop (Delete at [2]) is reachable only programmatically (-1).
  const second = order.stops[1]!;
  const secondEl = elementAt(dom, second.path);
  assert.equal(secondEl!.getAttribute("tabindex"), "-1");
});

// --- helpers -------------------------------------------------------------

function indexTree(node: Node, path: number[], out: Map<string, Node>): void {
  out.set(JSON.stringify(path), node);
  const obj = node as Record<string, unknown>;
  for (const key of ["children", "items", "panels"]) {
    const arr = obj[key];
    if (Array.isArray(arr)) arr.forEach((c, i) => indexTree(c as Node, [...path, i], out));
  }
}

/** Resolve the rendered element at a render path (List items unwrap their <li>). */
function elementAt(root: DomElement, path: number[]): DomElement | null {
  let cur: DomElement | null = root;
  for (const idx of path) {
    if (cur === null) return null;
    const kids: DomElement[] =
      cur.getAttribute("data-forge-type") === "List"
        ? cur.childElements.map((li) => li.childElements[0]).filter((c): c is DomElement => c !== undefined)
        : cur.childElements;
    cur = kids[idx] ?? null;
  }
  return cur;
}
