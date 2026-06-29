use terrane_cap_interface::{
    CapabilityDoc, CapabilityManifestDoc, ExampleDoc, InternalNote, LimitDoc, ParamDoc,
    ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

use crate::{resource_methods, RDB_PREFIX};

pub fn relational_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "relational_db".to_string(),
        title: "Relational DB".to_string(),
        summary: "Deterministic app-scoped tables, rows, and indexes backed by reserved kv records."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "relational_db.defineTable".to_string(),
                "relational_db.put".to_string(),
                "relational_db.delete".to_string(),
            ],
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: methods.clone(),
        },
        resources: vec![ResourceDoc {
            namespace: "relational_db".to_string(),
            summary: "Backend resource surface for typed tables and indexed queries.".to_string(),
            methods,
        }],
        schemas: vec![
            schema(
                "terrane.relational_db.tableSpec.v1",
                "TableSpec v1",
                include_str!("table_spec.schema.json"),
            ),
            schema(
                "terrane.relational_db.query.v1",
                "Query v1",
                include_str!("query.schema.json"),
            ),
        ],
        examples: vec![ExampleDoc {
            title: "Users table with tenant primary key and secondary indexes".to_string(),
            summary: "Defines a table, inserts rows, reads by primary key, and queries a unique email index."
                .to_string(),
            language: "js".to_string(),
            code: include_str!("../examples/users.js").to_string(),
            expected: "JSON rows returned from ctx.resource.relational_db.query".to_string(),
        }],
        constraints: vec![
            "Raw SQL is never exposed; apps use resource methods and structured query JSON."
                .to_string(),
            "All tables, rows, and indexes are scoped to the current app id.".to_string(),
            "Table, field, index, and constraint names must be portable ASCII identifiers."
                .to_string(),
            "specJson must be a complete TableSpec v1 document; partial or flag-only schemas are rejected."
                .to_string(),
            "Rows and specs are stored as canonical JSON for deterministic replay.".to_string(),
            "Reads are derived from folded kv state and are not recorded as events.".to_string(),
            "In-place table spec changes are rejected after rows exist until a migration engine lands."
                .to_string(),
            "Queries must be served by the selected primary or secondary index and stay within table limits."
                .to_string(),
        ],
        limits: vec![
            limit("maxTableNameBytes", "64", "Portable key layout and generated API names."),
            limit("maxRowBytesDefault", "65536", "Default per-row payload ceiling."),
            limit("maxRowBytesAbsolute", "1048576", "Hard TableSpec schema ceiling."),
            limit("defaultQueryLimit", "100", "Default bounded read size."),
            limit("maxQueryLimit", "500", "Hard bounded read ceiling."),
            limit("specVersion", "1", "Initial stable TableSpec format."),
        ],
        compatibility: vec![
            "The capability emits ordinary kv.set and kv.deleted records, so existing KV replay, storage projection, and app removal behavior apply."
                .to_string(),
            "schemaVersion is application metadata; specVersion controls Terrane table-spec compatibility."
                .to_string(),
            "Reserved kv keys are hidden from public ctx.resource.kv reads, scans, and writes."
                .to_string(),
            "Secondary indexes are explicit objects in specJson, not ad hoc flags on fields."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Reserved kv layout".to_string(),
                body: format!(
                    "{}tables/<table> stores table summaries; {}table/<table>/spec stores canonical TableSpec JSON; {}row/<table>/<encoded-primary-key> stores rows; {}idx/<table>/<index>/<encoded-partition>/<encoded-sort-or-empty>/<encoded-primary-key> stores secondary index entries; {}uniq/<table>/<index-or-constraint>/<encoded-partition> stores unique guards.",
                    RDB_PREFIX, RDB_PREFIX, RDB_PREFIX, RDB_PREFIX, RDB_PREFIX
                ),
            }]
        } else {
            Vec::new()
        },
    }
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    resource_methods()
        .into_iter()
        .map(|method| match method.name() {
            "defineTable" => method_doc(
                "defineTable",
                method.kind(),
                vec![
                    param("table", "Validated table name.", "table_name"),
                    param(
                        "specJson",
                        "Complete TableSpec v1 JSON string.",
                        "terrane.relational_db.tableSpec.v1",
                    ),
                ],
                "Create or idempotently update an empty table from a complete specJson document.",
                "void",
            ),
            "put" => method_doc(
                "put",
                method.kind(),
                vec![
                    param("table", "Target table name.", "table_name"),
                    param("rowJson", "JSON object row to validate and store.", "row_json"),
                ],
                "Validate and upsert one canonical JSON row while maintaining primary, secondary, and unique indexes atomically.",
                "void",
            ),
            "delete" => method_doc(
                "delete",
                method.kind(),
                vec![
                    param("table", "Target table name.", "table_name"),
                    param("keyJson", "Primary key object or encoded key tuple JSON.", "primary_key_json"),
                ],
                "Delete one row by primary key and remove its secondary and unique index entries. Missing rows are a no-op.",
                "void",
            ),
            "get" => method_doc(
                "get",
                method.kind(),
                vec![
                    param("table", "Target table name.", "table_name"),
                    param("keyJson", "Primary key object or encoded key tuple JSON.", "primary_key_json"),
                ],
                "Read one canonical row JSON string by primary key, or null when absent.",
                "string|null",
            ),
            "query" => method_doc(
                "query",
                method.kind(),
                vec![
                    param("table", "Target table name.", "table_name"),
                    param("index", "Index name, primary, or $primary.", "index_name"),
                    param(
                        "queryJson",
                        "Structured query JSON string.",
                        "terrane.relational_db.query.v1",
                    ),
                ],
                "Run a bounded primary-key or secondary-index query.",
                "string",
            ),
            "tables" => method_doc(
                "tables",
                method.kind(),
                Vec::new(),
                "List table summaries visible to the current app.",
                "string",
            ),
            "spec" => method_doc(
                "spec",
                method.kind(),
                vec![param("table", "Target table name.", "table_name")],
                "Read a table's canonical specJson string, or null when absent.",
                "string|null",
            ),
            other => unreachable!("unexpected relational_db resource method: {other}"),
        })
        .collect()
}

fn method_doc(
    name: &str,
    kind: &str,
    params: Vec<ParamDoc>,
    summary: &str,
    returns: &str,
) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params,
        returns: returns.to_string(),
        summary: summary.to_string(),
        errors: vec![
            "invalid input".to_string(),
            "missing table".to_string(),
            "unique conflict".to_string(),
        ],
    }
}

fn param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: true,
        schema_ref: schema_ref.to_string(),
    }
}

fn schema(id: &str, title: &str, schema_json: &str) -> SchemaDoc {
    SchemaDoc {
        id: id.to_string(),
        title: title.to_string(),
        media_type: "application/schema+json".to_string(),
        schema_json: schema_json.to_string(),
        public: true,
    }
}

fn limit(name: &str, value: &str, reason: &str) -> LimitDoc {
    LimitDoc {
        name: name.to_string(),
        value: value.to_string(),
        reason: reason.to_string(),
    }
}
