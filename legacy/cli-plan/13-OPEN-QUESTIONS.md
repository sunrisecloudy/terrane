# 13 — Open Questions & Decisions

Decisions to lock before or during build. Each has a recommendation; none blocks
starting Phase 1, but Q1/Q2 should be settled early because they shape the
keystone.

## Q1 — Where does the catalog live?

In-Rust table (authoritative) vs. checked-in `forge/data/commands.json`.

- **Recommendation:** Rust table is authoritative; **emit** JSON from it for
  non-Rust tools. Avoids a second hand-maintained file drifting. Mirrors the
  `forge/data/*.json` extraction pattern but keeps Rust as the source.
- **Open:** do we still want `forge/data/commands.json` checked in (generated,
  CI-verified) so JS tools read it without running `system.describe`? Leaning
  yes, generated-and-verified.
- **Verified 2026-06-26:** the live registry has 42 outer commands; the current
  `CORE_COMMANDS` export list is missing 13 of them and lists 5 spec-only names
  with no handler (F11). M4 drift gate is required, not nice-to-have.

## Q2 — One role table, or two with a cross-check?

P1.4 can either move roles into the descriptor (and have `authorize()` read it) or
keep `authorize()` authoritative and derive/verify the descriptor.

- **Recommendation:** descriptor holds the role set; `authorize()` consults the
  catalog. Single home. Requires care that the catalog is available at the
  `authorize()` call site (it is static, so fine).
- **Risk:** this touches the security-critical path; gate behind the full
  `--workspace` test before merge.

## Q3 — Arg-parsing dependency for the CLI

`clap` vs. a minimal hand parser.

- **Recommendation:** match the workspace. If `clap` is already a dependency
  somewhere, use it (good help/UX, subcommands). If not, a tiny parser keeps
  `forge-cli` light and dependency-free (it currently hand-rolls `match`). Decide
  by checking the existing dep graph.

## Q4 — Console hosting

Serve console assets from `forge-server` vs. a standalone `tools/console/` static
app.

- **Recommendation:** serve from `forge-server` behind a flag (off in headless),
  so "run the server, open the console" is one step; keep assets buildable
  standalone too. Reuse `runtime-web/` conventions.

## Q5 — Transport default for the CLI

Open a local core directly vs. always go through a server.

- **Recommendation:** default to **opening a local core** (no server needed for
  local dev); `--server <url>` opts into HTTP. Matches the `core-invoke`
  ergonomics and keeps the CLI useful with zero setup.

## Q6 — `ctx.*` inner surface in `describe`

Should `forge describe ctx.net` work even though `forge run ctx.net` is rejected?

- **Recommendation:** yes — `describe` covers inner entries for documentation;
  `run` refuses them with a pointer to the app runtime. Keeps the catalog a
  complete reference without letting operators issue host-calls.

## Q7 — `control.*` / debug surface

Keep `debug`-tier commands compiled (feature-gated) but excluded from public
front-ends, or remove them from the catalog entirely?

- **Recommendation:** keep them feature-gated and `visibility: debug`, excluded
  from public CLI/console/agent and from the public contract. This honors the
  retired `/control` decision while preserving internal test tooling. Revisit if
  the `control` feature is removed outright.

## Q8 — Naming

The initiative name and the binary name.

- **Recommendation:** initiative **"Forge Unified CLI"**; binary stays **`forge`**
  (back-compat with `forge demo`). The folder is `cli-plan/` to match
  `linux-plan/`, `window-plan/`, `forge-core-plan/`.
- **Open:** confirm with the user — the request floated a name ("CLI-plan"); this
  matches the house convention. ✅ assumed unless overridden.

## Q9 — Shared envelope builder for shells (stretch)

F5 found each native shell duplicates envelope construction. Should the unified
CLI work also extract a shared builder the shells adopt?

- **Recommendation:** **out of scope for v1** of this initiative (it touches all
  five shells and overlaps `forge-core-plan`). Note it as a follow-up; the CLI
  builds the envelope in one Rust place that shells *could* later share.

## Q10 — Event streaming in the CLI/console

`run` returns the `CoreResponse`; emitted `CoreEvents` are drained separately
(`/events/drain`). Should `forge run` auto-drain and print events?

- **Recommendation:** `forge run --events` opts into draining + printing emitted
  events after the response; off by default to keep output clean. Console shows
  events by default.

---

## Decisions already assumed (call out if wrong)

- Folder name `cli-plan/`, branch `plan/unified-cli`, worktree at
  `../terrane-cli-plan` (matches `terrane-custom-theme` convention).
- Public-engine only; no SaaS concerns; Premium consumes via the contract.
- Determinism and existing RBAC are invariants, not subject to change for
  convenience.
- MVP = Phases 1→2→3; console and agent follow.
- **Two doors, not one** (resolved): JS host-calls (`ctx.*`: db/files/net/ui) do
  **not** route through the outer `handle(CoreCommand)` entrypoint. Outer and
  inner stay separate at execution but share one catalog, journal, policy, and
  observability surface. "Agent does anything the UI can" is achieved via
  `ui.dispatch_event`; inner effects are observable via `system.trace`. Full
  rationale and exit criteria in
  [14-EFFECT-SURFACE-AND-OBSERVABILITY.md](14-EFFECT-SURFACE-AND-OBSERVABILITY.md).
  This also resolves Q6 (inner entries are `describe`-only).
