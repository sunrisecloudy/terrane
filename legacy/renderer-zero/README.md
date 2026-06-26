# renderer-zero

A small **TypeScript reference renderer + patch applier** for the Terrane / forge
declarative UI wire format (UI-13/14). It turns a UI `Node` tree (the exact JSON
shape defined by `forge/crates/ui/src/node.rs`) into a DOM subtree, and applies
index-path patches (`forge/crates/ui/src/patch.rs`) to keep that DOM in sync —
proven correct against the **real committed golden corpus**, not copies.

It is a *reference* implementation: deliberately minimal, dependency-light, and
1:1 with the Rust ground truth so it doubles as executable documentation of the
wire contract and as a conformance harness any other renderer can be checked
against.

## Why this exists (UI-13/14)

The forge core emits a serialized component tree and, on every event, a minimal
patch set. A host renderer must (a) materialize that tree faithfully and (b)
replay patches by index path. renderer-zero is the smallest faithful answer,
and its test suite *is* the conformance bar: it reads the committed fixtures and
fails if any one is unhandled.

## Layout

```
renderer-zero/
  package.json        type:module, pinned devDeps (typescript, @types/node)
  tsconfig.json       strict, ES2022, allowImportingTsExtensions
  .gitignore          node_modules/, dist/
  README.md           this file
  src/
    wire.ts           Node + Patch TypeScript types (ported from node.rs/patch.rs)
    parse.ts          wire decoder (UI-6): known nodes drop extra props,
                      unknown types preserved verbatim — mirrors serde deserialize
    render.ts         the renderer: every typed node + the full @forge/std catalog
    patch.ts          the patch applier: 5 ops, index-path addressing, DOM sync
    canonical.ts      canonical wire serialization + structural tree equality
    events.ts         event EMISSION: a gesture on a rendered control emits
                      { action, payload } to a host dispatch callback (UI-4/CR-6)
    focus.ts          UI-7 focus order: compute + apply tabindex / initial focus,
                      ported 1:1 from forge/crates/ui/src/focus.rs
    dom.ts            a minimal, deterministic, dependency-free DOM shim
    index.ts          public barrel
  test/
    fixtures.ts       loads the REAL committed forge fixtures by path
    golden.test.ts    drives forge/crates/ui/tests/golden/* (roundtrip/diff/unknown)
    a11y.test.ts      drives .../golden/a11y/representative_screen.json (roles/names)
    focus.test.ts     drives .../golden/a11y/representative_screen.json focus_order
                      (stops/kind/initial focus/traps_focus) + tabindex application
    events.test.ts    drives forge/fixtures/ui-events/* (event dispatch -> patches)
    emit.test.ts      drives forge/fixtures/ui-events/* (gesture -> EMITTED event)
    fallback.test.ts  drives .../golden/unknown_*.json (UI-6 fallback render)
    patch.test.ts     unit coverage of every op + every validation error path
```

## Running

No build step is required — Node (>= 22.6) runs the TypeScript directly via
native type-stripping.

```sh
npm test          # node --test over test/**/*.test.ts (zero runtime deps)
npm run typecheck # tsc --noEmit, strict
```

The only dependencies are dev-only and pinned: `typescript` (typecheck) and
`@types/node` (editor/typecheck types). The renderer and its tests have **zero
runtime dependencies** — the DOM is a hand-rolled shim (`src/dom.ts`), chosen
over jsdom so serialization is byte-stable across Node versions and golden
comparisons are reproducible.

## Components covered (every catalog member)

**Typed catalog** (rendered natively from `node.rs` variants):

| Node        | Element             | role      | accessible name        |
|-------------|---------------------|-----------|------------------------|
| `Stack`     | `<div>`             | group     | —                      |
| `Text`      | `<span>`            | text      | text content           |
| `Button`    | `<button>`          | button    | label, else ariaLabel  |
| `TextField` | `<input>`           | textbox   | label, else ariaLabel  |
| `List`      | `<ul>` of `<li>`    | list      | —                      |

