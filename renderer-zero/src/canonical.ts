/**
 * Canonical wire serialization (UI-12), ported from the Rust `Serialize` impl
 * in `forge/crates/ui/src/node.rs`.
 *
 * The Rust hand-rolled serializer emits each known node's fields in a fixed
 * declaration order (`type`, then `id`/`testId`, then the type-specific props,
 * optionals omitted when absent), and re-emits an `Unknown` node's props
 * verbatim. Reproducing that order here gives us two things at once:
 *
 *  1. a *stable* serialization to fingerprint a rendered/parsed tree against a
 *     golden (the roundtrip goldens), and
 *  2. a structural equality that matches Rust `Node: PartialEq` — two trees are
 *     equal iff their canonical forms are byte-identical. Absent vs explicitly-
 *     undefined optionals collapse to the same canonical form, exactly like the
 *     Rust `Option<T>` fields.
 *
 * This is the single source of truth the conformance suite compares against, so
 * a node shape the renderer does not understand cannot silently pass.
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

type Json = string | number | boolean | null | Json[] | { [k: string]: Json };

function str(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

/** Re-emit a node into a canonical, field-ordered plain object (wire shape). */
export function canonicalize(node: Node): Json {
  if (!isKnownType(node.type)) return canonicalizeUnknown(node as UnknownNode);
  const out: { [k: string]: Json } = { type: node.type };
  // BaseNode identity comes right after `type` (Rust `serialize_into`).
  const id = str((node as { id?: unknown }).id);
  if (id !== undefined) out["id"] = id;
  const testId = str((node as { testId?: unknown }).testId);
  if (testId !== undefined) out["testId"] = testId;

  switch (node.type) {
    case "Stack": {
      const s = node as StackNode;
      out["direction"] = s.direction === "h" ? "h" : "v";
      const gap = str(s.gap);
      if (gap !== undefined) out["gap"] = gap;
      out["children"] = (s.children ?? []).map(canonicalize);
      break;
    }
    case "Text": {
      const t = node as TextNode;
      out["text"] = t.text ?? "";
      const variant = str(t.variant);
      if (variant !== undefined) out["variant"] = variant;
      break;
    }
    case "Button": {
      const b = node as ButtonNode;
      out["label"] = b.label ?? "";
      const variant = str(b.variant);
      if (variant !== undefined) out["variant"] = variant;
      const aria = str(b.ariaLabel);
      if (aria !== undefined) out["ariaLabel"] = aria;
      const onTap = str(b.onTap);
      if (onTap !== undefined) out["onTap"] = onTap;
      break;
    }
    case "TextField": {
      const f = node as TextFieldNode;
      out["value"] = f.value ?? "";
      const label = str(f.label);
      if (label !== undefined) out["label"] = label;
      const aria = str(f.ariaLabel);
      if (aria !== undefined) out["ariaLabel"] = aria;
      const placeholder = str(f.placeholder);
      if (placeholder !== undefined) out["placeholder"] = placeholder;
      const onChange = str(f.onChange);
      if (onChange !== undefined) out["onChange"] = onChange;
      break;
    }
    case "List": {
      out["items"] = ((node as ListNode).items ?? []).map(canonicalize);
      break;
    }
  }
  return out;
}

/**
 * Canonicalize an unknown node: preserve its payload **fully verbatim** (UI-6).
 *
 * The Rust `Unknown` arm stores the original object as a raw
 * `serde_json::Map`/`Value` (`node.rs`) and re-emits it unchanged on
 * serialization — it never re-decodes nested objects into typed `Node`s, so a
 * KNOWN-typed object nested inside an unknown container keeps ALL its props
 * (even ones a typed node would drop, e.g. `sparkle` on a Button). We must match
 * that: do NOT route nested `{type:...}` objects back through `canonicalize()`
 * (which would strip extra props and diverge from Rust). The only normalization
 * is key sorting, because serde_json's default `Map` is a `BTreeMap` (no
 * `preserve_order` feature in the workspace), so Rust emits object keys sorted —
 * sorting here reproduces that byte-for-byte and keeps value equality
 * order-independent.
 */
function canonicalizeUnknown(node: UnknownNode): Json {
  // Prefer the verbatim original captured by `parse` (the UI-6 source of truth):
  // it preserves a non-string/absent `type` exactly, where the enumerable copy
  // carries only a normalized string discriminant. For a hand-built unknown that
  // never went through `parse` there is no RAW, so fall back to the enumerable
  // props — dropping a synthetic empty-string `type` so a `{type:""}` routing
  // tag is not mistaken for a real wire key.
  const raw = node[RAW];
  const source: Record<string, unknown> = raw ?? stripSyntheticType(node);
  return canonicalizeVerbatim(source) as { [k: string]: Json };
}

/** A shallow copy of a hand-built unknown's enumerable props with a synthetic
 * empty-string `type` discriminant removed (a real wire `type` is kept). */
function stripSyntheticType(node: UnknownNode): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(node)) {
    if (k === "type" && v === "") continue;
    out[k] = v;
  }
  return out;
}

/** Deep-canonicalize a raw JSON value verbatim, sorting object keys (Rust
 * `serde_json::Map` is a sorted `BTreeMap`) but never reinterpreting typed
 * objects as catalog nodes. */
function canonicalizeVerbatim(v: unknown): Json {
  if (v === null) return null;
  if (Array.isArray(v)) return v.map(canonicalizeVerbatim);
  if (typeof v === "object") {
    const obj = v as Record<string, unknown>;
    const out: { [k: string]: Json } = {};
    for (const key of Object.keys(obj).sort()) out[key] = canonicalizeVerbatim(obj[key]);
    return out;
  }
  if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") return v;
  return null;
}

/** Deterministic JSON of a node's canonical wire form. */
export function canonicalJson(node: Node): string {
  return JSON.stringify(canonicalize(node));
}

/** Structural equality matching Rust `Node: PartialEq` (via canonical form). */
export function treeEqual(a: Node, b: Node): boolean {
  return canonicalJson(a) === canonicalJson(b);
}
