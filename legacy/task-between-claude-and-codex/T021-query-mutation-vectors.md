---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/spec/query-dsl.md, forge/fixtures/query/*.json, forge/fixtures/query/manifest.json
---

# T021 — Query DSL + mutation test vectors (DL-15/16/17 — feeds the data-loop workflow)

The next forge feature area is the data loop: the typed query DSL + mutations over
the records projection (prd-merged/02 DL-15/16/17). I'll build the Rust query
planner; I want the grammar pinned + a vector corpus first.

## Deliverables

1. `forge/spec/query-dsl.md` — the M0a/v1 query surface:
   - the applet-facing DSL (DL-15): `db.from(c).where(f => f.x.gt(n)).orderBy(field,
     dir).limit(n)` — enumerate the operators (filter eq/ne/lt/le/gt/ge/in/like,
     and/or; orderBy; limit/offset; aggregates count/sum/avg/min/max; group;
     text-search; join on declared reference fields) and which are M0a vs P1.
   - the SQL-like string form (`query.execute`) with the SAME validated subset
     (prd-merged/05 UI-17 data browser uses it). Reference forge/spec/commands.md.
   - mutations (DL-16/17): insert/update/patch/delete + transact (one CRDT commit).
   - the rule that raw SQL is NEVER exposed to applets (DL-16).
2. `forge/fixtures/query/<case>.json` + manifest — each: a small set of seed records
   (RecordEnvelope shape, see forge/crates/domain/src/record.rs), a query (DSL form
   AND/or the SQL-like string), and the expected ordered result rows.
   ```json
   { "case": "where_gt_orderby_desc_limit",
     "seed": [ {"collection":"tasks","fields":{"title":"a","prio":1}}, ... ],
     "query": { "from": "tasks", "where": [["prio",">",1]], "orderBy": ["prio","desc"], "limit": 2 },
     "expect_ids": ["tasks/3","tasks/2"] }
   ```

## Coverage (~18)

filter by each operator; AND/OR; orderBy asc/desc; limit/offset; count/sum/avg;
group-by; text/LIKE; a join on a reference field; empty result; a mutation sequence
(insert→patch→delete) with expected post-state; a transact group; and a rejected
case (raw SQL attempt, or a filter on an ungranted collection → expected error).

In `## Result`, flag any operator whose semantics you're unsure of (esp. null
handling, type coercion in comparisons, LIKE escaping) so I pin the Rust planner's
behavior to the vectors rather than guessing.
