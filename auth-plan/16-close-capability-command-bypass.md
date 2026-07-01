# 16 — Close the `capability_command` default-deny bypass

Status: **production-ready plan** (2026-07-01). Builds on the runtime gate (doc
07), the grant model (doc 05), and the in-session approval work (doc 15).
Verified by two workflows: the bypass is real and confirmed live; this design
was adversarially stress-tested and hardened.

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

This slice closes the **resource-data bypass** and, with the allowlist posture,
locks down the *entire* untrusted `capability_command` surface: every registered
command is explicitly grant-gated, refused, or allowlisted (see the table below),
and a policy-inventory test fails if a new command is not classified. Notably it
refuses `net.fetch` / `model.ask` / `harness.*` over the untrusted surface —
closing an effect hole (SSRF / exfiltration / model spend) worse than the kv
bypass that prompted it: today an untrusted `capability_command net.fetch <url>`
runs. Only genuinely future refinements (app replace/update ownership,
read/write-verb granularity, agent principals) remain follow-ups.

**Discover grantable namespaces from metadata, then require an explicit command
classification.** The classifier should derive candidate resource namespaces
from capability metadata:

- command namespace is `namespace_of(name)`
- grantable namespace is present in `CapManifest.grant_resources`

Today that yields `kv`, `crdt`, and `relational_db`. Metadata alone is not
enough to authorize a command, because each grant-gated command also needs an
explicit app-id extractor. The policy inventory must classify every grantable
command as either:

- `GrantGated { namespace, app_arg_index }` — today always `app_arg_index = 0`
  for `kv`, `crdt`, and `relational_db` data commands.
- `Refuse { reason }` — required for grantable commands whose target app cannot
  be extracted safely, such as `kv.storage.*`.

A future grantable capability with direct commands is therefore denied by
default unless it lands both in `grant_resources` and in the policy inventory
with an explicit extractor.

## The classifier

One shared, pure function is the whole policy:

```
authorize_public_command(state, name, args) -> PublicAuthz
  Refuse{reason}          — command must not run untrusted at all
  NeedsGrant{app, namespace} — resource command whose (app, namespace) is ungranted
  Allow                    — explicitly allowed non-resource command
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

3. **Grantable command namespace with an explicit app-id extractor** → resolve
   `app = args[app_arg_index]`. Today this includes `kv`, `crdt`, and
   `relational_db` data commands, all with `app_arg_index = 0`; `kv.storage.*`
   differs and is already refused in rule 1. If no extractor is registered for a
   grantable command, **Refuse** with `"public command is not classified for
   grant gating"`. If `app` is empty/missing → Refuse with a clear arg error. If
   the app does not exist → Refuse `"no such app: <app>"` (not a permission
   prompt). Else if `namespace_granted(state, local_owner, app, namespace)` →
   Allow; otherwise **NeedsGrant{app, namespace}**.

4. **`build` is excluded.** *Hole:* the build capability has zero commands
   (`commands: Vec::new()`); its only surface is the pure `compileTs` resource
   read via the runtime host. No `build.*` command can reach dispatch — including
   it would be inert. Leave the runtime `ctx.resource.build` read as-is.

5. **Effect commands → Refuse** — `net.fetch` (untrusted HTTP: SSRF / data
   exfiltration), `model.ask` (untrusted paid model spend), and
   `harness.generate-app` / `harness.run-js` (both trigger model-CLI edge
   effects). These are *effects*, not grantable resources, so they cannot be
   grant-gated under the current model; an untrusted model must not fire them
   directly. They remain available through the app runtime (a granted app whose
   manifest declares `net`/`model`) and through trusted/dedicated tooling.

6. **Destructive lifecycle → Refuse** — `app.remove` deletes an app and cascades
   to wipe its `kv`/`crdt`/`relational_db` data through broadcast fold. An
   untrusted model must not trigger it — this is exactly the remove-then-re-add
   data-loss footgun seen in earlier weak-model runs. Operator/trusted path only.

7. **Raw bundle import → Refuse** — `app.import` installs a bundle from a host
   path and can pass `--storage` / `--path`, which may emit
   `kv.storage.configured`. Public app creation must use the dedicated
   `app_register` / `app_register_inline` tools that validate and stage bundles
   before dispatching the narrow app-add path. Raw import is trusted tooling.

