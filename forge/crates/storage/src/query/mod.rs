//! Query DSL AST + planner over the `records` projection (DL-15/16/17).
//!
//! prd-merged/02-data-layer-prd.md §6 and `forge/spec/query-dsl.md` pin the v1
//! query surface. This module is the storage-internal half: a typed [`Query`]
//! AST that (de)serializes directly from the Codex fixture plans, a planner that
//! compiles the AST to **parameterized** SQLite over the rebuildable `records`
//! projection (via JSON1 `json_extract`), and a [`QueryResult`] row/aggregate
//! set.
//!
//! DL-16 contract: raw SQL is never exposed. The only query surface is this
//! AST; record values are always bound as SQL parameters, never interpolated
//! into the statement text. A SQL-like string is accepted only after passing
//! [`reject_raw_sql`], which compiles to the same AST rather than executing
//! caller SQL.
//!
//! ## Field addressing
//!
//! Query plans address fields two ways and the planner resolves each to a
//! distinct canonical envelope JSON path:
//!
//! - **Display name** (`status`, `prio`) → `$.fields."<name>"` — applet
//!   ergonomics; the query-DSL surface.
//! - **Stable field id** (`f_alice_1`) → `$.field_ids."<id>"` — the merge/index
//!   correct addressing the dynamic-index engine and its fixtures use
//!   (`dynamic-indexes.md`). A `field_id` key in a plan resolves to the
//!   stable-id path, never the display path.
//!
//! The leaf key is always **double-quoted** in the JSON path so an identifier
//! containing a `.` (e.g. a field id `f_dev.01_0`) addresses the literal key
//! rather than a nested JSON1 path. See [`FieldRef::json_path`].
//!
//! Either path component is validated against an identifier allowlist before it
//! is placed in the (otherwise constant) statement text, so a field reference
//! can never carry SQL (DL-16).
//!
//! ## Semantics (pinned by `query-dsl.md` §Result)
//!
//! - Comparisons **do not coerce types**: `"2" > 10` is a [`CoreError::QueryError`],
//!   not silently `false`.
//! - A missing field compares as JSON `null` only for `eq(null)` / `ne(null)`;
//!   range/`like`/`in` over a missing or `null` value is `false`.
//! - Sort order is numbers < strings < booleans < nulls-last, with `entity_id`
//!   as the stable secondary key. Ordering is resolved in Rust so the total
//!   order is identical on every platform (SQLite's native affinity ordering
//!   does not match this spec rule).
//! - `LIKE` uses `%`/`_` with backslash escape and is ASCII case-insensitive,
//!   matching SQLite's portable default.
//!
//! This module is split into directory sub-modules (/simplify #8) — `ast`,
//! `parse`, `compile`, `order_aggregate`, `warnings`, `mutation`, `guard`, and
//! `json_path` — re-exported here so `crate::query::*` paths stay byte-stable.
//!
//! [`CoreError::QueryError`]: forge_domain::CoreError::QueryError

mod ast;
mod compile;
mod guard;
mod json_path;
mod mutation;
mod order_aggregate;
mod parse;
mod warnings;

pub use ast::{Aggregate, Dir, FieldRef, Filter, Op, OrderBy, Predicate, Query, TextSearch};
pub use compile::{compile_select, CompiledSelect};
pub use guard::reject_raw_sql;
pub use mutation::Mutation;
pub use order_aggregate::{
    cmp_json_pub, compute_aggregate, finalize_rows, group_key, AggregateResult, GroupResult,
    QueryResult, QueryRow,
};
pub use warnings::{FullScanReason, PlannedQuery, PlannerWarning};

