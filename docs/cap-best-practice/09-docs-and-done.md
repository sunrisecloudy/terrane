# 09 — Documentation obligations and definition of done

## The capability documents itself

Two in-code surfaces, both part of the capability — not optional polish:

- **`doc()`** returns the `CapabilityDoc`
  (`rust/crates/terrane-cap-interface/src/doc.rs`): summary, commands with
  params/errors/emitted events, queries, resources, constraints, examples.
  Keep it in `src/doc.rs`. It is the contract rendered by
  `terrane cap list` / `terrane cap info <ns>` and served to MCP clients —
  `terrane-cap-net/src/doc.rs` is a compact model. Put trusted-only or
  internal notes behind the `include_internal` flag rather than omitting them.
- **`describe()`** renders one log line per *own* event for `terrane log`.
  Return `None` for foreign kinds and for corrupt payloads
  (`decode_event(record).ok()?`); truncate large values
  (`helpers::truncate`).

## Markdown docs move with the code

Docs rot was a review finding (024): a permission gate changed and only some of
the docs mentioning it were updated, so clients built wrong mental models.
Rule: **a change to commands, resources, or policy updates every doc that
states the old behaviour, in the same commit series.**

| You changed… | Update |
|---|---|
| The `ctx.resource` surface | `docs/APP_API.md` — regenerate: `UPDATE_DOCS=1 cargo test -p terrane-core --test cap app_api_doc` (a default test fails while stale) |
| Command classification / grants / trust boundaries | `host/mcp/docs/SECURITY.md`, `CAPABILITY_OPERATIONS.md` — then sweep `AGENT_PLAYBOOK.md`, `APP_BUILDING.md`, `CLIENTS.md`, `README.md` for stale statements |
| The host HTTP/MCP contract | `docs/SERVER_API.md` (source of truth: `rust/crates/terrane-api`) |
| The exported public surface | `terrane contract export`, `tools/export-public-contract.mjs`, `tools/verify-public-contract.mjs` — see [08-public-surface-and-release.md](08-public-surface-and-release.md) |
| Capability-building conventions | this folder |

Grep the docs for your command names and event kinds before calling a policy
change done.

## Definition of done

A capability lands when all of this holds:

- [ ] Manifest declares exactly what you own; `default_registry()` builds
      (`validate()` passes — covered by opening a `Core` in any test).
- [ ] Engine tests assert events, state, error values, the `app.removed`
      cascade (if subscribed), and `replay_matches()` after mutations.
- [ ] Binary e2e exists — default-run if pure, `#[ignore = "reason"]` if
      effectful ([07-testing.md](07-testing.md)).
- [ ] Resources ⇒ grant spec declared, inventory tests extended, `APP_API.md`
      regenerated ([06-permissions-and-policy.md](06-permissions-and-policy.md)).
- [ ] New commands classified in `public_authz.rs` (or deliberately left
      refused-by-default) and audited for bypass side-channels.
- [ ] Public surface decision made: generic capability only, `ctx.resource`,
      CLI, MCP workflow/tool, HTTP route, or deliberately private
      ([08-public-surface-and-release.md](08-public-surface-and-release.md)).
- [ ] Existing logs still replay. Any event/payload/reserved-KV layout change is
      versioned or has a replay fixture for the old shape.
- [ ] `doc()` and `describe()` implemented; affected markdown docs updated.
- [ ] Capability discovery smokes work: `terrane cap info <ns>`, MCP
      `capabilities_list`, MCP `capability_info(<ns>)`, and
      `capability_command` with `help: true` for each public command.
- [ ] If exported surface changed, contract export/verification and
      `terrane-host/tests/contract.rs` are green.
- [ ] From `rust/`: `cargo test` green and
      `cargo clippy --all-targets -- -D warnings` green. `host/cli` validated
      separately if touched.
- [ ] Smoke by hand:
      `cargo run -p terrane-host --bin terrane -- cap info <ns>`.
- [ ] Committed as small, green, granular commits on a branch off `main`;
      stage your own files explicitly — never `git add -A`.

Back to the [checklist](README.md).
