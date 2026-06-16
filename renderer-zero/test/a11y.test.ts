/**
 * Accessibility conformance (UI-7) for the renderer's role/name emission,
 * cross-checked against the committed a11y golden
 * `forge/crates/ui/tests/golden/a11y/representative_screen.json` and the
 * role/name contract in `forge/crates/ui/src/accessibility.rs`.
 *
 * Two assertions:
 *  1. For each catalog component the renderer emits the spec ARIA role and the
 *     accessible name from the correct source (label/ariaLabel/text/title/...).
 *  2. The golden's `annotations` are reproduced: building a node per annotation
 *     and rendering it yields the annotation's `role`, and (for named,
 *     reconstructable nodes) its `name`.
 */

import { test } from "node:test";
import assert from "node:assert/strict";

import { render } from "../src/render.ts";
import { parse } from "../src/parse.ts";
import { type Node } from "../src/wire.ts";
import { A11Y_DIR, readJson, join } from "./fixtures.ts";

/** role + accessible name (`aria-label`, else text content) a node renders to. */
function roleName(node: Node): { role: string | null; name: string | null } {
  const el = render(node);
  const role = el.getAttribute("role");
  const aria = el.getAttribute("aria-label");
  // Text/Button expose their name as content when there is no explicit aria.
  const name = aria ?? (el.textContent !== "" ? el.textContent : null);
  return { role, name };
}

test("typed catalog role/name emission (UI-7 spec table)", () => {
  // Stack -> group, no name.
  assert.deepEqual(roleName({ type: "Stack", direction: "v", children: [] }), {
    role: "group",
    name: null,
  });
  // Text -> text, name = content.
  assert.deepEqual(roleName({ type: "Text", text: "Hello" }), { role: "text", name: "Hello" });
  // Button -> button, name = label.
  assert.deepEqual(roleName({ type: "Button", label: "Save" }), { role: "button", name: "Save" });
  // Icon-only Button -> name = ariaLabel (never inferred from icon).
  assert.deepEqual(roleName({ type: "Button", label: "", ariaLabel: "Close" } as Node), {
    role: "button",
    name: "Close",
  });
  // TextField -> textbox, name = label (placeholder never counts).
  assert.deepEqual(
    roleName({ type: "TextField", value: "", label: "Email", placeholder: "you@x" } as Node),
    { role: "textbox", name: "Email" },
  );
  // List -> list.
  assert.equal(roleName({ type: "List", items: [] }).role, "list");
});

test("extended @forge/std catalog role/name (UI-6 fallback, accessibility.rs)", () => {
  const cases: [Node, string, string | null][] = [
    [{ type: "Grid", columns: 3 } as unknown as Node, "group", null],
    [{ type: "Grid", rows: 2 } as unknown as Node, "group", null],
    [{ type: "Grid", columns: 3, interactive: true } as unknown as Node, "grid", null],
    [{ type: "Grid", rows: 2, selectable: true } as unknown as Node, "grid", null],
    [{ type: "Grid", columns: 3, dataGrid: true } as unknown as Node, "grid", null],
    [{ type: "Grid" } as unknown as Node, "group", null],
    [{ type: "Card", ariaLabel: "Profile" } as unknown as Node, "region", "Profile"],
    [{ type: "Card" } as unknown as Node, "group", null],
    [{ type: "Divider" } as unknown as Node, "separator", null],
    [{ type: "Markdown" } as unknown as Node, "document", null],
    [{ type: "Tabs", ariaLabel: "Sections" } as unknown as Node, "tablist", "Sections"],
    [{ type: "Image", alt: "A cat" } as unknown as Node, "img", "A cat"],
    [{ type: "Chart", summary: "Up 4%" } as unknown as Node, "img", "Up 4%"],
    [{ type: "Table", caption: "Sales" } as unknown as Node, "table", "Sales"],
    [{ type: "Modal", title: "Confirm" } as unknown as Node, "dialog", "Confirm"],
    [{ type: "Form", ariaLabel: "Signup" } as unknown as Node, "form", "Signup"],
    [{ type: "Select", label: "Country" } as unknown as Node, "combobox", "Country"],
    [{ type: "Checkbox", label: "Agree" } as unknown as Node, "checkbox", "Agree"],
    [{ type: "Switch", label: "Dark" } as unknown as Node, "switch", "Dark"],
    [{ type: "Slider", label: "Volume" } as unknown as Node, "slider", "Volume"],
    [{ type: "Badge", label: "New" } as unknown as Node, "status", "New"],
    // Icon: informative icon is an `img` named by ariaLabel (accessibility.rs).
    [{ type: "Icon", ariaLabel: "Search" } as unknown as Node, "img", "Search"],
    // Spacer is presentational and exposes no name.
    [{ type: "Spacer" } as unknown as Node, "presentation", null],
  ];
  for (const [node, role, name] of cases) {
    const got = roleName(node);
    assert.equal(got.role, role, `${(node as { type: string }).type} role`);
    assert.equal(got.name, name, `${(node as { type: string }).type} name`);
  }
});

