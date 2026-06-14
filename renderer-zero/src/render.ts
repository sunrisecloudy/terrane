/**
 * The reference renderer (UI-13): turn a forge UI `Node` tree into a DOM
 * subtree.
 *
 * Coverage. Every typed catalog member from `forge/crates/ui/src/node.rs`
 * (Stack, Text, Button, TextField, List) renders natively. Every other
 * `@forge/std` catalog component named in `spec/accessibility.md`
 * (Grid, Card, Scroll, Spacer, Divider, Markdown, Tabs, Icon, Image, Chart,
 * Table, Modal, Form, TextArea, Select, MultiSelect, Checkbox, Switch, Slider,
 * DatePicker, Badge, Stat) reaches this renderer as a UI-6 unknown-tagged
 * object on the M0a wire; the renderer recognizes it by name and renders a
 * semantically appropriate element with the correct ARIA role/name, while
 * preserving its payload. A genuinely unknown `type` renders the spec's
 * "Unknown Component Fallback": a labelled `group` reading
 * "Unsupported component <Type>", never the raw JSON.
 *
 * Each rendered element is annotated so a patch applier can re-locate nodes and
 * so tests can read the tree back: `data-forge-type` carries the wire type,
 * `data-action-tap` / `data-action-change` carry the ActionRef strings, and
 * `id` / `data-test-id` mirror the BaseNode identity fields. ARIA `role` and the
 * accessible name (`aria-label` or text content) follow `accessibility.rs`
 * exactly so the rendered DOM is conformant.
 */

import { DomDocument, type DomElement } from "./dom.ts";
import {
  type Node,
  type StackNode,
  type TextNode,
  type ButtonNode,
  type TextFieldNode,
  type ListNode,
  type UnknownNode,
  isKnownType,
} from "./wire.ts";

const doc = new DomDocument();

/** Render `node` (and its subtree) into a fresh detached DOM element. */
export function render(node: Node): DomElement {
  // `UnknownNode` shares the `type: string` discriminant, so we route by the
  // catalog membership first, then by the literal tag for the typed arms.
  if (!isKnownType(node.type)) return renderUnknown(node as UnknownNode);
  switch (node.type) {
    case "Stack":
      return renderStack(node as StackNode);
    case "Text":
      return renderText(node as TextNode);
    case "Button":
      return renderButton(node as ButtonNode);
    case "TextField":
      return renderTextField(node as TextFieldNode);
    case "List":
      return renderList(node as ListNode);
    default:
      return renderUnknown(node as UnknownNode);
  }
}

/** Stamp the shared identity + type bookkeeping every rendered element carries. */
function base(el: DomElement, node: Node, wireType: string): DomElement {
  el.setAttribute("data-forge-type", wireType);
  const id = (node as { id?: unknown }).id;
  if (typeof id === "string") el.setAttribute("id", id);
  const testId = (node as { testId?: unknown }).testId;
  if (typeof testId === "string") el.setAttribute("data-test-id", testId);
  return el;
}

function nonBlank(s: unknown): string | undefined {
  return typeof s === "string" && s.trim() !== "" ? s : undefined;
}

// --- Typed catalog -------------------------------------------------------

function renderStack(node: StackNode): DomElement {
  const el = base(doc.createElement("div"), node, "Stack");
  el.setAttribute("role", "group");
  const dir = node.direction === "h" ? "h" : "v";
  el.setAttribute("data-direction", dir);
  el.dataset.layout = dir === "h" ? "row" : "column";
  if (node.gap !== undefined) el.setAttribute("data-gap", node.gap);
  for (const child of node.children ?? []) el.appendChild(render(child));
  return el;
}

function renderText(node: TextNode): DomElement {
  const el = base(doc.createElement("span"), node, "Text");
  el.setAttribute("role", "text");
  if (node.variant !== undefined) el.setAttribute("data-variant", node.variant);
  el.textContent = node.text ?? "";
  return el;
}

