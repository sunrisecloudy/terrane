# 08 — Public surface and release checks

The capability crate is only the engine slice. Before shipping, decide how the
outside world learns about it and what compatibility promise you just made.

## Choose the front door

| Surface | Add it when | Source of truth / proof |
|---|---|---|
| Generic capability only | Operators or agents can use `capability_command` / `capability_query` safely | `CapabilityDoc`, `public_authz.rs`, MCP help tests |
| `ctx.resource.<ns>` | App backends should call it from JS/WASM | `manifest().resources`, grant specs, generated `docs/APP_API.md` |
| CLI command | Humans need a first-class terminal workflow | `rust/crates/terrane-host/src/cli.rs`, host e2e tests |
| MCP workflow/tool | Agents need a safer or clearer path than raw capability calls | `rust/crates/terrane-api` tool declarations, `terrane-host/src/mcp.rs`, MCP tests/docs |
| HTTP route | Web or premium hosts need a stable route | `rust/crates/terrane-api`, `host/web`, `docs/SERVER_API.md`, contract tests |

Prefer the smallest public surface that solves the workflow. A new capability
does not automatically deserve a bespoke CLI command, MCP tool, or HTTP route.
When it stays generic, make `doc()` and `capability_command` help good enough
for an agent with no source access.

## Machine-readable contract

`docs/SERVER_API.md` is the human explanation, but the exported contract is the
thing consumers pin. If a change affects routes, MCP tools, capability docs,
`ctx.resource`, app contract, or sync surface, verify both live declarations and
the exported artifact.

From the repo root:

```sh
cargo test -p terrane-host --test contract
cargo run -p terrane-host --bin terrane -- contract export
```

From the repo root when refreshing a distributable artifact:

```sh
node --no-warnings tools/export-public-contract.mjs --out public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract public-contract.json
```

Do not hand-edit `public-contract.json`. It exists so `terrane-premium` and other
consumers can prove they implement the same base surface.

## Discovery smoke

A capability is not MCP-ready merely because the Rust tests pass. Smoke the
actual discovery path:

1. `terrane cap list` includes the namespace.
2. `terrane cap info <ns>` shows commands, params, emitted events/effects,
   queries, resources, constraints, and examples.
3. MCP `capabilities_list` and `capability_info(<ns>)` expose the same usable
   story.
4. MCP `capability_command` with `{ "name": "<ns>.command", "help": true }`
   returns ordered args, returns/errors, emits/effects, and notes without
   dispatching.
5. For grant-gated commands, the ungranted path returns a structured
   `permission_required`; after approval, retrying the same command works.

## Compatibility before release

- Old logs must replay. If event payloads or reserved-KV layouts changed, add a
  fixture or focused test that opens a log with the old records.
- New public commands must be classified, documented, and tested through the
  edge path that exposes them.
- Renames need an alias, migration note, or explicit breaking-change decision.
- If a capability imports host paths, network data, model output, or generated
  bundles, document the trust boundary and test the refusal path before the
  effect runs.

Next: [09-docs-and-done.md](09-docs-and-done.md).