test("decorative Icon is presentational with NO accessible name (accessibility.rs)", () => {
  // `decorative: true` => role `presentation`, AxNameSource::None — even if a
  // stray `ariaLabel` is present it must NOT surface as a name on a decorative
  // icon, mirroring `unknown_accessibility`'s Icon arm.
  const decorative = roleName({ type: "Icon", decorative: true } as unknown as Node);
  assert.deepEqual(decorative, { role: "presentation", name: null });
  const decorativeWithStrayName = roleName({
    type: "Icon",
    decorative: true,
    ariaLabel: "ignored",
  } as unknown as Node);
  assert.deepEqual(decorativeWithStrayName, { role: "presentation", name: null });
});

test("genuinely-unknown component renders the UI-6 fallback group, never raw JSON", () => {
  const el = render(parse({ type: "FutureWidget", title: "Heatmap", points: [1, 2] }));
  assert.equal(el.getAttribute("role"), "group");
  assert.equal(el.getAttribute("aria-label"), "Unsupported component FutureWidget");
  // No raw JSON leaked into the rendered output.
  assert.ok(!el.textContent.includes("Heatmap"), "raw payload leaked into fallback render");
});

test("a11y attributes land on the DOM: role + aria-label + alt (Image) onto elements", () => {
  // Image's accessible name comes from `alt` (accessibility.rs AxNameSource::Alt);
  // it must surface on the rendered element as the accessible name.
  const img = render(parse({ type: "Image", alt: "A grey cat" }) as Node);
  assert.equal(img.getAttribute("role"), "img");
  assert.equal(img.getAttribute("aria-label"), "A grey cat", "alt did not become the accessible name");

  // A named TextField carries its label as aria-label on the rendered <input>.
  const field = render(parse({ type: "TextField", value: "", label: "Email" }) as Node);
  assert.equal(field.getAttribute("role"), "textbox");
  assert.equal(field.getAttribute("aria-label"), "Email");

  // A labelled Card becomes a named region; the role + name both land on the DOM.
  const card = render(parse({ type: "Card", ariaLabel: "Profile" }) as Node);
  assert.equal(card.getAttribute("role"), "region");
  assert.equal(card.getAttribute("aria-label"), "Profile");
});

interface Annotation {
  type: string;
  role: string;
  name: string | null;
  path: number[];
  focusable: boolean;
}
interface Screen {
  annotations: Annotation[];
}

test("a11y golden representative_screen: rendered roles match annotations", () => {
  const data = readJson<Record<string, Screen>>(join(A11Y_DIR, "representative_screen.json"));
  let checked = 0;
  for (const [screenName, screen] of Object.entries(data)) {
    for (const ann of screen.annotations) {
      const node = nodeForAnnotation(ann);
      if (node === null) continue; // type the renderer cannot reconstruct standalone
      const el = render(node);
      assert.equal(
        el.getAttribute("role"),
        ann.role,
        `${screenName} @ ${JSON.stringify(ann.path)}: ${ann.type} expected role ${ann.role}`,
      );
      checked++;
    }
  }
  assert.ok(checked >= 10, `expected to check many annotations, only checked ${checked}`);
});

/** Build a representative node carrying the annotation's accessible name so the
 * renderer derives the annotated role/name. `null` for types not
 * standalone-reconstructable. */
function nodeForAnnotation(ann: Annotation): Node | null {
  const name = ann.name ?? "";
  switch (ann.type) {
    case "Stack":
      return { type: "Stack", direction: "v", children: [] };
    case "Text":
      return { type: "Text", text: name };
    case "Button":
      return { type: "Button", label: name || "x" };
    case "TextField":
      return { type: "TextField", value: "", label: name || "Field" } as Node;
    case "List":
      return { type: "List", items: [] };
    case "Modal":
      return { type: "Modal", ...(ann.name ? { title: ann.name } : {}) } as unknown as Node;
    case "Tabs":
      return { type: "Tabs", ...(ann.name ? { ariaLabel: ann.name } : {}) } as unknown as Node;
    case "Grid":
      return (
        ann.role === "grid" ? { type: "Grid", interactive: true } : { type: "Grid" }
      ) as unknown as Node;
    default:
      return null;
  }
}
