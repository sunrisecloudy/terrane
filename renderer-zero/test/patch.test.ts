/**
 * Unit coverage for the patch applier's op semantics and validation, mirroring
 * the error paths in `forge/crates/ui/src/patch.rs` (`apply_one`/`apply_prop`/
 * `resolve_mut`/`children_mut`). Every op and every documented failure mode is
 * exercised so a regression in the applier fails loudly.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { serialize } from "../src/dom.ts";
import { applyTree, applyDom, PatchError, clone } from "../src/patch.ts";
import { canonicalJson, treeEqual } from "../src/canonical.ts";
import { type Node } from "../src/wire.ts";

function stack(...children: Node[]): Node {
  return { type: "Stack", direction: "v", children };
}

test("replace at root swaps the whole tree", () => {
  const tree = applyTree(stack({ type: "Text", text: "a" }), [
    { op: "replace", path: [], node: { type: "Text", text: "b" } },
  ]);
  assert.ok(treeEqual(tree, { type: "Text", text: "b" }));
});

test("replace at a child path swaps that subtree", () => {
  const tree = applyTree(stack({ type: "Text", text: "x" }), [
    { op: "replace", path: [0], node: { type: "Button", label: "Go", onTap: "go" } },
  ]);
  assert.ok(treeEqual(tree, stack({ type: "Button", label: "Go", onTap: "go" })));
});

test("update_text mutates a Text node", () => {
  const tree = applyTree(stack({ type: "Text", text: "old" }), [
    { op: "update_text", path: [0], value: "new" },
  ]);
  assert.ok(treeEqual(tree, stack({ type: "Text", text: "new" })));
});

test("update_text on a non-Text node throws PatchError", () => {
  assert.throws(
    () => applyTree(stack({ type: "Button", label: "B" }), [{ op: "update_text", path: [0], value: "x" }]),
    PatchError,
  );
});

test("update_prop sets every scalar prop the Rust applier accepts", () => {
  const t = applyTree(
    stack({ type: "Button", label: "A" }, { type: "TextField", value: "" }),
    [
      { op: "update_prop", path: [0], key: "label", value: "Save" },
      { op: "update_prop", path: [0], key: "onTap", value: "save" },
      { op: "update_prop", path: [0], key: "variant", value: "primary" },
      { op: "update_prop", path: [0], key: "ariaLabel", value: "Save it" },
      { op: "update_prop", path: [0], key: "testId", value: "save-btn" },
      { op: "update_prop", path: [1], key: "value", value: "Ada" },
      { op: "update_prop", path: [1], key: "label", value: "Name" },
      { op: "update_prop", path: [1], key: "placeholder", value: "..." },
      { op: "update_prop", path: [1], key: "onChange", value: "name.change" },
    ],
  );
  const btn = (t as { children: Node[] }).children[0] as Record<string, unknown>;
  assert.equal(btn["label"], "Save");
  assert.equal(btn["onTap"], "save");
  assert.equal(btn["variant"], "primary");
  assert.equal(btn["ariaLabel"], "Save it");
  assert.equal(btn["testId"], "save-btn");
  const tf = (t as { children: Node[] }).children[1] as Record<string, unknown>;
  assert.equal(tf["value"], "Ada");
  assert.equal(tf["placeholder"], "...");
  assert.equal(tf["onChange"], "name.change");
});

test("update_prop with a key invalid for the node type throws", () => {
  assert.throws(
    () => applyTree(stack({ type: "Text", text: "t" }), [{ op: "update_prop", path: [0], key: "onTap", value: "x" }]),
    PatchError,
  );
});

test("insert/remove operate by final index; out-of-range throws", () => {
  const inserted = applyTree(stack({ type: "Text", text: "a" }), [
    { op: "insert", path: [1], node: { type: "Text", text: "b" } },
  ]);
  assert.ok(treeEqual(inserted, stack({ type: "Text", text: "a" }, { type: "Text", text: "b" })));

  const removed = applyTree(clone(inserted), [{ op: "remove", path: [0] }]);
  assert.ok(treeEqual(removed, stack({ type: "Text", text: "b" })));

  assert.throws(() => applyTree(clone(inserted), [{ op: "remove", path: [5] }]), PatchError);
  assert.throws(() => applyTree(clone(inserted), [{ op: "insert", path: [9], node: { type: "Text", text: "z" } }]), PatchError);
});

test("addressing a child of a leaf node throws", () => {
  assert.throws(
    () => applyTree({ type: "Text", text: "leaf" }, [{ op: "update_text", path: [0], value: "x" }]),
    PatchError,
  );
});

test("out-of-range path index throws", () => {
  assert.throws(
    () => applyTree(stack({ type: "Text", text: "a" }), [{ op: "update_text", path: [3], value: "x" }]),
    PatchError,
  );
});

test("applyDom keeps the DOM equal to render(tree) after each op", () => {
  const initial = stack(
    { type: "Text", text: "One" },
    { type: "Button", label: "Go", onTap: "go" },
  );
  const dom = render(clone(initial));
  const { tree, dom: patched } = applyDom(clone(initial), dom, [
    { op: "update_text", path: [0], value: "Two" },
    { op: "insert", path: [2], node: { type: "Text", text: "Three" } },
    { op: "remove", path: [1] },
  ]);
  const expected = stack({ type: "Text", text: "Two" }, { type: "Text", text: "Three" });
  assert.ok(treeEqual(tree, expected));
  assert.equal(serialize(patched), serialize(render(expected)));
});

test("canonical form equals Rust wire field order (type, base, then props)", () => {
  const json = canonicalJson({
    type: "Button",
    id: "b1",
    testId: "t1",
    label: "Save",
    variant: "primary",
    ariaLabel: "Save",
    onTap: "save",
  } as Node);
  assert.equal(
    json,
    '{"type":"Button","id":"b1","testId":"t1","label":"Save","variant":"primary","ariaLabel":"Save","onTap":"save"}',
  );
});