// Crate-internal helpers other storage modules (`index`, `query_exec`) reach as
// `crate::query::<name>`. Re-exported here so those paths stay stable after the
// directory split (/simplify #8); not part of the public API.
pub(crate) use guard::validate_index_ident;
// `quote_json_path_key` stays module-private (used inside `ast`/`json_path`); only
// `field_id_json_path` is reached from a sibling storage module (`index`).
pub(crate) use json_path::field_id_json_path;

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::RecordEnvelope;
    use serde_json::json;

    fn plan(v: serde_json::Value) -> Query {
        Query::from_fixture_value(&v).expect("parse plan")
    }

    // --- AST parse: both fixture shapes ----------------------------------

    #[test]
    fn parses_array_tuple_leaf() {
        let q = plan(json!({"from": "tasks", "where": ["status", "=", "todo"]}));
        assert_eq!(q.from, "tasks");
        match q.filter.unwrap() {
            Filter::Leaf(p) => {
                assert_eq!(p.field, FieldRef::Name("status".into()));
                assert_eq!(p.op, Op::Eq);
                assert_eq!(p.value, json!("todo"));
            }
            other => panic!("expected leaf, got {other:?}"),
        }
    }

    #[test]
    fn parses_object_leaf_named_op() {
        let q = plan(json!({"from": "tasks", "where": {"field": "prio", "op": "gt", "value": 2}}));
        match q.filter.unwrap() {
            Filter::Leaf(p) => {
                assert_eq!(p.op, Op::Gt);
                assert_eq!(p.value, json!(2));
            }
            other => panic!("expected leaf, got {other:?}"),
        }
    }

    #[test]
    fn parses_nested_and_or() {
        let q = plan(json!({
            "from": "tasks",
            "where": {"and": [["status", "=", "todo"], {"or": [["prio", ">", 2], ["tag", "=", "home"]]}]}
        }));
        match q.filter.unwrap() {
            Filter::And(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[1], Filter::Or(_)));
            }
            other => panic!("expected AND, got {other:?}"),
        }
    }

    #[test]
    fn parses_order_limit_offset_both_forms() {
        let a = plan(json!({"from": "t", "orderBy": ["prio", "desc"], "limit": 5, "offset": 2}));
        let ob = a.order_by.unwrap();
        assert_eq!(ob.field, FieldRef::Name("prio".into()));
        assert_eq!(ob.dir, Dir::Desc);
        assert_eq!(a.limit, Some(5));
        assert_eq!(a.offset, Some(2));

        let b = plan(json!({"from": "t", "order_by": [{"field": "prio", "dir": "asc"}]}));
        assert_eq!(b.order_by.unwrap().dir, Dir::Asc);
    }

    // --- validation: identifiers + bad shapes ----------------------------

    #[test]
    fn rejects_field_with_sql_metacharacters() {
        let err = Query::from_fixture_value(&json!({
            "from": "tasks",
            "where": ["status'); DROP TABLE records;--", "=", "x"]
        }))
        .unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn rejects_negative_limit() {
        let err = Query::from_fixture_value(&json!({"from": "t", "limit": -1})).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn rejects_unknown_operator() {
        let err =
            Query::from_fixture_value(&json!({"from": "t", "where": ["a", "~~", 1]})).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    // --- compile: parameterization (DL-16) -------------------------------

    #[test]
    fn values_are_bound_never_interpolated() {
        let q = plan(json!({"from": "tasks", "where": ["status", "=", "todo'; DROP"]}));
        let c = compile_select(&q).unwrap();
        // The dangerous value is a bound parameter, not in the SQL text.
        assert!(
            !c.sql.contains("DROP"),
            "value must not appear in SQL: {}",
            c.sql
        );
        assert!(c.sql.contains("?"), "predicate must use a placeholder");
        assert!(c.params.iter().any(|p| p == &json!("todo'; DROP")));
    }

    #[test]
    fn compile_in_lists_one_placeholder_per_value() {
        let q = plan(json!({"from": "t", "where": ["status", "in", ["a", "b", "c"]]}));
        let c = compile_select(&q).unwrap();
        // 1 (collection) + 3 (in values).
        assert_eq!(c.params.len(), 4);
    }

    #[test]
    fn empty_in_is_rejected() {
        let q = plan(json!({"from": "t", "where": ["status", "in", []]}));
        let err = compile_select(&q).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    // --- no boolean<->number coercion (review 040 finding 3) --------------

    #[test]
    fn eq_boolean_compiles_a_json_type_guard() {
        // `done.eq(false)` must require the stored value to be a JSON boolean, so
        // it cannot also match a stored numeric 0. A non-boolean operand keeps the
        // plain equality (no needless guard).
        let qb = plan(json!({"from": "t", "where": ["done", "=", false]}));
        let cb = compile_select(&qb).unwrap();
        assert!(
            cb.sql.contains("json_type") && cb.sql.contains("'true','false'"),
            "boolean eq must carry the type guard: {}",
            cb.sql
        );
        let qs = plan(json!({"from": "t", "where": ["status", "=", "todo"]}));
        let cs = compile_select(&qs).unwrap();
        assert!(
            !cs.sql.contains("json_type"),
            "string eq needs no guard: {}",
            cs.sql
        );
    }

    #[test]
    fn ne_boolean_compiles_a_json_type_guard() {
        // `done.ne(false)` must treat a stored numeric 0 (or a missing field) as
        // differing, so the compiled predicate guards on json_type.
        let q = plan(json!({"from": "t", "where": ["done", "!=", true]}));
        let c = compile_select(&q).unwrap();
        assert!(
            c.sql.contains("NOT IN ('true','false')"),
            "boolean ne must carry the type guard: {}",
            c.sql
        );
    }

    #[test]
    fn in_with_boolean_member_guards_each_term() {
        // `in [false]` is a disjunction of type-guarded equality terms, so a
        // boolean member cannot coerce-match a stored numeric 0.
        let q = plan(json!({"from": "t", "where": ["done", "in", [false]]}));
        let c = compile_select(&q).unwrap();
        assert!(
            c.sql.contains("json_type") && c.sql.contains("'true','false'"),
            "boolean `in` member must carry the type guard: {}",
            c.sql
        );
    }

    // --- dotted field-id JSON paths (review 041 finding 5) ----------------

    #[test]
    fn dotted_field_id_path_is_double_quoted() {
        // A stable field id containing a `.` (mintable from an actor id like
        // `dev.01`) must address the literal key `$.field_ids."f_dev.01_0"`, not
        // the nested path `$.field_ids.f_dev.01.0` (which json1 reads as NULL).
        let q = Query::from_fixture_value(&json!({
            "from": "tasks",
            "where": [{"field_id": "f_dev.01_0", "op": "eq", "value": "x"}]
        }))
        .unwrap();
        let c = compile_select(&q).unwrap();
        assert!(
            c.sql.contains("$.field_ids.\"f_dev.01_0\""),
            "dotted field id must use a quoted JSON path: {}",
            c.sql
        );
        // And the canonical helper agrees.
        assert_eq!(
            field_id_json_path("f_dev.01_0"),
            "$.field_ids.\"f_dev.01_0\""
        );
    }

    #[test]
    fn display_name_path_is_double_quoted_too() {
        let q = plan(json!({"from": "t", "where": ["status", "=", "todo"]}));
        let c = compile_select(&q).unwrap();
        assert!(
            c.sql.contains("$.fields.\"status\""),
            "display name must use a quoted JSON path: {}",
            c.sql
        );
    }

    // --- coercion rule: no type coercion (query-dsl.md §Result) -----------

    #[test]
    fn range_op_rejects_non_numeric_operand() {
        // `"2" > 10`-style: a string operand to a range op is a coercion error.
        let q = plan(json!({"from": "t", "where": ["amount", ">", "10"]}));
        let err = compile_select(&q).unwrap_err();
        assert_eq!(err.code(), "QueryError", "{err}");
    }

    #[test]
    fn range_op_accepts_numeric_operand() {
        let q = plan(json!({"from": "t", "where": ["amount", ">=", 10]}));
        assert!(compile_select(&q).is_ok());
    }

    // --- ordering: spec total order --------------------------------------

    fn env_with(id: &str, field: &str, value: serde_json::Value) -> RecordEnvelope {
        let mut fields = std::collections::BTreeMap::new();
        if !value.is_null() {
            fields.insert(field.to_string(), value);
        }
        RecordEnvelope {
            envelope_version: 1,
            entity_id: forge_domain::RecordId::new(id),
            collection: forge_domain::CollectionId::new("t"),
            fields,
            field_ids: Default::default(),
            unknown_fields: Default::default(),
            extensions: Default::default(),
            created_at: Default::default(),
            updated_at: Default::default(),
            deleted: false,
        }
    }

    fn rows(items: Vec<RecordEnvelope>) -> Vec<QueryRow> {
        items
            .into_iter()
            .map(|e| QueryRow {
                id: e.entity_id.as_str().to_string(),
                envelope: e,
            })
            .collect()
    }

    /// A display-name [`FieldRef`] for terse test construction.
    fn name(s: &str) -> FieldRef {
        FieldRef::Name(s.to_string())
    }

    #[test]
    fn order_is_numbers_then_strings_then_bools_then_nulls() {
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Asc,
        });
        let input = rows(vec![
            env_with("e_null", "v", json!(null)),
            env_with("e_bool", "v", json!(true)),
            env_with("e_str", "v", json!("a")),
            env_with("e_num", "v", json!(5)),
        ]);
        let out = finalize_rows(input, &q);
        let ids: Vec<_> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["e_num", "e_str", "e_bool", "e_null"]);
    }

    #[test]
    fn entity_id_tie_break_is_always_ascending_even_for_desc() {
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Desc,
        });
        // Same primary value (1); ids must still ascend within the tie.
        let input = rows(vec![
            env_with("b", "v", json!(1)),
            env_with("a", "v", json!(1)),
        ]);
        let out = finalize_rows(input, &q);
        let ids: Vec<_> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn nulls_sort_last_independent_of_direction() {
        // review 040 finding 8: descending must NOT float nulls to the front.
        // A missing/null value sorts LAST for both asc and desc; only the
        // present values reverse.
        let input = vec![
            env_with("e_null", "v", json!(null)),
            env_with("e1", "v", json!(1)),
            env_with("e3", "v", json!(3)),
        ];
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Asc,
        });
        let asc: Vec<_> = finalize_rows(rows(clone_envs(&input)), &q)
            .iter()
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(asc, vec!["e1", "e3", "e_null"], "asc: nulls last");

        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Desc,
        });
        let desc: Vec<_> = finalize_rows(rows(clone_envs(&input)), &q)
            .iter()
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(
            desc,
            vec!["e3", "e1", "e_null"],
            "desc: values reversed but nulls STILL last"
        );
    }

    #[test]
    fn order_by_entity_id_desc_is_a_real_sort_key() {
        // review 040 finding 8: orderBy("id","desc") must actually descend by
        // entity id, not collapse to the ascending tie-break.
        let input = rows(vec![
            env_with("a", "v", json!(1)),
            env_with("c", "v", json!(1)),
            env_with("b", "v", json!(1)),
        ]);
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("id"),
            dir: Dir::Desc,
        });
        let ids: Vec<_> = finalize_rows(input, &q)
            .iter()
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(ids, vec!["c", "b", "a"], "id desc descends by entity id");

        let input2 = rows(vec![
            env_with("a", "v", json!(1)),
            env_with("c", "v", json!(1)),
            env_with("b", "v", json!(1)),
        ]);
        q.order_by = Some(OrderBy {
            field: name("entity_id"),
            dir: Dir::Asc,
        });
        let ids2: Vec<_> = finalize_rows(input2, &q)
            .iter()
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(ids2, vec!["a", "b", "c"], "entity_id asc ascends");
    }

    /// Clone a slice of envelopes (helper for re-running finalize_rows on the
    /// same input under two directions).
    fn clone_envs(items: &[RecordEnvelope]) -> Vec<RecordEnvelope> {
        items.to_vec()
    }

    #[test]
    fn offset_then_limit_applied_after_order() {
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Asc,
        });
        q.offset = Some(1);
        q.limit = Some(2);
        let input = rows(vec![
            env_with("e1", "v", json!(1)),
            env_with("e2", "v", json!(2)),
            env_with("e3", "v", json!(3)),
            env_with("e4", "v", json!(4)),
        ]);
        let out = finalize_rows(input, &q);
        let ids: Vec<_> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["e2", "e3"]);
    }

    // --- aggregates -------------------------------------------------------

    #[test]
    fn aggregate_sum_avg_min_max_over_numbers() {
        let agg = Aggregate {
            count: true,
            sum: Some(name("v")),
            avg: Some(name("v")),
            min: Some(name("v")),
            max: Some(name("v")),
        };
        let e1 = env_with("a", "v", json!(2));
        let e2 = env_with("b", "v", json!(4));
        let e3 = env_with("c", "v", json!(6));
        let refs = vec![&e1, &e2, &e3];
        let out = compute_aggregate(&refs, &agg);
        assert_eq!(out.count, Some(3));
        assert_eq!(out.sum, Some(12.0));
        assert_eq!(out.avg, Some(4.0));
        assert_eq!(out.min, Some(json!(2)));
        assert_eq!(out.max, Some(json!(6)));
    }

    #[test]
    fn aggregate_over_empty_is_count_zero_sum_zero_none_else() {
        let agg = Aggregate {
            count: true,
            sum: Some(name("v")),
            avg: Some(name("v")),
            min: Some(name("v")),
            max: Some(name("v")),
        };
        let out = compute_aggregate(&[], &agg);
        assert_eq!(out.count, Some(0));
        assert_eq!(out.sum, Some(0.0));
        assert_eq!(out.avg, None);
        assert_eq!(out.min, None);
        assert_eq!(out.max, None);
    }

    // --- raw-SQL rejection (DL-16) ---------------------------------------

    #[test]
    fn reject_raw_sql_blocks_ddl_dml_and_terminators() {
        for bad in [
            "DROP TABLE records",
            "SELECT * FROM t; DELETE FROM t",
            "SELECT 1 -- comment",
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET x = 1",
            "PRAGMA table_info(records)",
        ] {
            assert!(reject_raw_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn reject_raw_sql_allows_a_read_only_select() {
        assert!(reject_raw_sql("SELECT id FROM tasks WHERE prio > 1").is_ok());
    }
}
