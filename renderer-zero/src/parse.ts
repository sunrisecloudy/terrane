/**
 * Wire decoder (UI-6), ported from the Rust `Deserialize` impl in
 * `forge/crates/ui/src/node.rs` (`NodeVisitor`).
 *
 * Decoding a raw wire object into the renderer's `Node` model is *lossy by
 * design for known nodes* and *lossless for unknown ones*, exactly like serde:
 *
 *  - A known `type` (Stack/Text/Button/TextField/List) keeps only the catalog
 *    fields; any extra prop on a known node (e.g. `sparkle: true` on a Button)
 *    is dropped, never an error (node.rs lines 8-12).
 *  - An unknown `type` — or an object with no string `type` — is preserved
 *    verbatim as an `UnknownNode` (the `type` key included) so it round-trips.
 *  - Nested children/items are decoded recursively, so a nested unknown inside a
 *    known container also becomes a verbatim fallback.
 *  - Absent optionals decode to "not present"; Stack `direction` defaults to
 *    `"v"` when absent/unrecognized (tolerant).
 *
 * Parsing is what makes the canonical form authoritative: the renderer always
 * operates on the decoded model, so "extra prop on a known node" cannot leak
 * into render output or a diff.
 */

import {
  type Node,
  type StackNode,
  type TextNode,
  type ButtonNode,
  type TextFieldNode,
  type ListNode,
  type UnknownNode,
  RAW,
  isKnownType,
} from "./wire.ts";

type Raw = Record<string, unknown>;

function str(obj: Raw, key: string): string | undefined {
  const v = obj[key];
  return typeof v === "string" ? v : undefined;
}

function nodeArray(obj: Raw, key: string): Node[] {
  const v = obj[key];
  if (!Array.isArray(v)) return [];
  return v.map((e) => parse(e));
}

/** Decode one raw wire value into a `Node` (the model the renderer operates on). */
export function parse(value: unknown): Node {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    // Not an object → verbatim unknown fallback. Rust's `NodeVisitor` only ever
    // visits a map, so a non-object never reaches it; we model it as an empty
    // verbatim unknown (`type_name` = "", no props) so the renderer stays total.
    return makeUnknown({});
  }
  const obj = value as Raw;
  const type = obj["type"];
  if (typeof type !== "string" || !isKnownType(type)) {
    // Unknown `type` (or missing/non-string) → preserve the ORIGINAL object
    // VERBATIM (UI-6). Rust keeps `props = obj` untouched — so a non-string or
    // absent `type` survives exactly (a number stays a number, an absent key
    // stays absent) and is NEVER coerced to `""`. The enumerable `type` we add
    // is a normalized string discriminant for renderer routing only; the RAW
    // carrier is the lossless source of truth for canonicalization.
    return makeUnknown(obj);
  }

  switch (type) {
    case "Stack": {
      const dir = str(obj, "direction");
      const out: StackNode = {
        type: "Stack",
        direction: dir === "h" ? "h" : "v",
        children: nodeArray(obj, "children"),
      };
      copyBase(obj, out);
      const gap = str(obj, "gap");
      if (gap !== undefined) out.gap = gap;
      return out;
    }
    case "Text": {
      const out: TextNode = { type: "Text", text: str(obj, "text") ?? "" };
      copyBase(obj, out);
      const variant = str(obj, "variant");
      if (variant !== undefined) out.variant = variant;
      return out;
    }
    case "Button": {
      const out: ButtonNode = { type: "Button", label: str(obj, "label") ?? "" };
      copyBase(obj, out);
      const variant = str(obj, "variant");
      if (variant !== undefined) out.variant = variant;
      const aria = str(obj, "ariaLabel");
      if (aria !== undefined) out.ariaLabel = aria;
      const onTap = str(obj, "onTap");
      if (onTap !== undefined) out.onTap = onTap;
      return out;
    }
    case "TextField": {
      const out: TextFieldNode = { type: "TextField", value: str(obj, "value") ?? "" };
      copyBase(obj, out);
      const label = str(obj, "label");
      if (label !== undefined) out.label = label;
      const aria = str(obj, "ariaLabel");
      if (aria !== undefined) out.ariaLabel = aria;
      const placeholder = str(obj, "placeholder");
      if (placeholder !== undefined) out.placeholder = placeholder;
      const onChange = str(obj, "onChange");
      if (onChange !== undefined) out.onChange = onChange;
      return out;
    }
    case "List": {
      const out: ListNode = { type: "List", items: nodeArray(obj, "items") };
      copyBase(obj, out);
      return out;
    }
  }
}

function copyBase(obj: Raw, out: { id?: string; testId?: string }): void {
  const id = str(obj, "id");
  if (id !== undefined) out.id = id;
  const testId = str(obj, "testId");
  if (testId !== undefined) out.testId = testId;
}

/**
 * Build a UI-6 `UnknownNode` from a raw wire object, mirroring Rust's
 * `Node::Unknown { type_name, props }`:
 *
 *  - `RAW` holds the ORIGINAL object verbatim (the canonicalization source of
 *    truth) — its `type` value (number / null / absent) is never touched.
 *  - The enumerable copy carries the same props for ergonomic prop access
 *    (`render`'s extended catalog reads `node.ariaLabel` etc.) plus a normalized
 *    string `type` discriminant (`""` when the wire `type` is absent/non-string)
 *    used solely for renderer routing — canonicalization ignores it in favor of
 *    `RAW`, so it can never corrupt the round-trip.
 */
function makeUnknown(obj: Raw): UnknownNode {
  const rawType = obj["type"];
  const node: UnknownNode = {
    ...obj,
    type: typeof rawType === "string" ? rawType : "",
  };
  // Stash the untouched original. A shallow copy is enough — canonicalize only
  // reads it, never mutates it.
  Object.defineProperty(node, RAW, {
    value: { ...obj },
    enumerable: false,
    writable: true,
    configurable: true,
  });
  return node;
}
