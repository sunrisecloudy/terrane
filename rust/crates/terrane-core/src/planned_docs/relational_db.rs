use terrane_cap_interface::{
    limit, param, resource_method, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, ResourceDoc, SchemaDoc,
};
pub fn relational_db_doc(include_internal: bool) -> CapabilityDoc {
    let resource_methods = vec![
        resource_method(
            "defineTable",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param(
                    "specJson",
                    "Table specification JSON.",
                    "table_spec.schema.json",
                ),
            ],
            "Create or evolve a table definition.",
        ),
        resource_method(
            "dropTable",
            "write",
            &[param(
                "table",
                "Stable table name.",
                "table_name.schema.json",
            )],
            "Drop a table and its rows.",
        ),
        resource_method(
            "insert",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("rowJson", "Row JSON.", "row.schema.json"),
            ],
            "Insert or replace one row.",
        ),
        resource_method(
            "patch",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("id", "Primary key value.", ""),
                param("patchJson", "Partial row JSON.", "patch.schema.json"),
            ],
            "Patch one row by primary key.",
        ),
        resource_method(
            "delete",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("id", "Primary key value.", ""),
            ],
            "Delete one row by primary key.",
        ),
        resource_method(
            "get",
            "read",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("id", "Primary key value.", ""),
            ],
            "Read one row by primary key.",
        ),
        resource_method(
            "query",
            "read",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("queryJson", "Structured query JSON.", "query.schema.json"),
            ],
            "Run a structured query without exposing raw SQL.",
        ),
        resource_method("tables", "read", &[], "List table names."),
        resource_method(
            "describeTable",
            "read",
            &[param(
                "table",
                "Stable table name.",
                "table_name.schema.json",
            )],
            "Return a table specification.",
        ),
    ];
    CapabilityDoc {
        namespace: "relational_db".to_string(),
        title: "Relational DB".to_string(),
        summary: "Planned typed table storage and structured queries for app-owned data."
            .to_string(),
        status: "planned".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "relational_db.tableCreate".to_string(),
                "relational_db.tableDrop".to_string(),
                "relational_db.rowInsert".to_string(),
                "relational_db.rowPatch".to_string(),
                "relational_db.rowDelete".to_string(),
                "relational_db.query".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "relational_db.table.created".to_string(),
                "relational_db.table.dropped".to_string(),
                "relational_db.row.upserted".to_string(),
                "relational_db.row.deleted".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods.clone(),
        },
        resources: vec![ResourceDoc {
            namespace: "relational_db".to_string(),
            summary: "App-scoped relational tables planned to be backed by kv records."
                .to_string(),
            methods: resource_methods,
        }],
        schemas: relational_db_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Create a tasks table".to_string(),
                summary: "Define a table with a string primary key and typed fields.".to_string(),
                language: "js".to_string(),
                code: r#"ctx.resource.relational_db.defineTable("tasks", JSON.stringify({
  primaryKey: "id",
  fields: {
    id: { type: "string", required: true },
    title: { type: "string", required: true },
    done: { type: "boolean", required: true, default: false }
  }
}));"#
                .to_string(),
                expected: "table created".to_string(),
            },
            ExampleDoc {
                title: "Query incomplete tasks".to_string(),
                summary: "Use the structured query subset instead of raw SQL.".to_string(),
                language: "js".to_string(),
                code: r#"ctx.resource.relational_db.query("tasks", JSON.stringify({
  where: { field: "done", op: "eq", value: false },
  orderBy: [{ field: "title", direction: "asc" }],
  limit: 100
}));"#
                .to_string(),
                expected: r#"[{"id":"task_1","title":"Draft plan","done":false}]"#.to_string(),
            },
        ],
        constraints: vec![
            "Raw SQL is never exposed.".to_string(),
            "Table and field names must be portable identifiers.".to_string(),
            "Tables and rows are app-scoped.".to_string(),
            "Writes must be recorded as deterministic events.".to_string(),
            "Reads are derived from folded state and are not recorded.".to_string(),
            "Migrations are limited to additive fields, field deprecation, and indexes until a migration engine lands.".to_string(),
        ],
        limits: vec![
            limit("maxTablesPerApp", "64", "Keeps generated apps within local-first bounds."),
            limit("maxFieldsPerTable", "128", "Bounds schema validation and generated docs."),
            limit("maxIndexesPerTable", "16", "Bounds query planning metadata."),
            limit("maxQueryLimit", "1000", "Prevents unbounded agent/app reads."),
            limit("maxRowBytes", "65536", "Keeps row payloads practical for local sync."),
            limit("maxSpecBytes", "65536", "Keeps table specs reviewable and portable."),
        ],
        compatibility: vec![
            concat!(
                "This planned doc is exposed before runtime injection; generated apps must check ",
                "that the runtime actually grants the resource before calling it."
            )
            .to_string(),
            "The public docs describe relational behavior, not the reserved kv backing layout.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Reserved kv layout".to_string(),
                body: concat!(
                    "Reserved keys will use implementation-owned schema and row prefixes. ",
                    "These keys are hidden from app-facing docs unless includeInternal=true."
                )
                .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn relational_db_schemas() -> Vec<SchemaDoc> {
    vec![
        schema(
            "table_name.schema.json",
            "Table name",
            include_str!("schemas/relational_db/table_name.schema.json"),
        ),
        schema(
            "table_spec.schema.json",
            "TableSpec",
            include_str!("schemas/relational_db/table_spec.schema.json"),
        ),
        schema(
            "row.schema.json",
            "Row",
            include_str!("schemas/relational_db/row.schema.json"),
        ),
        schema(
            "patch.schema.json",
            "Patch",
            include_str!("schemas/relational_db/patch.schema.json"),
        ),
        schema(
            "query.schema.json",
            "Query",
            include_str!("schemas/relational_db/query.schema.json"),
        ),
        schema(
            "query_result.schema.json",
            "QueryResult",
            include_str!("schemas/relational_db/query_result.schema.json"),
        ),
    ]
}
