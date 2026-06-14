/**
 * The patch applier (UI-13): replay an index-path patch set against a live UI
 * `Node` tree, ported 1:1 from `forge/crates/ui/src/patch.rs` (`apply`/
 * `apply_one`/`apply_prop`).
 *
 * Addressing is index-path based: `[]` is the root, `[0]` the first child,
 * `[0,2]` the third child of the first child (Stack `children` / List `items`).
 * The five ops are `replace`, `update_text`, `update_prop`, `insert`, `remove`.
 *
 * `applyTree` mutates the tree in place and is the conformance target for the
 * `diff_*` goldens (render before → apply expected patch → equals after). A
 * malformed patch (out-of-range path, wrong target type, unknown prop key)
 * throws a `PatchError`, exactly mirroring the Rust `ValidationError`s, so an
 * unhandled case fails loudly rather than silently.
 */

import {
  type Node,
  type Patch,
  type Path,
  type UnknownNode,
  isKnownType,
} from "./wire.ts";
import { render } from "./render.ts";
import { type DomElement } from "./dom.ts";

/** A validation failure applying a patch (mirrors Rust `CoreError::ValidationError`). */
export class PatchError extends Error {
  override name = "PatchError";
}

/** Apply `patches` to `root` in order, mutating it in place. */
export function applyTree(root: Node, patches: Patch[]): Node {
  let current = root;
  for (const patch of patches) current = applyOne(current, patch);
  return current;
}

/** Apply a single patch, returning the (possibly new) root. */
function applyOne(root: Node, patch: Patch): Node {
  switch (patch.op) {
    case "replace": {
      if (patch.path.length === 0) return clone(patch.node);
      const { parent, index } = resolveParent(root, patch.path);
      setChild(parent, index, clone(patch.node), patch.path);
      return root;
    }
    case "update_text": {
      const target = resolve(root, patch.path);
      if (isKnownType(target.type) && target.type === "Text") {
        target.text = patch.value;
        return root;
      }
      throw new PatchError(
        `update_text at ${fmt(patch.path)} targets a ${target.type} node`,
      );
    }
    case "update_prop": {
      const target = resolve(root, patch.path);
      applyProp(target, patch.key, patch.value, patch.path);
      return root;
    }
    case "insert": {
      const { parent, index } = resolveParent(root, patch.path);
      const kids = childrenMut(parent, patch.path);
      if (index > kids.length) {
        throw new PatchError(`insert index ${index} out of range at ${fmt(patch.path)}`);
      }
      kids.splice(index, 0, clone(patch.node));
      return root;
    }
    case "remove": {
      const { parent, index } = resolveParent(root, patch.path);
      const kids = childrenMut(parent, patch.path);
      if (index >= kids.length) {
        throw new PatchError(`remove index ${index} out of range at ${fmt(patch.path)}`);
      }
      kids.splice(index, 1);
      return root;
    }
  }
}

/** Set a scalar prop on a known node by its wire key (mirrors `apply_prop`). */
function applyProp(target: Node, key: string, value: string, path: Path): void {
  // Shared base props apply to any known node.
  if (isKnownType(target.type)) {
    if (key === "id") {
      (target as { id?: string }).id = value;
      return;
    }
    if (key === "testId") {
      (target as { testId?: string }).testId = value;
      return;
    }
  }
  const t = target.type;
  const ok = (set: () => void): void => set();
  if (t === "Stack" && key === "gap") return ok(() => ((target as { gap?: string }).gap = value));
  if (t === "Text" && key === "variant")
    return ok(() => ((target as { variant?: string }).variant = value));
  if (t === "Button" && key === "label")
    return ok(() => ((target as { label: string }).label = value));
  if (t === "Button" && key === "variant")
    return ok(() => ((target as { variant?: string }).variant = value));
  if (t === "Button" && key === "ariaLabel")
    return ok(() => ((target as { ariaLabel?: string }).ariaLabel = value));
  if (t === "Button" && key === "onTap")
    return ok(() => ((target as { onTap?: string }).onTap = value));
  if (t === "TextField" && key === "value")
    return ok(() => ((target as { value: string }).value = value));
  if (t === "TextField" && key === "label")
    return ok(() => ((target as { label?: string }).label = value));
  if (t === "TextField" && key === "ariaLabel")
    return ok(() => ((target as { ariaLabel?: string }).ariaLabel = value));
  if (t === "TextField" && key === "placeholder")
    return ok(() => ((target as { placeholder?: string }).placeholder = value));
  if (t === "TextField" && key === "onChange")
    return ok(() => ((target as { onChange?: string }).onChange = value));
  throw new PatchError(
    `update_prop key \`${key}\` is not valid for a ${t} node at ${fmt(path)}`,
  );
}