8. **Explicit Allow set** (everything else must be *listed*, not defaulted):
   - `app.add` — create-new app registration is an intended low-level model
     operation. It stays allowed only while core/edge semantics reject existing
     app ids (`Error::AppExists`) and never replace an app through this public
     path. If replace/update semantics are added later, the inventory test must
     move the public command to Refuse or to a confirmation/ownership policy.
   - `replica.init` — idempotent local identity the multi-capability flow needs.
   - queries `app.exists`, `replica.peer` — read-only catalog / identity.
   - `help:true` on any command — usage text, no dispatch, no state change.

   Anything not in this Allow set and not a gated grantable command is **Refused
   until classified** — enforced by the policy-inventory test, so a new Public
   command cannot silently become reachable.

Principal is `ExecutionPrincipal::local_owner()` — the same subject grants are
keyed to and the same one `invoke` uses (verified: every Public request carries
`local_owner` today). Agent-aware principals are future work.

**Scope.** This classifier governs only the generic `capability_command` /
`capability_query` escape hatches. The dedicated tools — `invoke`, `app_actions`
(already grant-gated), `app_register*`, `app_scaffold`, `list_apps`,
`permission_*` — are separate surfaces and unaffected.

## Production public-command policy

Every command in the registry gets an explicit disposition. This is the
authoritative table the policy-inventory test enforces — **36 registered commands
+ 2 queries** as of this writing (sourced from `terrane cap info <ns>`, not a
guess):

| Command(s) | Namespace | Disposition | Why |
|---|---|---|---|
| `kv.set`, `kv.rm`, `kv.delete` | kv | **Grant-gated** | app data write; explicit extractor `app=args[0]` → grant or `permission_required` |
| `crdt.mapSet`,`mapDel`,`listPush`,`listInsert`,`listDel`,`textInsert`,`textDel`,`merge` | crdt | **Grant-gated** | app data write; explicit extractor `app=args[0]` |
| `relational_db.defineTable`,`put`,`delete` | relational_db | **Grant-gated** | app data write; explicit extractor `app=args[0]` |
| `kv.storage.set`, `kv.storage.clear` | kv | **Refuse** | storage config; app at `args[1]`/global; operator-only |
| `js-runtime.run`, `wasm-runtime.run` | js/wasm-runtime | **Refuse** | run apps via `invoke` (gates + prompts) |
| `net.fetch` | net | **Refuse** | untrusted HTTP = SSRF / exfil |
| `model.ask` | model | **Refuse** | untrusted paid model spend |
| `harness.generate-app`, `harness.run-js` | harness | **Refuse** | trigger model-CLI effects (spend) |
| `auth.*` — `grant`,`revoke`,`permission.{request,approve,deny,cancel}`,`agent.{register,delegate,revoke}`,`member.ensure-local-owner` | auth | **Refuse** | trusted-admin-only (already via `admit_command`) |
| `app.remove` | app | **Refuse** | destructive; wipes app data via cascade |
| `app.import` | app | **Refuse** | raw bundle install can configure storage through `--storage` / `--path`; use `app_register*` |
| `app.add` | app | **Allow** | create-new app registration; existing ids must still fail with `AppExists` |
| `replica.init` | replica | **Allow** | idempotent local identity |
| `app.exists` (query), `replica.peer` (query) | app / replica | **Allow** | read-only catalog / identity |
| any `help:true` | — | **Allow** | usage text; no dispatch |
| any newly-registered command / query | — | **Refuse until classified** | inventory test fails until a row exists |

Net effect across the 36 Public commands: **14 grant-gated, 20 refused** (10 of
them the pre-existing `auth.*`), **2 allowed**; both queries allowed. Adding a
capability command means choosing one row here and adjusting a test — production
behavior stays auditable, and no command is left to a "default allow" posture.

## Wiring — where the classifier runs

Two call sites, one policy:

- **MCP `capability_command` handler** (`terrane-host/src/mcp.rs`, both the real
  and `dryRun` branches — covers stdio *and* HTTP MCP). Call the classifier
  *before* dispatch:
  - `Refuse{reason}` → `tool_text(reason, isError)`.
  - `NeedsGrant{app, ns}` on the real command path → build **and record** a
    command-scoped `permission_required` and return it via **`tool_json`** so
    `structuredContent.type == "permission_required"` — which is exactly what the
    in-session elicitation loop and the admin console key on (doc 15). *Hole:* a
    guard that only returned a `String` error could never trigger elicitation.
  - `NeedsGrant{app, ns}` on the `dryRun` path → build the same structured
    `permission_required` shape, but mark it `requestStatus = "preview"` and
    **do not record** `auth.permission.request`. A dry run must not create an
    approvable pending request; the real command does. The MCP elicitation
    extractor must ignore preview-only permission responses so it never prompts
    for a request id that is not actually pending.
  - `Allow` → dispatch as today.
