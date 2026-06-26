/**
 * UI-6 unknown-component fallback conformance (NORMATIVE), driven by the REAL
 * committed `forge/crates/ui/tests/golden/unknown_*.json` goldens.
 *
 * The spec's "Unknown Component Fallback" rule: a component whose `type` is not
 * in the renderer's catalog must render a SAFE placeholder that
 *  (a) does NOT crash the renderer,
 *  (b) does NOT leak the raw payload JSON into the output,
 *  (c) PRESERVES the subtree — any KNOWN descendant (Text/Button/...) carried by
 *      the unknown still renders, so accessibility/content is never lost, and
 *  (d) round-trips the unknown payload verbatim (the canonical form is the same
 *      before and after a parse cycle).
 *
 * This complements `golden.test.ts` (which asserts the parse/canonical
 * round-trip of the same fixtures) by asserting the RENDER fallback specifically
 * — the DOM a host would actually show for a future/unknown component.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { parse } from "../src/parse.ts";
import { serialize, type DomElement } from "../src/dom.ts";
import { type Node } from "../src/wire.ts";
import { GOLDEN_DIR, readJson, join } from "./fixtures.ts";

const KNOWN = new Set(["Stack", "Text", "Button", "TextField", "List"]);

/** Find the first rendered element carrying `data-forge-type === type`. */
function findByForgeType(el: DomElement, type: string): DomElement | null {
  if (el.getAttribute("data-forge-type") === type) return el;
  for (const child of el.childElements) {
    const found = findByForgeType(child, type);
    if (found) return found;
  }
  return null;
}

/** Collect every raw object whose `type` is NOT a typed catalog member. */
function collectUnknown(v: unknown, out: Record<string, unknown>[] = []): Record<string, unknown>[] {
  if (Array.isArray(v)) {
    for (const e of v) collectUnknown(e, out);
  } else if (v !== null && typeof v === "object") {
    const obj = v as Record<string, unknown>;
    const t = obj["type"];
    if (typeof t === "string" && !KNOWN.has(t)) out.push(obj);
    for (const val of Object.values(obj)) collectUnknown(val, out);
  }
  return out;
}

interface UnknownFixture {
  kind: string;
  tree: unknown;
  must_not_error?: boolean;
}

const UNKNOWN_FILES = [
  "unknown_button_extra_prop.json",
  "unknown_future_widget_child.json",
  "unknown_nested_in_list.json",
];

for (const file of UNKNOWN_FILES) {
  test(`UI-6 fallback golden ${file}: renders safely and preserves the subtree`, () => {
    const fx = readJson<UnknownFixture>(join(GOLDEN_DIR, file));
    assert.equal(fx.kind, "unknown", `${file} is not an unknown-kind fixture`);

    // (a) Rendering must not throw.
    let dom!: DomElement;
    assert.doesNotThrow(() => {
      dom = render(parse(fx.tree));
    }, `${file}: rendering an unknown tree threw`);
    const html = serialize(dom);
    assert.ok(html.length > 0, `${file}: empty render output`);

    // (b)+(c) Each genuinely-unknown component (a non-catalog `type`) renders the
    // spec fallback: a labelled `group` reading "Unsupported component <Type>",
    // NEVER its raw payload values.
    for (const unk of collectUnknown(fx.tree)) {
      const type = String(unk["type"]);
      const el = findByForgeType(dom, type);
      assert.ok(el, `${file}: unknown component ${type} produced no fallback element`);
      assert.equal(el!.getAttribute("role"), "group", `${file}: ${type} fallback role`);
      assert.equal(
        el!.getAttribute("aria-label"),
        `Unsupported component ${type}`,
        `${file}: ${type} fallback accessible name`,
      );
      assert.equal(el!.getAttribute("data-forge-unknown"), "true", `${file}: ${type} not marked unknown`);
      // No raw payload value leaked into the rendered output. Check string
      // props the goldens carry (title/range), which must never appear as text.
      for (const [k, v] of Object.entries(unk)) {
        if (k === "type") continue;
        if (typeof v === "string") {
          assert.ok(!html.includes(v), `${file}: ${type} leaked raw prop \`${k}\`=${JSON.stringify(v)} into render`);
        }
      }
    }
  });
}

test("UI-6 fallback preserves a KNOWN child rendered inside an unknown container", () => {
  // A FutureWidget nested in a Stack: the unknown is itself a typed Stack child,
  // but a genuinely-unknown *container* must still render its known descendants.
  // Here we use an unknown container directly to prove subtree preservation.
  const tree = parse({
    type: "FuturePanel",
    children: [
      { type: "Text", text: "Still visible" },
      { type: "Button", label: "Still tappable", onTap: "x" },
    ],
  });
  const dom = render(tree);
  // The container is the fallback group...
  assert.equal(dom.getAttribute("role"), "group");
  assert.equal(dom.getAttribute("aria-label"), "Unsupported component FuturePanel");
  assert.equal(dom.getAttribute("data-forge-unknown"), "true");
  // ...and its KNOWN children still render with their real roles (accessibility
  // is never lost behind an unknown wrapper).
  const text = dom.childElements.find((c) => c.getAttribute("data-forge-type") === "Text");
  const button = dom.childElements.find((c) => c.getAttribute("data-forge-type") === "Button");
  assert.ok(text, "known Text child was dropped by the fallback");
  assert.equal(text!.textContent, "Still visible");
  assert.ok(button, "known Button child was dropped by the fallback");
  assert.equal(button!.getAttribute("role"), "button");
  assert.equal(button!.getAttribute("data-action-tap"), "x");
});

test("UI-6 fallback does not crash on a non-string or absent `type`", () => {
  // node.rs reads `type` via as_str(); a numeric or missing `type` falls through
  // to the Unknown arm. The renderer must render a fallback, not throw.
  for (const raw of [{ type: 42, x: 1 }, { foo: "bar" }, [1, 2, 3], "scalar", null]) {
    assert.doesNotThrow(() => {
      const html = serialize(render(parse(raw as unknown)));
      assert.ok(html.length > 0);
    }, `rendering ${JSON.stringify(raw)} threw`);
  }
});

test("UI-6: the genuine fallback never reaches the extended @forge/std catalog", () => {
  // A recognized extended-catalog name (e.g. Card) is NOT the genuine fallback —
  // it renders its own semantic element. Only a name outside BOTH the typed and
  // extended catalogs gets "Unsupported component".
  const card = render(parse({ type: "Card", ariaLabel: "Profile" }) as Node);
  assert.notEqual(card.getAttribute("data-forge-unknown"), "true");
  assert.equal(card.getAttribute("role"), "region");

  const genuine = render(parse({ type: "TotallyMadeUp" }) as Node);
  assert.equal(genuine.getAttribute("data-forge-unknown"), "true");
  assert.equal(genuine.getAttribute("aria-label"), "Unsupported component TotallyMadeUp");
});