/** Resolve a node reference at `path`, walking container children by index. */
function resolve(root: Node, path: Path): Node {
  let cur: Node = root;
  for (let depth = 0; depth < path.length; depth++) {
    const here = path.slice(0, depth + 1);
    const kids = childrenMut(cur, here);
    const idx = path[depth] as number;
    const next = kids[idx];
    if (next === undefined) {
      throw new PatchError(`path index ${idx} out of range at ${fmt(here)}`);
    }
    cur = next;
  }
  return cur;
}

/** Resolve the parent container + final index for an insert/remove/replace. */
function resolveParent(root: Node, path: Path): { parent: Node; index: number } {
  if (path.length === 0) {
    throw new PatchError("insert/remove/replace requires a non-root path");
  }
  const parentPath = path.slice(0, -1);
  const index = path[path.length - 1] as number;
  return { parent: resolve(root, parentPath), index };
}

/** The mutable child array of a container, or throw for a leaf. */
function childrenMut(node: Node, path: Path): Node[] {
  if (isKnownType(node.type)) {
    if (node.type === "Stack") return (node as { children: Node[] }).children;
    if (node.type === "List") return (node as { items: Node[] }).items;
  }
  throw new PatchError(`node at ${fmt(path)} is a leaf ${node.type} with no children`);
}

/** Overwrite the child at `index` of a container parent. */
function setChild(parent: Node, index: number, next: Node, path: Path): void {
  const kids = childrenMut(parent, path.slice(0, -1));
  if (index >= kids.length) {
    throw new PatchError(`replace index ${index} out of range at ${fmt(path)}`);
  }
  kids[index] = next;
}

function fmt(path: Path): string {
  return `[${path.join(", ")}]`;
}

/** A structural deep clone of a node (so patches never alias the source tree). */
export function clone<T>(node: T): T {
  return structuredClone(node) as T;
}

// --- DOM-incremental applier --------------------------------------------

/**
 * Apply patches to BOTH a live tree and its rendered DOM, keeping them in sync.
 *
 * The tree is the authoritative model (the conformance target); after each patch
 * mutates it, the DOM is re-derived from the now-authoritative tree so the
 * returned `{ tree, dom }` always satisfies `render(tree)` ≡ `dom`. This is a
 * correctness-first ("render-from-truth") strategy, not an incremental DOM
 * diff: each op re-renders from the tree rather than surgically mutating the
 * existing DOM. That keeps DOM/tree equivalence trivially guaranteed against the
 * goldens without re-implementing every op against the DOM's list-item and
 * extended-catalog wrapper structure; a production renderer would specialize the
 * scalar ops (`update_text`/`update_prop`) into in-place element mutations.
 */
export function applyDom(
  tree: Node,
  dom: DomElement,
  patches: Patch[],
): { tree: Node; dom: DomElement } {
  let curTree = tree;
  let curDom = dom;
  for (const patch of patches) {
    curTree = applyOne(curTree, patch);
    curDom = render(curTree);
  }
  return { tree: curTree, dom: curDom };
}

/** Locate the DOM element addressed by a render-tree index path (best-effort). */
export function domAt(root: DomElement, path: Path): DomElement | null {
  let cur: DomElement | null = root;
  for (const idx of path) {
    if (cur === null) return null;
    // List items wrap each child in an <li>; unwrap to the rendered child.
    const kids = renderChildren(cur);
    cur = kids[idx] ?? null;
  }
  return cur;
}

/** The rendered-child elements of a container element, unwrapping List `<li>`. */
function renderChildren(el: DomElement): DomElement[] {
  if (el.getAttribute("data-forge-type") === "List") {
    return el.childElements.map((li) => li.childElements[0]).filter((c): c is DomElement => !!c);
  }
  return el.childElements;
}

/** Re-export so callers can build unknown nodes without importing wire twice. */
export type { UnknownNode };
