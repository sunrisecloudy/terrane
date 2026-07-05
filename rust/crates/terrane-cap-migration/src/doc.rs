use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, InternalNote, QueryDoc, ResourceDoc, SchemaDoc,
};

use crate::{MAX_RECORDED_EVENTS_PER_STEP, MAX_SCRIPT_BYTES};

pub fn migration_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "migration".to_string(),
        title: "Schema Migration".to_string(),
        summary: "Versioned app-data migrations recorded as forward replayable events."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "migration.apply".to_string(),
                "migration.commit".to_string(),
            ],
            queries: vec!["migration.status".to_string()],
            events: vec!["migration.applied".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: migration_commands(),
        queries: migration_queries(),
        events: migration_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: Vec::new(),
        constraints: vec![
            "Migrations are forward-only; no log rewriting and no down migration mechanism."
                .to_string(),
            "Migration scripts run once through the JS runtime path; replay folds recorded kv, relational_db, and migration events without re-running JavaScript."
                .to_string(),
            "Apps do not receive ctx.resource.migration; hosts apply manifest-declared migrations explicitly."
                .to_string(),
            "Each step must move exactly one version forward.".to_string(),
        ],
        limits: vec![
            limit(
                "scriptBytes",
                &MAX_SCRIPT_BYTES.to_string(),
                "Migration source accepted by migration.apply.",
            ),
            limit(
                "recordedEventsPerStep",
                &MAX_RECORDED_EVENTS_PER_STEP.to_string(),
                "Large rewrites should be split across data versions.",
            ),
        ],
        compatibility: vec![
            "Apps with no migration.applied event are treated as data version 1."
                .to_string(),
            "On app.removed, folded migration state for that app is dropped.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Runtime-internal commit".to_string(),
                body: "migration.commit is for trusted host/runtime use so the final version fact lands in the same runtime batch as data writes."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn migration_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "migration.apply",
            &[
                param("app", "Target app id.", "app_id"),
                param("to_version", "Next data version.", "integer"),
                param("script_source", "JavaScript defining migrate(ctx).", "js"),
            ],
            "runtime output",
            "Run one manifest-declared migration step through the JS runtime.",
        )
        .with_errors(&["missing app", "non-consecutive version", "script too large"])
        .with_emits(&["kv.*", "relational_db.*", "migration.applied"]),
        command_doc(
            "migration.commit",
            &[
                param("app", "Target app id.", "app_id"),
                param("from", "Current folded version.", "integer"),
                param("to", "Next folded version.", "integer"),
                param("script_hash", "SHA-256 of script source.", "sha256_hex"),
            ],
            "events",
            "Record the final version fact for a migration runtime batch.",
        )
        .with_errors(&["trusted host required", "version mismatch", "bad script hash"])
        .with_emits(&["migration.applied"]),
    ]
}

fn migration_queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "migration.status",
        &[param("app", "Target app id.", "app_id")],
        "JSON { app, version, history }",
        "Read the folded migration version and applied-step history.",
    )
    .with_errors(&["missing app"])]
}

fn migration_events() -> Vec<EventDoc> {
    vec![event_doc(
        "migration.applied",
        &[
            param("app", "Target app id.", "app_id"),
            param("from_version", "Previous data version.", "integer"),
            param("to_version", "Applied data version.", "integer"),
            param("script_hash", "SHA-256 of script source.", "sha256_hex"),
        ],
        "Records a completed app-data migration step.",
    )]
}
