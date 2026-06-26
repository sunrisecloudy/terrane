/**
 * The forge UI wire format, ported 1:1 from the Rust ground truth
 * (`forge/crates/ui/src/node.rs` and `patch.rs`).
 *
 * On the wire, nodes are serde-tagged on `"type"` with TS-facing camelCase keys
 * (`direction`, `onTap`, `onChange`, `testId`, ...). Patches are tagged on
 * `"op"` with snake_case op names (`update_text`, `update_prop`, ...).
 *
 * Forward-compatibility (UI-6, NORMATIVE): any object whose `"type"` is not a
 * known catalog member is preserved verbatim as an `UnknownNode` rather than
 * erroring. The full `@forge/std` catalog (Grid/Card/Modal/Tabs/Table/...) also
 * arrives over this M0a wire as unknown-tagged objects; the renderer recognizes
 * them by name but never mutates their payload.
 */

/** A serializable reference to a host action (`Button.onTap`, `TextField.onChange`). */
export type ActionRef = string;

/** Layout direction for a Stack (`"h" | "v"`). */
export type StackDir = "h" | "v";

/** Shared identity fields every typed catalog node may carry (`BaseNode`). */
export interface BaseFields {
  /** Stable identifier (wire key `id`). */
  id?: string;
  /** Test/renderer handle (wire key `testId`) — must survive (de)serialization. */
  testId?: string;
}

/** A directional container of child nodes. */
export interface StackNode extends BaseFields {
  type: "Stack";
  /** Layout direction; defaults to `"v"` when absent/unrecognized (tolerant). */
  direction?: StackDir;
  /** Inter-child spacing token (`"none" | "xs" | "sm" | "md" | "lg"`). */
  gap?: string;
  children: Node[];
}

/** A run of display text. */
export interface TextNode extends BaseFields {
  type: "Text";
  /** The displayed string (wire key `text`). */
  text: string;
  /** Typographic variant (`"body" | "caption" | "title" | "monospace"`). */
  variant?: string;
}

/** A tappable button. */
export interface ButtonNode extends BaseFields {
  type: "Button";
  label: string;
  /** Visual variant (`"primary" | "secondary" | "destructive"`). */
  variant?: string;
  /** Explicit accessible name (UI-7); required for icon-only buttons. */
  ariaLabel?: string;
  /** Action ref fired on tap. */
  onTap?: ActionRef;
}

/** A single-line editable text field. */
export interface TextFieldNode extends BaseFields {
  type: "TextField";
  value: string;
  label?: string;
  /** Explicit accessible name (UI-7). */
  ariaLabel?: string;
  placeholder?: string;
  /** Action ref fired on change. */
  onChange?: ActionRef;
}

/** A list of item nodes. */
export interface ListNode extends BaseFields {
  type: "List";
  items: Node[];
}

/**
 * A symbol key under which an {@link UnknownNode} stashes the ORIGINAL wire
 * object verbatim (UI-6). Rust's `Node::Unknown { type_name, props }` keeps a
 * string discriminant *and* the untouched `serde_json` object: the discriminant
 * may be `""` (absent/non-string `type`) while the object preserves whatever
 * `type` value the wire actually carried — a number, `null`, or no `type` key at
 * all. We mirror that by exposing a normalized string `type` for routing while
 * carrying the untouched original here for lossless canonicalization.
 *
 * It is a `symbol` so it never appears as a wire prop, never collides with a
 * real key, and is dropped by `JSON.stringify`/`structuredClone`-of-plain-object
 * paths the renderer does not control — `parse` re-attaches it on decode.
 */
export const RAW = Symbol.for("forge.renderer-zero.unknownRaw");

/**
 * Forward-compatible fallback for any unrecognized `"type"` (UI-6). Preserves
 * the original object verbatim (under {@link RAW}) so it round-trips exactly —
 * including a non-string or absent `type` — and a future-aware renderer loses
 * nothing. The enumerable `type` is a normalized *string* discriminant (`""`
 * when the wire `type` was absent or not a string) used only for renderer
 * routing and never for canonicalization.
 */
export interface UnknownNode {
  type: string;
  /** The untouched original wire object (UI-6 verbatim source of truth). */
  [RAW]?: Record<string, unknown>;
  [key: string]: unknown;
}

/** The set of `"type"` tags the typed catalog recognizes natively. */
export const KNOWN_TYPES = ["Stack", "Text", "Button", "TextField", "List"] as const;
export type KnownType = (typeof KNOWN_TYPES)[number];

/** A node in the forge UI tree. */
export type Node =
  | StackNode
  | TextNode
  | ButtonNode
  | TextFieldNode
  | ListNode
  | UnknownNode;

/** Narrow a raw object to one of the typed catalog members. */
export function isKnownType(t: unknown): t is KnownType {
  return typeof t === "string" && (KNOWN_TYPES as readonly string[]).includes(t);
}

/** Whether `node` is the UI-6 forward-compatible fallback (an unknown `type`). */
export function isUnknown(node: Node): node is UnknownNode {
  return !isKnownType((node as { type?: unknown }).type);
}

/** An index path from the root of a tree. `[]` is the root itself. */
export type Path = number[];

/** Replace the node at `path` wholesale (node type changed / scalar cleared). */
export interface ReplacePatch {
  op: "replace";
  path: Path;
  node: Node;
}

/** Update the `text` of a Text node at `path`. */
export interface UpdateTextPatch {
  op: "update_text";
  path: Path;
  value: string;
}

/** Update a scalar prop (`label`/`value`/`onTap`/`onChange`/`id`/`testId`/...). */
export interface UpdatePropPatch {
  op: "update_prop";
  path: Path;
  /** Wire prop key (TS-facing name). */
  key: string;
  value: string;
}

/** Insert `node` as the child at the final index of `path`. */
export interface InsertPatch {
  op: "insert";
  path: Path;
  node: Node;
}

/** Remove the child at the final index of `path`. */
export interface RemovePatch {
  op: "remove";
  path: Path;
}

/** A single mutation against a tree, addressed by index `Path`. */
export type Patch =
  | ReplacePatch
  | UpdateTextPatch
  | UpdatePropPatch
  | InsertPatch
  | RemovePatch;

/** The ordered child/item nodes a container exposes; leaves return `[]`. */
export function childrenOf(node: Node): Node[] {
  if (isUnknown(node)) return [];
  switch (node.type) {
    case "Stack":
      return node.children ?? [];
    case "List":
      return node.items ?? [];
    default:
      return [];
  }
}
