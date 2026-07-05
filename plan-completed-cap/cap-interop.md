# Capability: `interop` — app-to-app calls over the existing verb surface

New crate `rust/crates/terrane-cap-interop/`, namespace `interop`, registered
in `default_registry`. One app calls another app's backend verbs — the
Sandstorm-powerbox idea rebuilt on machinery Terrane already has. Nothing new
is invented at the app surface: the verb contract (`handle(input)`,
`__actions__` discovery) is already MCP-shaped and already exposed to agents
via the host MCP (`app_actions` + `invoke`); interop gives the same surface a
second caller — other apps.

## Locked decisions (user, 2026-07-05)

1. **Reuse the verb surface, host-mediated.** No MCP client inside QuickJS and
   no network hop: `ctx.resource.interop.call(target, verb, …args)` dispatches
   the target's backend through the normal `js-runtime.run` path. MCP is the
   discovery/description format; the host is the transport.
2. **Every app must expose the common API.** `common.receive` is **required
   for all apps**: bundle validation (builder validate and `app.import`)
   rejects any bundle that does not declare and implement it — including
   reinstalls of existing bundles, which must be patched. The repo's own apps
   (`apps/todo`, premium aliases) are patched in this slice.
3. **Replies are recorded.** Each call folds as an `interop.called` event so
   the caller's replay reproduces the reply without re-running the target —
   the same recorded-effect story as `model.ask`.

## The common API

A registry of well-known verbs under `common.*`, documented in `APP_API.md`:

| Verb | Required | Contract |
| --- | --- | --- |
| `common.receive` | **yes** | `handle(["common.receive", kind, payloadJson])` → string reply. `kind` is a hint (`"text"`, `"json"`, `"email"`, `"link"`, `"blob"`); payload for blobs is a `{name, hash, size, mime}` ref per [cap-blob.md](cap-blob.md). The scaffold default stores the item under `inbox/<n>` in kv. |
| `common.search` | no | `(query)` → JSON array of `{id, title, snippet}` |
| `common.export` | no | `(format)` → document/blob ref |

The manifest gains `"interfaces": ["inbox", …]` — `inbox` is implied and
mandatory; optional interfaces advertise picker eligibility (e.g. a
"send to…" sheet lists `inbox` apps; a global search fans out to `search`
apps). Consumers of this contract: inbound email
([cap-email.md](cap-email.md) v2), OS deep links / share sheet
([cap-deep-links.md](cap-deep-links.md)), and any app with data to hand off.

## Command / event / resource surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `interop.call` | args `caller, target, verb, args…` → validates grant (folded auth state) + target existence + verb not `__`-internal, returns `Decision::Effect(Effect::AppCall {chain, target, verb, args})` |
| Resource | `interop.call(target, verb, …args)` | routes to the command; returns the reply string |
| Resource | `interop.send(interface, kind, payload)` | sugar: resolve the caller's granted default target for `interface` (or trigger a pick), then `common.receive` |
| Resource | `interop.pick(interface)` | powerbox: raises a permission-elicitation in the shell listing apps declaring `interface`; the user's choice is recorded as an auth grant (`interop:<interface>=<target>` selector) and returned |
| Query | `interop.apps` | apps declaring a given interface (from folded app catalog manifests) |
| Event | `interop.called` | `{caller, target, verb, args, reply_kind, reply, reply_hash, ok}` — reply > 256 KiB offloads to the blob CAS |
| Event (auth-owned) | `auth.granted` | picker results reuse the existing grant events — no parallel ACL system |

## Execution & replay

The edge runs the target exactly as a normal backend run: the target executes
with **its own** manifest resource scope (never the caller's), records its own
ordinary `kv.*`/resource events, and its string reply is recorded in
`interop.called`. Replay folds the target's events and the caller's recorded
reply — neither backend re-runs (Option A holds end to end).

`Effect::AppCall` carries the call `chain` (list of app ids): max depth **4**,
a target already in the chain is a typed `InteropCycle` error. Calls run under
the existing single-writer lock; a nested call is part of the same
command-lifecycle, not a concurrent writer.

## Security & permissions

- Grants are directional and explicit: caller → target (specific app) or
  caller → interface (target chosen by the user via the picker). Both flow
  through the existing `auth.permission.request/approve` elicitation — the
  picker is that prompt with an app list instead of yes/no, preserving the
  Sandstorm insight (choosing IS granting).
- The target sees `caller` as the request principal (auth's
  `ExecutionPrincipal` extended with the calling app), so a target can vary or
  refuse by caller.
- Internal verbs (`__actions__`, `__`-prefixed) are never callable via interop.
- Revocation = existing `auth.revoke`; folded state consulted on every decide.

## Limits

Reply 256 KiB inline / 8 MiB via blob; args ≤ 64 KiB total; depth 4;
100 interop calls per backend run; picker lists ≤ 200 apps.

## Implementation plan

1. **Interface:** `Effect::AppCall {chain, target, verb, args}` in
   `terrane-cap-interface::abi`; extend `ExecutionPrincipal` with an
   app-caller variant.
2. **Manifest:** `interfaces` field in bundle manifest parsing (terrane-cap-app
   / builder); validation rule: `inbox` mandatory + backend must answer a
   `common.receive` probe in builder validate; `app.import` enforces the
   manifest declaration (deterministic check).
3. **Crate `terrane-cap-interop`:** manifest, decide (grant check, chain
   rules), fold (`interop.called`, keep last N per caller), doc, describe.
4. **Edge:** `AppCall` arm in `EdgeRunner::run` — nested `js-runtime.run`
   dispatch, reply capture, blob offload.
5. **Picker:** shell elicitation variant (web + mac) rendering `interop.apps`;
   grant recording via existing auth commands; MCP parity (agents hit the same
   permission_required flow).
6. **Common API rollout:** scaffold template gains default `common.receive`;
   patch `apps/todo` + premium aliases; `APP_API.md` registry section.
7. **Tests:** engine (grant enforcement, cycle/depth, replay identity of
   nested runs, reply offload); e2e (two real apps calling each other,
   picker-grant flow, validation rejection of a bundle without `common.receive`).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Cross-replica interop (rides [cap-sync-v2.md](cap-sync-v2.md) later),
streaming replies, transitive capability forwarding (A hands B a capability to
C — Sandstorm does this; we require the user in the loop per edge), interface
version negotiation beyond the verb-name contract.
