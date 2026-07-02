# 06 — Resources, grants, and command policy

Authorization is layered: grants in the core (owned by `terrane-cap-auth`) and
a classification gate at the host edge. A new capability usually touches both.

## The resource surface (`ctx.resource.<ns>`)

If app backends should use your capability, declare methods in the manifest and
implement the read side:

```rust
resources: vec![
    ResourceMethod::Write { name: "set", params: &["key", "value"] },
    ResourceMethod::Read  { name: "get", params: &["key"] },
],
```

- Reads dispatch to your `read_resource` (app-scoped via `ctx.app`); they emit
  nothing. Writes route back through your `decide` and become events
  ([05-effects-and-runtimes.md](05-effects-and-runtimes.md)).
- Reject reserved keys (`__terrane/…`) at this surface — apps must never read
  or forge platform projections ([04-cross-capability.md](04-cross-capability.md)).
- The `ctx.resource` reference in `docs/APP_API.md` is **generated** from these
  declarations and drift-guarded by a test; regenerate with
  `UPDATE_DOCS=1 cargo test -p terrane-core --test cap app_api_doc`.

## Grant specs — mandatory with resources

Every capability with resources must declare how grants target it, minimally
the baseline selector:

```rust
grant_resources: vec![GrantResourceSpec::namespace_v1(
    "<ns>", &["read", "write"], "One-line summary for approval UIs.",
)],
```

`Registry::validate()` fails on resources without grant specs (and the
reverse), and inventory tests
(`rust/crates/terrane-core/tests/cap/grant_spec_inventory.rs`,
`grant_verbs_match_specs.rs`) lock that the shipped registry's verbs cover the
declared method kinds — extend their namespace lists with yours.

**Default deny.** At runtime an app only sees your methods if a grant exists:
the resource host checks `terrane_cap_auth::namespace_granted(state, principal,
app, ns)` and otherwise returns an empty method table. You implement no check
yourself — declaring the spec is your whole job; `auth` and the host enforce.

## Command authority

`Request { authority }` is `Public` by default; hosts mark their own control
plane `TrustedHost`. The core's admit gate refuses `auth.*` commands without
`TrustedHost`. If one of your commands is host-admin-only (storage repointing,
destructive ops), don't invent a new gate — refuse it at the edge
classification below, the way `kv.storage.*` and `app.remove` are.

## Edge classification (`classify_public_command`)

`rust/crates/terrane-host/src/public_authz.rs` classifies every command an
*untrusted* caller (MCP `capability_command`) may attempt:

| Disposition | Meaning | Examples |
|---|---|---|
| `Allow` | Safe unconditionally | `app.add`, `replica.init` |
| `GrantGated { namespace, app_arg_index }` | Allowed iff the app holds a grant for that namespace | `kv.set`, `crdt.*`, `relational_db.*` |
| `Refuse { reason }` | Never over untrusted paths | `kv.storage.*`, `app.remove`, `app.import`, `net.fetch`, `auth.*` |
| `Unclassified` | **Default — refused** | anything not listed |

New commands are refused until you classify them, which is safe — but classify
deliberately rather than leaving them to rot. For `GrantGated`, the app id's
argument position is part of the classification; keep it stable.

Update `rust/crates/terrane-host/tests/public_authz.rs` with the classifier
change. The inventory tests there lock the registered command count, the
allow/refuse/grant-gated split, public query coverage, and side-channel probes
such as "`app.import` must not emit `kv.storage.configured` through an untrusted
path".

**Audit for bypasses (the review-024 lesson).** A refusal is a property of the
*event kinds it protects*, not of one command name. `app.import` was `Allow`
while carrying `--storage`/`--path` options that repointed storage — the exact
thing refusing `kv.storage.*` was meant to prevent — so it is now `Refuse`.
When you classify, check every `Allow`/`GrantGated` command of yours: can any
argument combination emit an event kind whose own command is refused? Add a
policy test that probes the real surface, not just the classifier table.

## Principals: don't bake in "local"

Grants key on `(org, subject, app, resource)`. Where v1 intentionally assumes
the local owner (`LOCAL_ORG`, `LOCAL_OWNER_SUBJECT`), say so in a comment and
derive from the dispatching `ExecutionPrincipal` wherever you already can —
silent hardcoding is how multi-org support gets painful later (review-006).

Changing any of this means updating the MCP docs in the same change — see
[09-docs-and-done.md](09-docs-and-done.md).

Next: [07-testing.md](07-testing.md).
