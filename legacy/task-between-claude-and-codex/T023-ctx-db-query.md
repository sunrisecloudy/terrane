---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: forge/std/forge-std.d.ts (extend Db), forge/fixtures/e2e/query_*/ scenarios
---

# T023 — Applet-facing `ctx.db.query` surface (DL-15) + scenarios

The data loop built a real query engine in forge-storage (filters/order/aggregates/
LIKE/indexes), but applets can't reach it yet — `ctx.db` only has insert/get/list.
The next workflow wires `ctx.db.query` through the runtime HostBridge + core. I need
the typed surface + e2e scenarios that exercise it through the facade.

## Deliverables

1. Extend `forge/std/forge-std.d.ts` `Db` interface with a typed `query`:
   ```ts
   query(q: {
     from: string;
     where?: [string, "="|"!="|"<"|"<="|">"|">="|"in"|"like", JsonValue][];
     orderBy?: [string, "asc"|"desc"];
     limit?: number; offset?: number;
     // aggregate?/groupBy? optional, mark P1 if unsure
   }): Promise<DbRecord[]>;
   ```
   Match the structured query shape the storage engine already parses (read
   `forge/spec/query-dsl.md` + `forge/crates/storage/src/query.rs` Query type so the
   wire JSON the applet sends == what storage deserializes). Keep it strict-TS-clean.
2. Add e2e scenarios under `forge/fixtures/e2e/query_<name>/` (same 4-file shape as
   T018: applet.ts, manifest.json, input.json, expect.json) where the applet inserts
   several records then `ctx.db.query(...)`s them and renders/returns the results:
   - `query_filter_order` — insert tasks, query where priority > N order by date desc.
   - `query_limit` — insert several, query with limit, assert count.
   - `query_denied` — applet queries a collection its manifest does NOT grant db.read →
     expect CapabilityRequired/PermissionDenied (exact code per forge/spec/errors.md), no rows.
   `expect.json`: `{ "result": {...}, "query_rows": [...ids or fields...], "replay_identical": true }`.

## Notes

Keep applets deterministic. In `## Result`, flag any query feature the applet surface
should expose but the storage Query type doesn't support yet, so I scope the host
bridge `db_query` to exactly what's wired (and reject the rest with a clear error).
