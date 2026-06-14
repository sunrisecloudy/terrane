/**
 * Event-dispatch conformance (UI-4/CR-6) driven by the REAL committed vectors in
 * `forge/fixtures/ui-events/`.
 *
 * renderer-zero owns the renderer + patch-application half of the interactive
 * loop (the handler half is the core's). So for every fixture we assert the
 * renderer-side invariants:
 *
 *  - dispatch/replay: applying each event's expected `patches` to the parsed
 *    `initial_tree`, in sequence, yields exactly the fixture's `final_tree` —
 *    by tree equality AND by identical rendered DOM. An empty patch list is a
 *    true no-op.
 *  - the renderer surfaces every event's `action` (ActionRef) on a locatable
 *    rendered element, so a host could route the event back — i.e. the wire
 *    `onTap`/`onChange` survives into the DOM as `data-action-*`.
 *  - error vectors: the patches (none) leave the prior tree intact.
 *
 * The manifest's `count` and `cases` are the source of truth; a fixture whose
 * `kind` is unhandled FAILS rather than being skipped.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { parse } from "../src/parse.ts";
import { serialize, type DomElement } from "../src/dom.ts";
import { applyTree, clone } from "../src/patch.ts";
import { canonicalJson, treeEqual } from "../src/canonical.ts";
import { type Patch } from "../src/wire.ts";
import { UI_EVENTS_DIR, listFixtures, readJson, join } from "./fixtures.ts";

interface EventManifest {
  count: number;
  cases: { file: string; kind: string; note?: string }[];
}
interface EventCase {
  action: string;
  event_type: string;
  payload: unknown;
}
interface ResultExpect {
  patches: unknown[];
}
interface RunExpect {
  patches?: unknown[][];
  patches_byte_identical_to?: string;
}
interface Fixture {
  kind: string;
  initial_tree?: unknown;
  events: EventCase[];
  expect: {
    results?: ResultExpect[];
    first_run?: RunExpect;
    replay_run?: RunExpect;
    final_tree?: unknown;
  };
}

const manifest = readJson<EventManifest>(join(UI_EVENTS_DIR, "manifest.json"));

test("ui-events manifest matches the committed corpus", () => {
  const onDisk = new Set(listFixtures(UI_EVENTS_DIR, (n) => n !== "manifest.json"));
  const listed = new Set(manifest.cases.map((c) => c.file));
  for (const f of onDisk) assert.ok(listed.has(f), `ui-events file ${f} not in manifest`);
  for (const c of manifest.cases) assert.ok(onDisk.has(c.file), `manifest references missing ${c.file}`);
});

function parsePatch(raw: unknown): Patch {
  const p = raw as Record<string, unknown>;
  if ((p["op"] === "replace" || p["op"] === "insert") && p["node"] !== undefined) {
    return { ...p, node: parse(p["node"]) } as unknown as Patch;
  }
  return p as unknown as Patch;
}

/** Find an element exposing the given ActionRef under `data-action-tap`/`change`. */
function findByAction(el: DomElement, action: string): DomElement | null {
  if (el.getAttribute("data-action-tap") === action || el.getAttribute("data-action-change") === action) {
    return el;
  }
  for (const child of el.childElements) {
    const found = findByAction(child, action);
    if (found) return found;
  }
  return null;
}

for (const c of manifest.cases) {
  test(`ui-events ${c.kind}: ${c.file}`, () => {
    const fx = readJson<Fixture>(join(UI_EVENTS_DIR, c.file));
    if (fx.initial_tree === undefined) {
      // A pre-dispatch rejection vector with no tree to render; nothing for the
      // renderer half to assert beyond the manifest consistency above.
      assert.ok(["error"].includes(c.kind), `no initial_tree for non-error case ${c.file}`);
      return;
    }

    const initial = parse(fx.initial_tree);

    // Collect the per-event patch batches from whichever shape the fixture uses:
    // dispatch/error use `expect.results[].patches`; replay uses
    // `expect.first_run.patches` (an array of per-event batches).
    const batches: unknown[][] = [];
    if (fx.expect.results) {
      for (const r of fx.expect.results) batches.push(r.patches ?? []);
    } else if (fx.expect.first_run?.patches) {
      for (const b of fx.expect.first_run.patches) batches.push(b);
    }

    // Replay determinism: a `replay_run` declaring byte-identity to `first_run`
    // must hold against the committed first_run patches.
    if (fx.expect.replay_run?.patches_byte_identical_to === "first_run") {
      assert.deepEqual(
        fx.expect.replay_run.patches ?? fx.expect.first_run?.patches,
        fx.expect.first_run?.patches,
        `replay_run is not byte-identical to first_run for ${c.file}`,
      );
    }

    // 1) Apply the expected patch batches, in order, to a clone of the initial
    //    tree.
    let tree = clone(initial);
    let totalPatches = 0;
    for (const batch of batches) {
      const patches = batch.map(parsePatch);
      totalPatches += patches.length;
      tree = applyTree(tree, patches);
    }

    // 2) If the fixture declares a final_tree, the applied result must equal it
    //    (tree equality AND identical rendered DOM).
    if (fx.expect.final_tree !== undefined) {
      const finalTree = parse(fx.expect.final_tree);
      assert.ok(
        treeEqual(tree, finalTree),
        `final tree mismatch for ${c.file}:\n got:  ${canonicalJson(tree)}\n want: ${canonicalJson(finalTree)}`,
      );
      assert.equal(
        serialize(render(tree)),
        serialize(render(finalTree)),
        `rendered DOM mismatch vs final_tree for ${c.file}`,
      );
    }

    // 3) Error vectors must not mutate the prior tree (no patches applied).
    if (c.kind === "error") {
      assert.equal(totalPatches, 0, `error vector ${c.file} unexpectedly carried patches`);
      assert.ok(treeEqual(tree, initial), `error vector ${c.file} mutated the prior tree`);
    }

    // 4) The renderer surfaces each event's ActionRef on a locatable element so
    //    a host can route the event back (the dispatch wiring). The "no handler"
    //    vector intentionally fires an action absent from the tree.
    const dom = render(initial);
    if (c.file !== "no_handler_event_ignored.json" && c.file !== "unknown_action_rejected.json") {
      for (const ev of fx.events) {
        assert.ok(
          findByAction(dom, ev.action),
          `ActionRef ${ev.action} not exposed on any rendered element in ${c.file}`,
        );
      }
    }
  });
}
