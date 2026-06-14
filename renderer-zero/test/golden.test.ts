/**
 * Golden conformance suite (UI-13/14) driven by the REAL committed corpus in
 * `forge/crates/ui/tests/golden/`. The suite asserts:
 *
 *  - roundtrip_*.json — the renderer renders the tree, and the tree's canonical
 *    wire form round-trips identically (parse -> canonical -> equals source).
 *    The rendered DOM serializes to a stable, non-empty fingerprint.
 *  - diff_*.json — render the `old` tree, apply the fixture's `expect_patches`,
 *    and assert the result equals the `new` tree (tree equality AND identical
 *    rendered DOM). Empty patch lists must leave the tree unchanged.
 *  - unknown_*.json — a tree containing a UI-6 unknown component renders and
 *    canonicalizes without throwing, preserving the unknown payload verbatim.
 *
 * The manifest is the source of truth for which fixtures exist: every manifest
 * case must map to a handled file, and every golden file must appear in the
 * manifest. An unhandled or unknown `kind` FAILS the suite — a fixture cannot
 * be silently skipped.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { parse } from "../src/parse.ts";
import { serialize } from "../src/dom.ts";
import { applyTree, applyDom, clone } from "../src/patch.ts";
import { canonicalJson, treeEqual } from "../src/canonical.ts";
import { type Node, type Patch } from "../src/wire.ts";
import { GOLDEN_DIR, listFixtures, readJson, join } from "./fixtures.ts";

interface ManifestCase {
  file: string;
  kind: "roundtrip" | "diff" | "unknown" | string;
  note?: string;
}
interface Manifest {
  cases: ManifestCase[];
}

const manifest = readJson<Manifest>(join(GOLDEN_DIR, "manifest.json"));

test("golden manifest is complete and consistent with the corpus", () => {
  const listed = new Set(manifest.cases.map((c) => c.file));
  const onDisk = new Set(listFixtures(GOLDEN_DIR, (n) => n !== "manifest.json"));
  for (const f of onDisk) {
    assert.ok(listed.has(f), `golden file ${f} is not registered in manifest.json`);
  }
  for (const c of manifest.cases) {
    assert.ok(onDisk.has(c.file), `manifest references missing golden file ${c.file}`);
  }
  assert.ok(manifest.cases.length > 0, "manifest is empty");
});

for (const c of manifest.cases) {
  test(`golden ${c.kind}: ${c.file}`, () => {
    const path = join(GOLDEN_DIR, c.file);
    const data = readJson<Record<string, unknown>>(path);

    switch (c.kind) {
      case "roundtrip":
        assertRoundtrip(parse(data["tree"]), data["tree"]);
        break;
      case "diff":
        assertDiff(
          parse(data["old"]),
          parse(data["new"]),
          (data["expect_patches"] as unknown[]).map((p) => parsePatch(p)),
        );
        break;
      case "unknown":
        assertUnknown(data["tree"]);
        break;
      default:
        assert.fail(`unhandled golden kind \`${c.kind}\` for ${c.file}`);
    }
  });
}

function assertRoundtrip(tree: Node, raw: unknown): void {
  // Canonical wire form round-trips: parse -> canonical -> re-parse -> canonical
  // is identical, and equals the canonical form of the raw source tree.
  const canon = canonicalJson(tree);
  assert.equal(canonicalJson(clone(tree)), canon, "canonical form is not stable under clone");
  assert.equal(
    canonicalJson(parse(JSON.parse(canon))),
    canon,
    "canonical wire form does not round-trip (parse∘serialize is not identity)",
  );
  assert.equal(canonicalJson(parse(raw)), canon, "parsing the raw source differs from the tree");
  // The renderer produces a non-empty DOM whose serialization is stable.
  const dom1 = serialize(render(tree));
  const dom2 = serialize(render(clone(tree)));
  assert.equal(dom1, dom2, "render output is not deterministic");
  assert.ok(dom1.length > 0, "render produced empty output");
  // The rendered root carries the wire type so the DOM is introspectable.
  assert.match(dom1, /data-forge-type="/, "rendered root missing data-forge-type");
}

/** Decode a raw patch object, parsing any embedded `node` through `parse`. */
function parsePatch(raw: unknown): Patch {
  const p = raw as Record<string, unknown>;
  if ((p["op"] === "replace" || p["op"] === "insert") && p["node"] !== undefined) {
    return { ...p, node: parse(p["node"]) } as unknown as Patch;
  }
  return p as unknown as Patch;
}

