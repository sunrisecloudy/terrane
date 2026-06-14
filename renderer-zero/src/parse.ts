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
    // Not an object → verbatim unknown fallback (no string `type`).
    return { type: "", ...(typeof value === "object" && value !== null ? value : {}) } as UnknownNode;
  }
  const obj = value as Raw;
  const type = obj["type"];
  if (typeof type !== "string" || !isKnownType(type)) {
    // Unknown `type` (or missing/non-string) → preserve verbatim (UI-6).
    return { ...obj, type: typeof type === "string" ? type : "" } as UnknownNode;
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
