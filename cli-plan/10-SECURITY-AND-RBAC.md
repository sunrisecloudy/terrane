# 10 — Security & RBAC

The unified CLI exposes "every action," so the security model must be explicit:
**convenience must not widen the attack surface.** The guiding rule — the CLI is a
*front-end*; it never bypasses the gates the core already enforces.

## The gates that already exist (and stay)

1. **Role gate (RBAC).** `authorize(cmd)` runs *before* dispatch and rejects on
   role (`forge/crates/core/src/auth.rs:28`). The CLI cannot skip it — it goes
   through `handle()` like everyone else.
2. **Capability gate.** Collection-scoped `db.read`/`db.write` grants narrow
   access *after* the role gate (`auth.rs:239`, `:341`). Scoped from trusted
   context, never from the request payload.
3. **Actor identity is set by the host, not the caller.** The server injects its
   own actor/workspace and ignores client-supplied identity
   (`forge/crates/server/src/lib.rs:107`). The CLI must do the same: the `--role`
   flag is a *local-core developer convenience*, not a way to forge identity
   against a server.
4. **Audit trail.** Privileged and mutating commands already land in the SC-12
   audit log; `audit.query` reviews them. The CLI inherits this for free.

## The new dimension: visibility tiers

The catalog adds `visibility` per command so each front-end can filter what it
*offers* — a layer **on top of** RBAC, not a replacement.

| Tier | Meaning | CLI | Web console (public build) | Agent (default) |
| --- | --- | --- | --- | --- |
| `public` | safe, app-facing reads/runs | ✅ | ✅ | ✅ |
| `operator` | install/manage/export | ✅ | ✅ (authed) | ✅ (authed) |
| `admin` | quotas, audit, trust, provisioning | ✅ (authed) | 🔒 opt-in flag | 🔒 opt-in |
| `debug` | `control.*`, `legacy.core_step`, bridge gates | 🔒 feature-gated | ❌ | ❌ |

**Tier ≠ role.** RBAC decides *whether a given actor may run* a command; tier
decides *whether a front-end even shows/offers* it. Both apply. A `public` command
still enforces its role set; an `admin` command is hidden from the public console
*and* still role-gated if reached.

### Invariant (Phase 1 test)

> No `public` command may require a privileged-only role. If a command needs Owner
> it cannot be `public`. This catches accidental exposure at build time
> ([05](05-PHASE-1-SELF-DESCRIBING-REGISTRY.md) P1.6 #5).

## Where each front-end enforces

- **`system.describe`** filters server-side by the caller's role *and* a requested
  tier ceiling — so an under-privileged or public caller never even *learns* about
  commands it may not run ([06](06-PHASE-2-INTROSPECTION-COMMAND.md)).
- **CLI** shows everything for local development but still hits `authorize()` on
  execution; against `--server`, identity is the server's.
- **Console** defaults to loopback + public/operator tiers; `admin`/`debug`
  require an explicit build/flag. Defense in depth: even if the UI showed an admin
  command, the server still role-gates it.
- **Agent** is given a tier ceiling; the projector never emits tools above it and
  the executor re-checks.

## Effectful commands & determinism

- `effectful: true` marks commands that touch host effects (network/disk/clock/
  random) via the policy layer (`forge-policy`). The console/agent warn before
  running these; the CLI notes them in `describe`.
- **Determinism is preserved** because:
  - `system.describe` is a pure read (no effects, no events);
  - the CLI issues only pre-existing commands whose replay behavior is unchanged;
  - effectful host-calls remain mediated by the bridge/policy exactly as today —
    the CLI does not open a new effect path.

## Debug / control surface

`control.*` is feature-gated (`feature = "control"`) and intersects the **retired
`/control` decision**. Treatment here:

- Default builds: `control.*` and other `debug`-tier commands are **excluded** from
  the public CLI/console/agent surface.
- They remain available under their existing feature flag for internal testing,
  described as `visibility: debug` so they are never accidentally surfaced.
- The unified CLI is positioned as the **principled replacement** for ad-hoc
  `DevControlPlane` invocation (F10), not an extension of the retired surface.

## Public-engine boundary

Per `CLAUDE.md`: no SaaS/control-plane concerns enter here. The CLI authenticates
to a *local* core or a *local* server; it does not implement identity, sessions,
billing, or hosted trust. A hosted product consumes this surface through
`artifacts/public-contract.json` or a pinned checkout — never a private fork.

## Threat checklist (review during build)

- [ ] CLI cannot forge actor identity against a server (server owns identity).
- [ ] `--role` is documented as local-dev only.
- [ ] No `public` command requires a privileged role (build test).
- [ ] `admin`/`debug` absent from default public console + agent.
- [ ] `system.describe` is role/tier-scoped; no info leak of privileged commands.
- [ ] Mutating/effectful commands are flagged and (console/agent) confirmed.
- [ ] Audit rows are produced for privileged/mutating CLI actions (inherited).
- [ ] Token handling (server) never logs/persists the bearer token.
