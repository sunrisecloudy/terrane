# 16 — Close the `capability_command` default-deny bypass

Status: **planned** (2026-07-01). Builds on the runtime gate (doc 07), the grant
model (doc 05), and the in-session approval work (doc 15). Verified by two
workflows: the bypass is real and confirmed live; this design was adversarially
stress-tested and hardened.

## The hole (confirmed, live)

Default-deny is enforced **only** on the host `invoke`/`app_actions` path
(`terrane-host/src/permission.rs:108`, `lib.rs:317`), which calls
`terrane_cap_auth::namespace_granted`. The capability `decide()` functions carry
**no** auth check. The MCP `capability_command` tool dispatches via
`dispatch_public_on_core` (`Request::new`, `CommandAuthority::Public`); the only
gate on that path is `admit_command` (`terrane-core/src/lib.rs:1025`), which
blocks **only** `auth.*`. So an untrusted model can write **any** app's
`kv`/`crdt`/`relational_db` with no grant.

Live proof (fresh home, no grant ever issued):

```
capability_command kv.set probe secret leaked      → records:1, isError:false
capability_command crdt.mapSet probe doc k v        → records:1, isError:false
capability_command relational_db.defineTable/put …  → records:2/1, isError:false
```

3/3 adversarial verifiers could not refute it. The in-session work (doc 15) fixed
grant *visibility*; it did not touch this. The gate guards the front door
(`invoke`); `capability_command` is an open side door.

## Design principle

Gate the **untrusted (Public) command surface** at the host, reusing the *same*
`namespace_granted` check the `invoke` path uses and the *same* `permission_required`
handshake — so elicitation and the admin console (doc 15) approve
`capability_command` denials in-session too. Trusted (CLI/FFI) dispatch and the
app-runtime resource host are unaffected (verified: CLI uses
`dispatch_on_core → Request::trusted_host`; the runtime writes go through
`RuntimeResourceHost`, not `dispatch_public_on_core`).

**Gate by namespace, not by an enumerated command list.** `namespace_of(name) ∈
{kv, crdt, relational_db}` catches every read *and* write command in those
namespaces (and is immune to command-name drift like `mapDel`/`mapRm`). This is
correct default-deny: no access to a namespace without a grant.

## The classifier

One shared, pure function is the whole policy:

```
authorize_public_command(state, name, args) -> PublicAuthz
  Refuse{reason}          — command must not run untrusted at all
  NeedsGrant{app, namespace} — resource command whose (app, namespace) is ungranted
  Allow                    — everything else (app.*, granted resource commands, …)
```

Rules (each rule answers a stress-test hole):