function renderButton(node: ButtonNode): DomElement {
  const el = base(doc.createElement("button"), node, "Button");
  el.setAttribute("type", "button");
  el.setAttribute("role", "button");
  if (node.variant !== undefined) el.setAttribute("data-variant", node.variant);
  // Accessible name: visible label, else ariaLabel for an icon-only button
  // (never inferred from an icon) — mirrors accessibility.rs.
  const label = nonBlank(node.label);
  if (label !== undefined) {
    el.textContent = node.label;
    const aria = nonBlank(node.ariaLabel);
    if (aria !== undefined) el.setAttribute("aria-label", aria);
  } else {
    const aria = nonBlank(node.ariaLabel);
    if (aria !== undefined) el.setAttribute("aria-label", aria);
  }
  if (node.onTap !== undefined) el.setAttribute("data-action-tap", node.onTap);
  return el;
}

function renderTextField(node: TextFieldNode): DomElement {
  const el = base(doc.createElement("input"), node, "TextField");
  el.setAttribute("type", "text");
  el.setAttribute("role", "textbox");
  el.setAttribute("value", node.value ?? "");
  if (node.placeholder !== undefined) el.setAttribute("placeholder", node.placeholder);
  // Name: label, else ariaLabel; placeholder never counts (accessibility.rs).
  const label = nonBlank(node.label);
  const aria = nonBlank(node.ariaLabel);
  if (label !== undefined) el.setAttribute("aria-label", label);
  else if (aria !== undefined) el.setAttribute("aria-label", aria);
  if (node.onChange !== undefined) el.setAttribute("data-action-change", node.onChange);
  return el;
}

function renderList(node: ListNode): DomElement {
  const el = base(doc.createElement("ul"), node, "List");
  el.setAttribute("role", "list");
  for (const item of node.items ?? []) {
    const li = doc.createElement("li");
    li.setAttribute("role", "listitem");
    li.appendChild(render(item));
    el.appendChild(li);
  }
  return el;
}

// --- Extended @forge/std catalog (arrives as UI-6 unknown on the M0a wire) ---

/**
 * The role + element tag + accessible-name source for every `@forge/std`
 * catalog component that is not a typed node. Keyed off the wire `type`, this
 * mirrors `unknown_accessibility` in `accessibility.rs` so the rendered DOM is
 * accessibility-conformant. `nameFrom` lists the prop keys (in priority order)
 * a component's accessible name is derived from.
 */
interface CatalogSpec {
  tag: string;
  /** Static role, or a function deriving it from props (e.g. Grid). */
  role: string | ((props: UnknownNode) => string);
  /** Prop keys the accessible name comes from, in priority order. */
  nameFrom: string[];
  /** Container child-array keys to recurse into, in order. */
  childKeys?: string[];
}

const CATALOG: Record<string, CatalogSpec> = {
  // Structural containers.
  Grid: {
    tag: "div",
    role: (p) => (isInteractiveGrid(p) ? "grid" : "group"),
    nameFrom: ["ariaLabel"],
    childKeys: ["children", "items"],
  },
  Card: {
    tag: "section",
    role: (p) => (nonBlank(p["ariaLabel"]) ? "region" : "group"),
    nameFrom: ["ariaLabel"],
    childKeys: ["children", "items"],
  },
  Scroll: {
    tag: "div",
    role: (p) => (nonBlank(p["ariaLabel"]) ? "region" : "group"),
    nameFrom: ["ariaLabel"],
    childKeys: ["children", "items"],
  },
  Spacer: { tag: "div", role: "presentation", nameFrom: [] },
  Divider: { tag: "hr", role: "separator", nameFrom: ["ariaLabel"] },
  Markdown: { tag: "div", role: "document", nameFrom: [] },
  Tabs: { tag: "div", role: "tablist", nameFrom: ["ariaLabel"], childKeys: ["panels", "children"] },
  // Media / regions.
  // Icon: a decorative icon (`decorative: true`) is presentational and exposes
  // NO accessible name; an informative icon is an `img` named by `ariaLabel`
  // (mirrors `unknown_accessibility`'s Icon arm in accessibility.rs).
  Icon: {
    tag: "span",
    role: (p) => (isDecorative(p) ? "presentation" : "img"),
    nameFrom: ["ariaLabel"],
  },
  Image: { tag: "img", role: "img", nameFrom: ["alt"] },
  Chart: { tag: "figure", role: "img", nameFrom: ["summary"] },
  Table: { tag: "table", role: "table", nameFrom: ["caption", "ariaLabel"], childKeys: ["children", "items"] },
  Modal: { tag: "div", role: "dialog", nameFrom: ["title"], childKeys: ["children", "items"] },
  Form: { tag: "form", role: "form", nameFrom: ["ariaLabel"], childKeys: ["children", "items"] },
  // Form controls.
  TextArea: { tag: "textarea", role: "textbox", nameFrom: ["label", "ariaLabel"] },
  Select: { tag: "select", role: "combobox", nameFrom: ["label", "ariaLabel"] },
  MultiSelect: { tag: "select", role: "listbox", nameFrom: ["label", "ariaLabel"] },
  Checkbox: { tag: "input", role: "checkbox", nameFrom: ["label", "ariaLabel"] },
  Switch: { tag: "input", role: "switch", nameFrom: ["label", "ariaLabel"] },
  Slider: { tag: "input", role: "slider", nameFrom: ["label", "ariaLabel"] },
  DatePicker: { tag: "input", role: "combobox", nameFrom: ["label", "ariaLabel"] },
  Badge: { tag: "span", role: "status", nameFrom: ["label", "ariaLabel"] },
  Stat: { tag: "span", role: "status", nameFrom: ["label", "ariaLabel"] },
};

