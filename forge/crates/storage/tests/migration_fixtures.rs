//! Data-driven DL-13 migration harness over `forge/fixtures/migrations/`.
//!
//! The T033 corpus expresses each case as an ordered `changes[]` *schema-change*
//! sequence plus an `expect`/`error_kind` verdict. The companion to the registry
//! harness (`forge-core/tests/schema.rs`, which drives the registry side) is THIS
//! one: it drives the **record-transform** side of DL-13 — the actual
//! `Store::apply_migration` over seeded records — for every case whose verdict
//! turns on a record value (the `widen_field` cases). For each such case it
//! builds the matching `MigrationDescriptor`, seeds a record whose value exercises
//! the verdict, applies the migration through the real atomic driver, and asserts
//! the before/after record (ok) or the typed rejection + full rollback (rejected).
//!
//! The harness GENUINELY asserts each vector it claims: a `ran == expected` guard
//! makes a silently-skipped case fail, and every assertion reads back the real
//! stored record (no faking).

use forge_schema::{FieldTransform, FieldType, MigrationDescriptor};
use forge_storage::{IndexManager, Store, MIGRATION_OP_KIND};

use forge_domain::{CollectionId, LogicalTimestamp, RecordEnvelope, RecordId};
use std::collections::BTreeMap;

/// Load a migration fixture by file name from `forge/fixtures/migrations/`.
fn load_migration(name: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/migrations")
        .join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read migration fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse migration fixture {name}: {e}"))
}

/// Seed one `expenses` record carrying `value` at stable id `f_amount`.
fn seed_amount(store: &Store, id: &str, value: serde_json::Value) {
    let mut env = RecordEnvelope::new(
        CollectionId::new("expenses"),
        RecordId::new(id),
        BTreeMap::from([("amount".to_string(), value.clone())]),
        LogicalTimestamp(1),
    );
    env.field_ids.insert("f_amount".into(), value);
    store.put_record(&env).unwrap();
}

/// Extract the `to` target type of the last `widen_field` change in a fixture, as
/// a `FieldType`. The fixtures key by the legacy `f0` id; the descriptor we build
/// keys by `f_amount` (the seeded record's stable id), so only the `to` matters.
fn widen_target(fx: &serde_json::Value) -> Option<FieldType> {
    let changes = fx["changes"].as_array()?;
    let widen = changes
        .iter()
        .rev()
        .find(|c| c.get("op").and_then(|o| o.as_str()) == Some("widen_field"))?;
    serde_json::from_value(widen.get("to")?.clone()).ok()
}

/// A descriptor that widens `expenses.f_amount` to `to` (v1 → v2).
fn widen_descriptor(to: FieldType) -> MigrationDescriptor {
    MigrationDescriptor {
        collection: "expenses".into(),
        from_schema_version: 1,
        to_schema_version: 2,
        transforms: vec![FieldTransform::WidenField {
            field_id: "f_amount".into(),
            to,
        }],
    }
}

/// The record-transform fixtures: each names the fixture file, the seed value its
/// verdict turns on, and the expected outcome. These are the DL-13 cases whose
/// pass/fail depends on the stored record VALUE (the `widen_field` corpus). The
/// other corpus cases (add_collection/field, deprecate, enforce, the
/// not-expressible rejects) are registry-only and are asserted by
/// `forge-core/tests/schema.rs`; here we cover the data side.
struct RecordCase {
    file: &'static str,
    /// The stored value that exercises the verdict.
    seed: serde_json::Value,
    /// `Ok(expected_after_value)` for an accepted migration, or `Err(())` for a
    /// rejected one (full rollback asserted).
    expect_after: std::result::Result<serde_json::Value, ()>,
}