1. **`kv.storage.*` → Refuse** (`"storage configuration is trusted-admin-only"`).
   *Hole:* its app id is `args[1]` (scope `app`) or **absent** (scope `default`,
   a global op that repoints every app's physical backend to an attacker path).
   A per-app grant cannot authorize a global op, and `args[0]` is the scope
   literal, not an app. Storage config is an operator concern, not app-building —
   remove it from the untrusted surface entirely.

2. **`js-runtime.run` / `wasm-runtime.run` → Refuse**
   (`"run apps through the invoke tool"`). *Hole:* they live in non-grantable
   namespaces, so the namespace filter never fires, letting the model run an app
   backend directly and skip the `invoke` permission handshake. Force them
   through `invoke`, which already gates.

3. **`namespace_of(name) ∈ {kv, crdt, relational_db}`** → resolve `app = args[0]`
   (uniform for all data commands in these namespaces; only `kv.storage.*`
   differs and is already refused in rule 1). If `app` is empty/missing → Refuse
   with a clear arg error. Else if `namespace_granted(state, local_owner, app,
   namespace)` → Allow; otherwise **NeedsGrant{app, namespace}**.

4. **`build` is excluded.** *Hole:* the build capability has zero commands
   (`commands: Vec::new()`); its only surface is the pure `compileTs` resource
   read via the runtime host. No `build.*` command can reach dispatch — including
   it would be inert. Leave the runtime `ctx.resource.build` read as-is.

5. **Everything else → Allow** — `app.add`, `app.remove`, `replica.*`, etc. App
   *creation* is intended for the model (via `app_register*`/`app_scaffold`); it
   is not resource-data access. (See Out of scope for `app.import`.)

Principal is `ExecutionPrincipal::local_owner()` — the same subject grants are
keyed to and the same one `invoke` uses (verified: every Public request carries
`local_owner` today). Agent-aware principals are future work.

## Wiring — where the classifier runs

Two call sites, one policy:

- **MCP `capability_command` handler** (`terrane-host/src/mcp.rs`, both the real
  and `dryRun` branches — covers stdio *and* HTTP MCP). Call the classifier
  *before* dispatch:
  - `Refuse{reason}` → `tool_text(reason, isError)`.
  - `NeedsGrant{app, ns}` → build **and record** a command-scoped
    `permission_required` and return it via **`tool_json`** so
    `structuredContent.type == "permission_required"` — which is exactly what the
    in-session elicitation loop and the admin console key on (doc 15). *Hole:* a
    guard that only returned a `String` error could never trigger elicitation.
  - `Allow` → dispatch as today.
- **`dispatch_public_on_core` / `dry_run_public_on_core`** (`terrane-host/src/lib.rs`)
  — belt-and-suspenders: call the classifier and hard-`Err(String)` on
  `Refuse`/`NeedsGrant`. In the deny case the MCP handler already short-circuits,
  so this only fires if a *future* Public caller forgets to pre-check. *Hole:*
  gating only the dispatch branch would leave `dryRun` (which runs `decide` and
  can leak key-existence via `KeyNotFound`) ungated — so **both** branches
  enforce.

## Command-scoped `permission_required`

*Hole:* `request_permission_for_app_with_admin_base` derives `missingResources`
from the app **manifest** and errors for apps with no `--source` bundle. A
`kv.set <app>` targets a specific namespace, and the app may be sourceless.

Add `permission_required_for_namespace(core, app, namespace, source, admin_base)`
that reuses the request-id / `grantCommands` / recording machinery of
`permission.rs` but sets `missingResources = [namespace]` and does **not** read
the manifest or require a source bundle. If the target app does not exist →
return a plain `"no such app: <app>"` error (not a permission prompt).

## Out of scope (flagged follow-ups)

- **`app.import` / `app.add` over untrusted `capability_command`** write app
  bundles into reserved KV. They stay `Allow` (app creation is the model's job),
  but untrusted create/overwrite of arbitrary app ids is a separate policy
  question — track separately.
- **Verb/selector granularity.** `namespace_granted` is namespace-level (no
  read/write split, no key/table selector). This guard inherits exactly the
  `invoke` path's granularity — consistent, not a regression. A future
  read/write-verb model would refine both paths together.
- **`capability_query` needs no change.** `kv`/`crdt`/`relational_db` declare no
  queries; the only `QuerySpec` is `app.exists`. There is no resource-read query
  surface to gate (reads that *are* dispatch commands are already caught by the
  namespace gate).

## Test matrix (tests in their own files, per CLAUDE.md)

Classifier unit tests (`terrane-host`):
- `kv.storage.set`/`kv.storage.clear` (both scopes) → Refuse.
- `js-runtime.run`/`wasm-runtime.run` → Refuse.
- `kv.set`/`crdt.mapSet`/`relational_db.put` ungranted → NeedsGrant{args[0], ns};
  granted → Allow.
- `kv.set` with empty args → Refuse (arg error).
- `app.add`, `replica.*` → Allow.

Behavior / e2e (`host/mcp/tests`):
- `capability_command kv.set <app>` no grant → `permission_required`
  (structuredContent), request recorded; after grant → succeeds.
- Same via **elicitation**: approve in session → retry succeeds (no restart).
- Same via **admin console** POST approve → retry succeeds.
- `dryRun kv.set` ungranted → `permission_required`, and does **not** leak
  `KeyNotFound`.
- `capability_command kv.storage.set …` → refused, never dispatched.
- `capability_command js-runtime.run …` → refused ("use invoke").
- `capability_command app.add …` → still succeeds (ungated).
- Regression: existing `invoke` flow and the full MCP e2e stay green; CLI
  `kv`/`crdt`/`relational_db` writes (trusted) still work with no grant.

## Phased delivery (each phase green: `cargo test` + `cargo clippy -D warnings`)

1. **Classifier** — `authorize_public_command` + `PublicAuthz` + unit tests.
2. **Command-scoped permission_required** — `permission_required_for_namespace`
   + tests.
3. **Wire the MCP handler** (real + dryRun) to Refuse / emit permission_required /
   Allow; add belt-and-suspenders enforcement in
   `dispatch_public_on_core`/`dry_run_public_on_core`.
4. **e2e** — the matrix above through the real `terrane-mcp` binary, including
   elicitation + console approval of a `capability_command` denial.
5. **Docs** — AGENT_PLAYBOOK / SECURITY: `capability_command` resource writes now
   require a grant (same handshake as `invoke`, incl. in-session approval); app
   execution and storage config are `invoke`/trusted-only.

## Acceptance

1. No `capability_command` (or HTTP-MCP) call can write or read
   `kv`/`crdt`/`relational_db` for an ungranted app — it returns
   `permission_required` and records a request.
2. That `permission_required` is approvable in-session (elicitation) and via the
   admin console, after which the retried command succeeds — no restart.
3. `kv.storage.*` and `*-runtime.run` are refused over untrusted
   `capability_command`; `app.add`/app creation still works.
4. `invoke`, the CLI, and the app runtime are unchanged; replay-identity holds
   (the guard is a read-only check of folded `AuthState`, never folds).