**Extended `@forge/std` catalog** — these reach an M0a renderer as UI-6
forward-compatible (unknown-tagged) objects on the wire; renderer-zero
recognizes each by name and renders a semantically correct element with the
ARIA role/name from `forge/crates/ui/src/accessibility.rs`, while preserving the
payload verbatim:

`Grid` (group/grid when interactive), `Card`, `Scroll`, `Spacer`, `Divider`,
`Markdown`, `Tabs`, `Icon`, `Image`, `Chart`, `Table`, `Modal`, `Form`,
`TextArea`, `Select`, `MultiSelect`, `Checkbox`, `Switch`, `Slider`,
`DatePicker`, `Badge`, `Stat`.

Where a component's role/name depends on its props, renderer-zero follows
`accessibility.rs` exactly — e.g. a decorative `Icon` (`decorative: true`) is
`presentation` with **no** accessible name (a stray `ariaLabel` is ignored),
while an informative `Icon` is `img` named by `ariaLabel`; `Card`/`Scroll`
become `region` only once labelled, and `Spacer` is presentational.

**Unknown fallback** (UI-6, NORMATIVE): a genuinely unrecognized `type` renders
the spec's "Unknown Component Fallback" — a labelled `group` reading
`Unsupported component <Type>`, never the raw JSON — and round-trips losslessly.
An unknown node's payload is preserved **fully verbatim**: like Rust's
`Node::Unknown { type_name, props }` (which keeps the original object as raw
`serde_json` and a separate string discriminant), a known node nested *inside* an
unknown container is **not** re-decoded, so it retains every prop a typed node
would otherwise drop. The fidelity extends to the `type` field itself: Rust reads
`type` with `as_str()`, so a **non-string** `type` (e.g. the number `42`) or a
**missing** `type` key survives untouched — never coerced to `""` and never
injected. renderer-zero mirrors this by carrying the untouched original object
alongside a normalized string `type` discriminant used only for renderer routing;
canonicalization reads the verbatim original, so the round-trip is exact (and
survives `clone`). Canonicalization only sorts object keys, matching serde_json's
default `BTreeMap` ordering (the workspace does not enable `preserve_order`).

## Patch ops covered (all five, per `patch.rs`)

`replace`, `update_text`, `update_prop`, `insert`, `remove` — addressed by index
path (`[]` root, `[0]` first child, `[0,2]` third child of first child) over
Stack `children` / List `items`. `update_prop` accepts every scalar key the Rust
applier accepts (`id`, `testId`, `gap`, `variant`, `label`, `value`, `ariaLabel`,
`placeholder`, `onTap`, `onChange`). Out-of-range paths, wrong-target ops
(`update_text` on a Button), and unknown prop keys raise a `PatchError` exactly
like the Rust `ValidationError`s.

`applyTree` mutates the live tree (the conformance target); `applyDom` keeps a
rendered DOM in sync so `render(tree)` ≡ `dom` after every op. `applyDom` uses a
correctness-first "render-from-truth" strategy: after each op mutates the
authoritative tree, the DOM is re-derived from it (not surgically diffed), which
guarantees the equivalence invariant without re-implementing every op against
the DOM's wrapper structure.

## Conformance corpus asserted

Every fixture below is read from its committed location and asserted; a fixture
whose `kind` is unhandled **fails** (it can never be silently skipped), and each
manifest is cross-checked against the files on disk.

