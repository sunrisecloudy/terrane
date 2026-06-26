# Phase 4 — The web command console

**Theme:** an operator UI that is *generated from the catalog* and submits
through the existing server `/bridge` endpoint. This is the "web scrape /
interface on top of all the commands" from the request. Because it renders the
catalog, it never drifts from what the core actually does.

**Risk:** low-moderate. It is a real frontend, but it contains **no command
knowledge** — only a schema-driven renderer over an existing endpoint.
**Replay impact:** none (it issues existing commands).

## Backend: already present

`forge-server` exposes everything the console needs:

- `POST /bridge` — generic command dispatch (`server/src/lib.rs:92`).
- `GET /health` (`:84`), `POST /events/drain` (`:96`).
- Optional bearer auth (`:157`).

The only backend addition is **serving the static console assets** (a new
`GET /` / `GET /console` static route) and ensuring `system.describe` is
reachable over `/bridge` (it is, once Phase 2 lands).

## Frontend: a thin renderer

```
  1. POST /bridge { name: "system.describe", payload: { tier: <max> } }
        → catalog (role/tier-scoped server-side)
  2. Render left-nav: namespaces → commands (badges: tier, mutates, effectful)
  3. Select a command → build a form from its payload_schema
        (string/number/bool/enum/object/array widgets; required markers)
  4. Submit → POST /bridge { name, payload } [Authorization: Bearer …]
        → render CoreResponse (pretty JSON + status)
  5. Optionally POST /events/drain → show emitted CoreEvents
```

### Form generation

A small JSON-Schema-to-form mapping (the same schemas Phase 1 authors):

| Schema | Widget |
| --- | --- |
| `string` (+`enum`) | text / select |
| `integer`/`number` | number input |
| `boolean` | checkbox |
| `object` | nested fieldset |
| `array` | repeatable row group |
| `$ref` | resolve + inline |

For commands without a schema yet (preview), fall back to a raw-JSON textarea —
so the console is useful immediately and improves as schemas land.

### Safety in the UI

- **Tier filter** drives what is shown; the public build hides `admin`/`debug`
  by default (server also enforces — defense in depth, see
  [10-SECURITY-AND-RBAC.md](10-SECURITY-AND-RBAC.md)).
- **`mutates` / `effectful` badges** and a confirm step for mutating/effectful
  commands.
- **Token field** for authed commands; never persisted beyond the session.
- The console targets **loopback by default** (operator tool), matching the
  existing `DevControlPlane` posture (F10).

## Where the code lives

Reuse, don't reinvent:

- The Node patterns in `tools/reference-host/` (`invokeForgeCore`,
  `bridge-dispatcher`) show how to talk to the surface from JS.
- `runtime-web/` shows the web build/runtime conventions to match.
- Decide static-asset hosting in [13](13-OPEN-QUESTIONS.md) Q4 (serve from
  `forge-server` vs. a separate `tools/console/` static app).

## Steps

- **P4.1** Add a static route in `forge-server` to serve console assets (behind a
  flag; off in headless deployments).
- **P4.2** Build the catalog fetch + left-nav (vanilla or the repo's existing web
  stack — match `runtime-web/`).
- **P4.3** Implement schema-driven form generation with raw-JSON fallback.
- **P4.4** Wire submit → `/bridge`, render response + drained events.
- **P4.5** Tier/visibility filtering + mutate/effect confirmations.
- **P4.6** Tests: a headless check that the console lists the catalog and that a
  `query.execute` submitted through it returns rows (drive via the existing web
  harness / a Playwright-style smoke if available).

## Deliverables

- A static console served (optionally) by `forge-server`.
- Schema-driven forms with raw-JSON fallback.
- Tier filtering + confirmations + token auth.
- A smoke test exercising list + run through the UI path.

## Validation

```sh
cd forge
cargo run -p forge-server            # serves /bridge (+ /console if enabled)
# then open the console, run query.execute, observe rows + events
```

## Exit criteria

- Selecting any visible command renders a usable form and runs it through
  `/bridge`.
- The console shows exactly the catalog `system.describe` returns for its tier —
  no hard-coded commands.
- `admin`/`debug` commands are absent from the default public build.