function renderUnknown(node: UnknownNode): DomElement {
  const spec = CATALOG[node.type];
  if (spec === undefined) {
    // True UI-6 fallback (a `type` in NEITHER the typed nor the extended
    // catalog): the spec's "Unknown Component Fallback" — a labelled `group`
    // reading "Unsupported component <Type>", NEVER the raw payload JSON. The
    // fallback still PRESERVES the subtree: any KNOWN descendant the unknown
    // carries under `children`/`items` is rendered so accessibility/content is
    // never lost behind the placeholder (mirrors the focus.rs traversal that
    // re-parses an unknown's verbatim child arrays).
    const el = doc.createElement("div");
    el.setAttribute("data-forge-type", node.type);
    el.setAttribute("data-forge-unknown", "true");
    el.setAttribute("role", "group");
    el.setAttribute("aria-label", `Unsupported component ${node.type}`);
    appendUnknownChildren(el, node, ["children", "items"]);
    return el;
  }
  const el = doc.createElement(spec.tag);
  el.setAttribute("data-forge-type", node.type);
  el.setAttribute("data-forge-catalog", "extended");
  const id = node["id"];
  if (typeof id === "string") el.setAttribute("id", id);
  const testId = node["testId"];
  if (typeof testId === "string") el.setAttribute("data-test-id", testId);
  const role = typeof spec.role === "function" ? spec.role(node) : spec.role;
  el.setAttribute("role", role);
  // Accessible name from the first non-blank source key. A presentational role
  // (e.g. a decorative Icon, a Spacer) intentionally exposes NO name, matching
  // `AxNameSource::None` in accessibility.rs.
  if (role !== "presentation") {
    for (const key of spec.nameFrom) {
      const v = nonBlank(node[key]);
      if (v !== undefined) {
        el.setAttribute("aria-label", v);
        break;
      }
    }
  }
  // Recurse into declared child arrays so known descendants still render.
  if (spec.childKeys) appendUnknownChildren(el, node, spec.childKeys);
  return el;
}

/** Render the known descendants an unknown node carries under the first present
 * array among `keys`, appending them to `el`. Non-node entries are skipped
 * (tolerant per UI-6). */
function appendUnknownChildren(el: DomElement, node: UnknownNode, keys: string[]): void {
  for (const key of keys) {
    const arr = node[key];
    if (Array.isArray(arr)) {
      for (const child of arr) {
        if (isNodeLike(child)) el.appendChild(render(child as Node));
      }
      return;
    }
  }
}

function isNodeLike(v: unknown): v is Node {
  return typeof v === "object" && v !== null && typeof (v as { type?: unknown }).type === "string";
}

/** Whether an element declared itself decorative (`decorative: true`),
 * mirroring `is_decorative` in accessibility.rs. */
function isDecorative(props: UnknownNode): boolean {
  return props["decorative"] === true;
}

/** Whether a Grid is interactive enough for the `grid` role (accessibility.rs). */
function isInteractiveGrid(props: UnknownNode): boolean {
  return (
    props["interactive"] === true ||
    props["selectable"] === true ||
    "columns" in props ||
    "rows" in props
  );
}