| Source                                                   | fixtures | what is asserted                                        |
|----------------------------------------------------------|----------|---------------------------------------------------------|
| `forge/crates/ui/tests/golden/roundtrip_*.json`          | 7        | parse → canonical wire form round-trips; render is stable & typed |
| `forge/crates/ui/tests/golden/diff_*.json`               | 10       | render `old`, apply `expect_patches`, equals `new` (tree + DOM) |
| `forge/crates/ui/tests/golden/unknown_*.json`            | 3        | UI-6 fallback renders a safe placeholder, preserves the known subtree, never leaks raw JSON, round-trips verbatim |
| `forge/crates/ui/tests/golden/a11y/representative_screen.json` (roles/names) | 1 (3 screens, 18 annotations) | rendered roles/names match the a11y golden |
| `forge/crates/ui/tests/golden/a11y/representative_screen.json` (focus order) | 1 (3 screens) | computed `focus_order` matches the golden stop-for-stop (path/kind/role/name), `initial_focus` + `traps_focus` match, and `applyFocus` writes `tabindex` / initial focus onto the DOM |
| `forge/fixtures/ui-events/*.json` (patch application)    | 12       | event ActionRef → expected patches → final tree (dispatch/error/replay) |
| `forge/fixtures/ui-events/*.json` (event emission)       | 12       | a gesture on the rendered control EMITS `{ action, payload }` to the host dispatch — exactly the vector's event; rejection/throwing is the core's job after this faithful emit |

**20** golden UI fixtures + the a11y screen golden (driven twice: roles/names
*and* focus order) + **12** ui-event vectors (driven twice: patch application
*and* event emission) = **33 committed fixture files** driving the suite
(**82** test cases total, including unit/regression coverage of the patch ops,
the decorative-Icon a11y rule, UI-6 verbatim nesting, UI-6 verbatim `type`
fidelity for non-string/absent `type`, and the renderer/core layering of
emission vs dispatch), all green.

### The three loops this renderer closes (UI-4 / UI-6 / UI-7)

1. **Event emission (UI-4 / CR-6)** — `src/events.ts`. `fireEvent` / `bindEvents`
   turn a `tap`/`change` gesture on a rendered Button/TextField into a serialized
   `{ action: ActionRef, payload }` handed to a host dispatch callback. A control
   with no matching ActionRef (a Text, a Button with no `onTap`, or an action
   absent from the tree) emits **nothing** — never an error, never a `null`-action
   emit. The renderer only *serializes and routes* the event; whether the action
   is then accepted, payload-validated, or the handler throws is the **core's**
   concern, which is why the renderer still emits faithfully for the
   `invalid_payload` / `handler_throws` vectors and lets the core reject.
2. **UI-6 unknown fallback** — `src/render.ts`. An unrecognized `type` renders the
   spec's "Unknown Component Fallback": a labelled `group` reading
   `Unsupported component <Type>`, never the raw JSON — and it **preserves the
   subtree**, rendering any KNOWN descendant under `children`/`items` so
   accessibility/content is never lost behind the placeholder. A non-string or
   absent `type` never crashes the renderer.
3. **A11y + focus (UI-7)** — `src/focus.ts`, ported 1:1 from `focus.rs`. The
   renderer emits role / `aria-label` / `alt` (Image) onto each DOM node, and the
   focus layer computes the deterministic focus order — Stack/Grid source order,
   Tabs tablist-then-active-panel, open-Modal containment (`traps_focus` + initial
   focus on the first focusable child) — with each stop kind-tagged (`Element` vs
   `Tab`) so a tab and a rendered child at the same numeric path never collide.
   `applyFocus` writes a roving `tabindex` (`0` on the initial focus, `-1`
   elsewhere) and marks the initial focus / focus trap on the DOM.

## Relationship to the Rust ground truth

- `wire.ts` / `parse.ts` / `canonical.ts` mirror `node.rs` (field order, optional
  omission, the `Unknown` fallback, serde's drop-extra-props-on-known-nodes).
- `patch.ts` mirrors `patch.rs` (the five ops, index-path resolution, the exact
  validation-error conditions).
- `render.ts` role/name emission mirrors `accessibility.rs`.
- `focus.ts` mirrors `focus.rs` (the per-container focus-order rules, Modal
  containment, the `FocusStopKind` Element/Tab disambiguation).
- `events.ts` realizes the renderer half of UI-4/CR-6: emission of the wire
  `Button.onTap` / `TextField.onChange` ActionRef to a host dispatcher.

If the Rust contract changes, these files are the single place to update, and the
fixtures will tell you immediately.