function assertDiff(oldTree: Node, newTree: Node, patches: Patch[]): void {
  // 1) Tree-level: apply the fixture's expected patches to a clone of `old`.
  const treeResult = applyTree(clone(oldTree), patches);
  assert.ok(
    treeEqual(treeResult, newTree),
    `tree mismatch after patch:\n got:  ${canonicalJson(treeResult)}\n want: ${canonicalJson(newTree)}`,
  );

  // 2) DOM-level: rendering old then applying the patches must yield the same
  //    DOM as rendering `new` from scratch.
  const dom = render(clone(oldTree));
  const { tree: domTree, dom: patchedDom } = applyDom(clone(oldTree), dom, patches);
  assert.equal(
    serialize(patchedDom),
    serialize(render(newTree)),
    "patched DOM differs from freshly-rendered new tree",
  );
  assert.ok(treeEqual(domTree, newTree), "applyDom tree diverged from new tree");

  // 3) Empty patch list is a true no-op.
  if (patches.length === 0) {
    assert.ok(treeEqual(applyTree(clone(oldTree), []), oldTree), "empty patch mutated the tree");
  }
}

function assertUnknown(raw: unknown): void {
  // 1) Decoding (UI-6) must never throw, even with an unrecognized component or
  //    an extra prop on a known node.
  let tree!: Node;
  assert.doesNotThrow(() => {
    tree = parse(raw);
  }, "parsing an unknown tree threw");

  // 2) The canonical form is a lossless, idempotent round-trip: re-parsing the
  //    canonical JSON yields the identical canonical JSON.
  const canon = canonicalJson(tree);
  assert.equal(
    canonicalJson(parse(JSON.parse(canon))),
    canon,
    "canonical form is not stable under re-parse (UI-6 round-trip broken)",
  );

  // 3) Every UNKNOWN-typed component in the raw tree survives VERBATIM: each of
  //    its props (e.g. FutureWidget.confidence/title/points) is preserved with
  //    its exact value. (Known nodes legitimately drop extra props, per serde.)
  for (const unk of collectUnknownObjects(raw)) {
    const decoded = findUnknownByType(tree, String(unk["type"]));
    assert.ok(decoded, `unknown component ${unk["type"]} was lost on decode`);
    for (const [k, v] of Object.entries(unk)) {
      assert.deepEqual(
        decoded[k],
        v,
        `unknown component ${unk["type"]} lost prop \`${k}\` (verbatim preservation, UI-6)`,
      );
    }
  }

  // 4) Rendering does not throw and yields non-empty output.
  let html = "";
  assert.doesNotThrow(() => {
    html = serialize(render(tree));
  }, "rendering an unknown tree threw");
  assert.ok(html.length > 0, "unknown render produced empty output");
}

/** Collect every raw object whose `type` is NOT a typed catalog member. */
function collectUnknownObjects(v: unknown, out: Record<string, unknown>[] = []): Record<string, unknown>[] {
  if (Array.isArray(v)) {
    for (const e of v) collectUnknownObjects(e, out);
  } else if (v !== null && typeof v === "object") {
    const obj = v as Record<string, unknown>;
    const t = obj["type"];
    if (typeof t === "string" && !["Stack", "Text", "Button", "TextField", "List"].includes(t)) {
      out.push(obj);
    }
    for (const val of Object.values(obj)) collectUnknownObjects(val, out);
  }
  return out;
}

/** Find the first decoded unknown node of `type` anywhere in `tree`. */
function findUnknownByType(node: Node, type: string): Record<string, unknown> | null {
  const obj = node as Record<string, unknown>;
  if (obj["type"] === type && !["Stack", "Text", "Button", "TextField", "List"].includes(type)) {
    return obj;
  }
  for (const child of childrenForSearch(node)) {
    const found = findUnknownByType(child, type);
    if (found) return found;
  }
  return null;
}

function childrenForSearch(node: Node): Node[] {
  const obj = node as Record<string, unknown>;
  const out: Node[] = [];
  for (const key of ["children", "items"]) {
    const arr = obj[key];
    if (Array.isArray(arr)) out.push(...(arr as Node[]));
  }
  return out;
}