- **`dispatch_public_on_core` / `dry_run_public_on_core`** (`terrane-host/src/lib.rs`)
  — belt-and-suspenders: call the classifier and hard-`Err(String)` on
  `Refuse`/`NeedsGrant`. In the deny case the MCP handler already short-circuits,
  so this only fires if a *future* Public caller forgets to pre-check. *Hole:*
  gating only the dispatch branch would leave `dryRun` (which runs `decide` and
  can leak key-existence via `KeyNotFound`) ungated — so **both** branches
  enforce.
- **Policy inventory test** (contract/host test): enumerate all registered
  commands and prove each Public command is either refused, grant-gated, or
  explicitly allowlisted. This is the production guard against future capability
  drift.

## Command-scoped `permission_required`

*Hole:* `request_permission_for_app_with_admin_base` derives `missingResources`
from the app **manifest** and errors for apps with no `--source` bundle. A
`kv.set <app>` targets a specific namespace, and the app may be sourceless.

Add `permission_required_for_namespace(core, app, namespace, source, admin_base)`
that reuses the request-id / `grantCommands` / recording machinery of
`permission.rs` but sets `missingResources = [namespace]` and does **not** read
the manifest or require a source bundle. If the target app does not exist →
return a plain `"no such app: <app>"` error (not a permission prompt).

On the **`dryRun`** path, report the requirement *without recording* a request —
a dry run must stay side-effect-free. Only the real path records the pending
`auth.permission.request`. The preview response still includes the same app,
namespace, admin URL, and grant command hints, but sets
`requestStatus = "preview"` and either omits admin approval affordances or makes
clear that the caller must rerun the real command to create an approvable
request.

The payload must make the operation source obvious to admins and agents:

- `operation = "capability_command:<name>"`, for example
  `capability_command:kv.set`
- `source = "mcp_stdio"` / `"mcp_http"` / existing MCP source string
- `requestStatus = "pending"` for recorded real-command requests; `"preview"`
  for dry-run-only requirements
- `message` says this is a direct capability command, not an app runtime invoke
- `grantCommands` stays the same (`terrane auth grant user:local-owner <app>
  <namespace>`) because the grant being requested is still the app namespace
  grant

That distinction matters in production audit logs: a grant request caused by
`invoke notes write` and one caused by direct `capability_command kv.set notes`
are both valid permission requests, but they should not be indistinguishable.

## Query policy

`capability_query` needs no runtime change for the current registered surface:
`kv`, `crdt`, and `relational_db` declare no direct queries, and the existing
`app.exists` query is catalog metadata rather than resource data.

Production regression rule: if a future capability with `grant_resources`
declares a query, public `capability_query` must be gated by the same grant
model or refused by default. Add a test that enumerates registered query specs
and fails when a grantable namespace exposes a public query without an explicit
policy decision.

## Out of scope (genuinely future — the existing surface is fully classified above)

- **App replace/update ownership.** `app.add` stays `Allow` only for create-new
  registration because the current core/edge path rejects existing ids. Raw
  `app.import` is refused on the untrusted public command surface; any future
  public import/replace/update command needs an ownership/confirmation model and
  a storage-side-effect policy before it can be exposed.
- **Verb/selector granularity.** `namespace_granted` is namespace-level (no
  read/write split, no key/table selector). This guard inherits exactly the
  `invoke` path's granularity — consistent, not a regression. A future
  read/write-verb model would refine both paths together.
- **Agent-aware principal.** The guard hardcodes `local_owner`; a future
  multi-agent principal model refines the guard and the `invoke` gate together.
- **Future effect capabilities.** Any new effect command must land a row in the
  classification table (defaulting to Refuse) before exposure — the inventory
  test enforces this.

## Test matrix (tests in their own files, per CLAUDE.md)

Classifier unit tests (`terrane-host`):
- `kv.storage.set`/`kv.storage.clear` (both scopes) → Refuse.
- `js-runtime.run`/`wasm-runtime.run` → Refuse.
- `net.fetch`/`model.ask`/`harness.generate-app`/`harness.run-js` → Refuse.
- `app.remove` → Refuse.
- `app.import` with storage options → Refuse before dispatch; no
  `kv.storage.configured` record is appended.
- `kv.set`/`crdt.mapSet`/`relational_db.put` ungranted → NeedsGrant{args[0], ns};
  granted → Allow.