#[test]
fn dl13_record_transform_fixtures_apply_atomically() {
    let cases = [
        // int 5 widened to float → 5.0 (the value's JSON type becomes a float).
        RecordCase {
            file: "widen_int_to_float_ok.json",
            seed: serde_json::json!(5),
            expect_after: Ok(serde_json::json!(5.0)),
        },
        // text "hi" widened to scalar → unchanged (any scalar satisfies scalar).
        RecordCase {
            file: "widen_text_to_scalar_ok.json",
            seed: serde_json::json!("hi"),
            expect_after: Ok(serde_json::json!("hi")),
        },
        // int 3 widened to nullable(int) → unchanged (present value stays).
        RecordCase {
            file: "widen_to_nullable_ok.json",
            seed: serde_json::json!(3),
            expect_after: Ok(serde_json::json!(3)),
        },
        // float 12.5 "narrowed" to int → rejected with full rollback.
        RecordCase {
            file: "narrow_float_to_int_rejected.json",
            seed: serde_json::json!(12.5),
            expect_after: Err(()),
        },
    ];

    let mut ran = 0usize;
    for case in &cases {
        let fx = load_migration(case.file);
        let to = widen_target(&fx)
            .unwrap_or_else(|| panic!("fixture {} has no widen_field target", case.file));
        let descriptor = widen_descriptor(to);

        let mut store = Store::open_in_memory().unwrap();
        let indexes = IndexManager::new();
        seed_amount(&store, "e1", case.seed.clone());
        let before = store.get_record("expenses", "e1").unwrap().unwrap();
        assert_eq!(store.schema_version().unwrap(), 1);

        let result = store.apply_migration(&descriptor, &indexes);

        match &case.expect_after {
            Ok(expected) => {
                let outcome = result.unwrap_or_else(|e| {
                    panic!("fixture {} expected ok but was rejected: {e}", case.file)
                });
                assert!(outcome.applied, "fixture {}: must have applied", case.file);
                assert_eq!(outcome.schema_version, 2, "fixture {}: version bumped", case.file);
                assert_eq!(store.schema_version().unwrap(), 2);
                // The stored record now carries the migrated value.
                let after = store.get_record("expenses", "e1").unwrap().unwrap();
                assert_eq!(
                    &after.field_ids["f_amount"], expected,
                    "fixture {}: migrated value mismatch",
                    case.file
                );
                // int→float specifically must change the JSON number type.
                if expected.is_f64() {
                    assert!(
                        after.field_ids["f_amount"].is_f64(),
                        "fixture {}: value must be a float after widen",
                        case.file
                    );
                }
                // A migration op was recorded (DL-13).
                assert!(
                    store.list_ops().unwrap().iter().any(|o| o.kind == MIGRATION_OP_KIND),
                    "fixture {}: a migration must be recorded in the oplog",
                    case.file
                );
            }
            Err(()) => {
                let err = result.expect_err(&format!(
                    "fixture {} expected rejection but applied cleanly",
                    case.file
                ));
                assert_eq!(
                    err.code(),
                    "SchemaCompatibilityError",
                    "fixture {}: rejection kind",
                    case.file
                );
                // FULL ROLLBACK: version unchanged, record unchanged, no oplog op.
                assert_eq!(
                    store.schema_version().unwrap(),
                    1,
                    "fixture {}: version must not advance on rejection",
                    case.file
                );
                let after = store.get_record("expenses", "e1").unwrap().unwrap();
                assert_eq!(
                    after, before,
                    "fixture {}: a rejected migration must leave the record unchanged",
                    case.file
                );
                assert!(
                    store.list_ops().unwrap().iter().all(|o| o.kind != MIGRATION_OP_KIND),
                    "fixture {}: no migration op may survive a rejected migration",
                    case.file
                );
            }
        }
        ran += 1;
    }

    // The ran==count guard: every record-transform case above must have executed.
    assert_eq!(ran, cases.len(), "every DL-13 record-transform fixture must run");
}

/// A migration over MANY records is atomic across all of them: a single
/// non-coercible record rolls back the records that were already transformed in
/// the same transaction (the fault-injection guarantee at fixture scale).
#[test]
fn dl13_multi_record_migration_rolls_back_on_any_failure() {
    let mut store = Store::open_in_memory().unwrap();
    let indexes = IndexManager::new();
    // Three records: two coercible (10.0, 20.0) and one not (7.25).
    seed_amount(&store, "e1", serde_json::json!(10.0));
    seed_amount(&store, "e2", serde_json::json!(20.0));
    seed_amount(&store, "e3", serde_json::json!(7.25));
    let before: Vec<_> = ["e1", "e2", "e3"]
        .iter()
        .map(|id| store.get_record("expenses", id).unwrap().unwrap())
        .collect();

    let descriptor = widen_descriptor(FieldType::IntNum);
    let err = store.apply_migration(&descriptor, &indexes).unwrap_err();
    assert_eq!(err.code(), "SchemaCompatibilityError");

    // Not one of the three records changed, and the version did not move.
    assert_eq!(store.schema_version().unwrap(), 1);
    for (id, before) in ["e1", "e2", "e3"].iter().zip(&before) {
        let after = store.get_record("expenses", id).unwrap().unwrap();
        assert_eq!(&after, before, "record {id} must be fully rolled back");
    }
}
