# PRD 05 — UI System (declarative component-tree protocol + platform app surfaces)

**Status:** Merged draft v1 · **Depends on:** 01, 02 · **Depended on by:** 06
**Sources:** F-05 (Forge UI protocol, catalog, renderers, conformance kit) + P-11 (editor, schema designer, data browser, permission/time-travel/LLM UX) + decisions D1 (headless-first), D2 (declarative tree + renderer zero)

Two distinct UI layers, kept separate on purpose:
**A. Applet UI** — what generated code renders: a declarative component tree, never platform APIs.
**B. Platform app UI** — the workshop around it (editor, data browser, permission prompts): built by us, shell-native, specified in §6.

## A. Applet UI protocol

## 1. Requirements

- **UI-1** Applets emit a typed component tree via `ctx.ui.render(tree)`; the core diffs successive trees and ships minimal patches to the renderer over the standard Stream API (CR-A1). Applets never touch platform UI APIs; the sandbox exposes no DOM/webview surface.
- **UI-2** v1 catalog (~26, the LLM's whole vocabulary): Layout — `Stack(h/v), Grid, Scroll, Spacer, Divider, Card`; Content — `Text, Icon, Image, Badge, Markdown`; Input — `Button, TextField, TextArea, Select, MultiSelect, Checkbox, Switch, Slider, DatePicker`; Data — `List` (virtualized), `Table` (sort/select), `Chart` (line/bar/pie/scatter), `Stat`; Structure — `Tabs, Modal, Form` (validation states).
- **UI-3** Components are **semantic, not pixel-specified**: variants (`primary/secondary/destructive`), sizes, intent colors; concrete styling is the shell theme's job — applets automatically match platform look, dark mode, accessibility settings.
- **UI-4** State & events: handlers (`onTap, onChange, onSubmit`) route through the core event queue (CR-6); controlled-input pattern; `db.watch` + re-render is the standard data-binding loop. Input → patched frame: < 16 ms p95 desktop, < 33 ms web/mobile.
- **UI-5** Lists/tables virtualize in the **shell** (native lazy lists): the applet supplies a query handle; the shell pulls visible rows via the core. 100k-row tables stay smooth without manual paging.
- **UI-6** **Forward compatibility (normative, tested):** versioned wire format; unknown component types render as a labeled fallback box with `Text`-coercible props; unknown props ignored, never errors; unknown registry fields render via the same fallback (DL-9). A v1 client renders a v3 applet usably degraded.
- **UI-7** Accessibility: every component maps to platform a11y primitives (labels, traits, focus order); `Form` label presence enforced by std types at type-check; WCAG 2.1 AA contrast for built-in themes; a11y audit is a GA gate.
- **UI-8** Theming: workspace tokens (accent, radius, density) applied shell-side; applets read tokens, never define raw colors (escape hatch: `Chart` palettes).
- **UI-9** Navigation: applet declares `pages`; shell owns chrome (back, titles, deep links `forge://ws/<id>/applet/<id>/page`).
- **UI-10** Presence affordances built-in: avatars row + per-record "being edited by" badge from the presence channel (SS-1) — collaboration UI for free.
- **UI-11** Script outputs (CR-8) render through the same protocol: a run's `Result` maps to `Text/Markdown/Table/Card` trees (P-11 "JSON/text/table/card outputs"), so the harness, logs view, and applet surfaces share one renderer path.

## 2. Headless contract & renderer zero (decisions D1/D2)

- **UI-12** The tree + patch + event wire format is specified and versioned **before any real shell exists**. The M0 CLI harness asserts on golden trees: *given this event sequence, the core emits this patch sequence*. The interaction loop round-trips headlessly (simulate `onTap` → expect Modal in next patch).
- **UI-13** **Renderer zero**: a deliberately minimal DOM renderer (~weeks into M0) subscribing to patches and mapping components to bare HTML. Purpose: validate focus handling, controlled inputs, virtualization handles, IME, and event payload shape while the contract is cheap to change. UI contract changes require renderer-zero validation. The production web renderer (M3) descends from it.
- **UI-14** **Renderer conformance kit**: golden trees + scripted-interaction + screenshot tests shared by all renderers; behavioral divergence is release-blocking (same bar as CR-12).

## 3. Renderer implementations

| Shell | Renderer | Milestone |
|---|---|---|
| CLI harness | tree/patch assertions (+ optional TUI dump) | M0 |
| Renderer zero | minimal TS + DOM | M0 |
| macOS/iOS | SwiftUI (patches → observable view-model graph) | M1 / M6 |
| Web | TS + DOM (production; also share-link viewer SS-14) | M3 |
| Windows | WinUI 3 (ItemsRepeater virtualization) | M6 |
| Android | Compose (patches → snapshot state) | M6 |

## B. Platform app surfaces (P-11; shell-native, not applet-rendered)

- **UI-15** **Editor:** multi-file TS, generated `ctx`/schema type definitions, inline TS + policy-scan diagnostics, LLM diff viewer, test runner output, run console with structured logs, global search (files/schemas/state/logs), desktop shortcuts, mobile-friendly quick edits, iPad split view.
- **UI-16** **Schema designer:** collections, fields (stable IDs visible), types, constraints, indexes, relationships, compatibility warnings, data preview, query console. All changes are versioned CRDT ops.
- **UI-17** **Data browser:** logical tables, raw record envelopes, indexes + planner output, tombstones/purge records, CRDT frontiers, storage usage.
- **UI-18** **Permission UX (normative):** prompts are resource-specific — script/app name, requester, capability, resource/domain/path, method, limits, duration, role impact, audit behavior. "Allow network?" is forbidden; "Allow `FetchWeather` to GET `https://api.weather.example/*` up to 1 MB per run?" is the bar.
- **UI-19** **Time travel UX:** per-file timeline (author/model, change summary, diff, test/run results) with restore-as-new-version (DL-20).
- **UI-20** **LLM panel:** prompt input, context-mode selector, provider/model selector, plan preview, diff review, tests/errors, permission changes, apply/rollback, auto-loop budget display (LM-6..11).
- **UI-21** **Debug panel:** host-call timeline, permission decisions, resource usage, deterministic replay inputs, network metadata, AI trace, sync events.

## 4. Out of scope (v1)

Custom-drawn canvas components; applet-defined animations beyond built-in transitions; embedded web content inside applet UI (sandboxed `WebView` component deferred pending security review).

## 5. Acceptance

- Catalog demo applet renders on harness, renderer zero, and every shipped shell with zero per-platform applet code; conformance kit green.
- Unknown-component/prop fuzz → zero crashes, 100% fallback rendering.
- 100k-row table at 60 fps desktop and modern mobile.
- A vibe coder can generate, inspect, approve, and run a simple utility without docs; an expert can inspect code, schema, logs, permissions, history (P-11 acceptance).

## 6. Open questions

1. `Chart` scope at v1 (proposal: four listed types, no custom axes math).
2. Print/PDF export of a page at v1 or v1.x.
3. Renderer zero: keep as maintained reference renderer or freeze after M3.
