# Forge Public API Reference

This document indexes every **generated-app-visible** and **operator-visible** Forge surface. The live HTML reference is generated from the same sources and served at `/docs` when `forge-server` runs with the console enabled.

## Surfaces

| Surface | Audience | Source of truth | HTML section |
| --- | --- | --- | --- |
| Applet host API | Applet/script authors | `forge/std/forge-std.d.ts` | Applet API |
| Core commands | Operators, agents, shells | `forge/data/commands.json` | Core Commands |
| CLI | Local developers, CI | `forge/crates/cli` | CLI |
| HTTP bridge | Embedded hosts, web console | `forge/crates/server` | HTTP Bridge |
| Example applets | Authors learning the API | `forge/examples/*` | Examples |

## Regenerate the page

```sh
node --no-warnings tools/build-forge-api-docs.mjs
node --test tools/test/forge-api-docs.test.mjs
```

Output lands in `forge/docs/public-api/` (`index.html`, `styles.css`, `app.js`).

## Related markdown

- [Applet authoring guide](applet-authoring-guide.md) — how to write `main(ctx, input)` applets
- [CLI reference](cli-reference.md) — `forge commands`, `describe`, `run`, `trace`, `demo`
- [HTTP bridge reference](http-bridge-reference.md) — `/bridge`, `/events/drain`, `/schemas/commands/*`
- [Example applets](example-applets.md) — bundled `forge/examples/*` coverage matrix

## Contract export

The public contract (`artifacts/public-contract.json`) includes this reference, `forge-std.d.ts`, command schemas, and conformance tests. After changing any public API artifact, regenerate:

```sh
node --no-warnings tools/build-forge-api-docs.mjs
node --no-warnings tools/export-commands-catalog.mjs
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
```