- `kv.set` with empty args → Refuse (arg error).
- non-existent app arg on resource command → Refuse `"no such app"` rather than
  permission prompt.
- `app.add`, `replica.init` → Allow.
- `app.add` against an existing id still fails with `AppExists`; the public
  allowlist must not become an overwrite path.
- no allowlisted command may emit `kv.storage.configured`.
- a grantable command with no explicit app-id extractor → Refuse or fail the
  policy-inventory test until classified.
- unknown/new command class → Refuse or fail policy-inventory test until
  classified.

Behavior / e2e (`host/mcp/tests`):
- `capability_command kv.set <app>` no grant → `permission_required`
  (structuredContent), request recorded, `operation` is
  `capability_command:kv.set`; after grant → succeeds.
- Same via **elicitation**: approve in session → retry succeeds (no restart).
- Same via **admin console** POST approve → retry succeeds.
- `dryRun kv.set` ungranted → `permission_required`, records no pending request,
  cannot be approved from that dry-run alone, and does **not** leak `KeyNotFound`.
- `permission_required_from_tool_response` / elicitation extraction ignores
  `requestStatus = "preview"` and still extracts recorded pending requests.
- `capability_command kv.storage.set …` → refused, never dispatched.
- `capability_command js-runtime.run …` → refused ("use invoke").
- `capability_command net.fetch …` / `model.ask …` / `harness.generate-app …` →
  refused (effect; never dispatched, no network/model call made).
- `capability_command app.remove …` → refused (destructive lifecycle).
- `capability_command app.import … --storage … --path …` → refused before
  dispatch; no `kv.storage.configured` record is appended.
- `capability_command app.add …` / `replica.init` → still succeed when they are
  allowlisted operations.
- `capability_command app.add …` for an existing id → `AppExists`, no overwrite.
- `capability_query app.exists` still succeeds; any grantable namespace query
  added in future fails the policy-inventory test until gated/refused.
- policy inventory covers every registered command and every registered query.
- Regression: existing `invoke` flow and the full MCP e2e stay green; CLI
  `kv`/`crdt`/`relational_db` writes (trusted) still work with no grant.

## Phased delivery (each phase green: `cargo test` + `cargo clippy -D warnings`)

1. **Policy inventory + classifier** — `authorize_public_command` + `PublicAuthz`
   + command/query inventory tests that force classification of every Public
   surface.
2. **Command-scoped permission_required** — `permission_required_for_namespace`
   with `operation = capability_command:<name>` + tests.
3. **Wire the MCP handler** (real + dryRun) to Refuse / emit permission_required /
   Allow; add belt-and-suspenders enforcement in
   `dispatch_public_on_core`/`dry_run_public_on_core`.
4. **e2e** — the matrix above through the real `terrane-mcp` binary, including
   elicitation + console approval of a `capability_command` denial.
5. **Docs** — AGENT_PLAYBOOK / SECURITY: `capability_command` resource writes now
   require a grant (same handshake as `invoke`, incl. in-session approval); app
   execution and storage config are `invoke`/trusted-only.
6. **Public-command follow-up tickets** — explicitly file remaining hardening
   decisions for app replace/update ownership, verb/selector granularity, and
   agent-aware principals. `app.remove` and current effect commands are not
   follow-ups for this slice; they are refused here and covered by tests.

## Acceptance

1. No real `capability_command` (or HTTP-MCP) call can write or read
   `kv`/`crdt`/`relational_db` for an ungranted app — it returns
   `permission_required` and records a request.
2. That real-command `permission_required` is approvable in-session
   (elicitation) and via the admin console, after which the retried command
   succeeds — no restart. `dryRun` returns a preview `permission_required`
   without recording a request.
3. The permission request clearly says `operation =
   capability_command:<command>` and records the MCP source, so direct capability
   prompts are distinguishable from app runtime invokes.
4. `kv.storage.*`, `*-runtime.run`, `net.fetch`, `model.ask`, `harness.*`,
   `app.import`, and `app.remove` are refused over untrusted
   `capability_command`; `app.add` / `replica.init` still work for this slice,
   and existing-app adds cannot overwrite.
5. Every registered Public command is covered by a policy inventory test:
   refused, grant-gated, or explicitly allowlisted.
6. Every registered Public query is covered by a policy inventory test; future
   grantable capability queries must be gated or refused by default.
7. `invoke`, the CLI, and the app runtime are unchanged; replay-identity holds
   (the guard is a read-only check of folded `AuthState`, never folds).
