use terrane_cap_interface::{
    event_doc, limit, param, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, SchemaDoc,
};

pub fn builder_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "builder".to_string(),
        title: "Builder".to_string(),
        summary: concat!(
            "Owns replayable app-generation draft state and validates generated bundle files; ",
            "it does not run harness prompts itself."
        )
        .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "agent".to_string(),
            "host-implementer".to_string(),
            "app-author".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: vec![
                "builder.requested".to_string(),
                "builder.generated".to_string(),
                "builder.failed".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: Vec::new(),
        queries: Vec::new(),
        events: builder_events(),
        resources: Vec::new(),
        schemas: builder_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Draft lifecycle".to_string(),
                summary: concat!(
                    "Harness effects emit builder events; builder folds those events into ",
                    "draft state that can be replayed deterministically."
                )
                .to_string(),
                language: "events".to_string(),
                code: concat!(
                    "builder.requested { id, app_id, name, prompt, harness }\n",
                    "builder.generated { id, files }\n",
                    "builder.failed { id, error }"
                )
                .to_string(),
                expected: "BuilderState.drafts[id] contains prompt metadata, generated files, or the latest failure.".to_string(),
            },
            ExampleDoc {
                title: "Generated app bundle output".to_string(),
                summary: "The files payload must include a valid manifest.json and referenced backend/UI files.".to_string(),
                language: "json".to_string(),
                code: concat!(
                    "{\n",
                    "  \"files\": [\n",
                    "    {\"path\":\"manifest.json\",\"content\":\"{...}\"},\n",
                    "    {\"path\":\"main.js\",\"content\":\"function handle(input){ return 'ok'; }\"},\n",
                    "    {\"path\":\"index.html\",\"content\":\"<main></main>\"},\n",
                    "    {\"path\":\"styles.css\",\"content\":\"main { display: block; }\"}\n",
                    "  ]\n",
                    "}"
                )
                .to_string(),
                expected: "Files are normalized, validated, sorted by path, and recorded in builder.generated.".to_string(),
            },
        ],
        constraints: vec![
            "Builder has no public commands; harness and host edge code create builder events after external generation attempts.".to_string(),
            "Builder owns draft state and event folding for app-generation drafts.".to_string(),
            "Harness owns prompt construction and effect execution; builder owns generated bundle validation and durable draft events.".to_string(),
            "A requested event records draft id, app id, app name, original prompt, and selected harness.".to_string(),
            "A generated event replaces the draft files and clears the error; a failed event clears files and records the error.".to_string(),
            "Replay determinism comes from folding builder events only; external harnesses are not called during replay.".to_string(),
            "Generated manifest.json must match the requested app id/name, use runtime `js`, and reference existing backend and UI files.".to_string(),
            "Generated resource allowlists are constrained to currently supported app resources: kv, crdt, and document.".to_string(),
        ],
        limits: vec![
            limit(
                "maxGeneratedFiles",
                "48",
                "Keeps generated bundles reviewable and bounded.",
            ),
            limit(
                "maxGeneratedTotalBytes",
                "524288",
                "Bounds event-log payload and draft previews.",
            ),
            limit(
                "supportedExtensions",
                "html, htm, css, js, mjs, json, svg",
                "Keeps generated app bundles portable and inspectable.",
            ),
        ],
        compatibility: vec![
            "Draft ids and app ids are portable ASCII identifiers containing letters, digits, '-' or '_'.".to_string(),
            "Paths are normalized relative paths with '/' separators; absolute paths, parent directories, and backslashes are rejected.".to_string(),
            "The files schema is intentionally the same shape requested by the harness app-bundle prompt schema.".to_string(),
            "Builder validates JS app bundles today; WASM bundle generation can be added with a versioned schema rather than weakening current validation.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "State projection".to_string(),
                body: concat!(
                    "BuilderState stores drafts in a BTreeMap keyed by draft id. Generated ",
                    "files are deduplicated by normalized path and emitted in sorted order ",
                    "for stable replay and display."
                )
                .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn builder_events() -> Vec<terrane_cap_interface::EventDoc> {
    vec![
        event_doc(
            "builder.requested",
            &[
                param("id", "Draft id.", "draft_id"),
                param("app_id", "Generated app id.", "app_id"),
                param("name", "Generated app display name.", "string"),
                param("prompt", "Original app-generation prompt.", "string"),
                param(
                    "harness",
                    "Harness CLI selected by the requester.",
                    "string",
                ),
            ],
            "Creates or replaces a draft shell before external app generation runs.",
        ),
        event_doc(
            "builder.generated",
            &[
                param("id", "Draft id.", "draft_id"),
                param(
                    "files",
                    "Validated generated bundle files.",
                    "terrane.builder.filesOutput.v1#/properties/files",
                ),
            ],
            "Stores the validated file list and clears the draft error.",
        ),
        event_doc(
            "builder.failed",
            &[
                param("id", "Draft id.", "draft_id"),
                param(
                    "error",
                    "Generation or validation failure message.",
                    "string",
                ),
            ],
            "Clears generated files and records the latest draft failure.",
        ),
    ]
}

fn builder_schemas() -> Vec<SchemaDoc> {
    vec![
        schema(
            "terrane.builder.filesOutput.v1",
            "Builder files output",
            BUILDER_FILES_SCHEMA,
        ),
        schema(
            "terrane.builder.jsManifest.v1",
            "Generated JS manifest.json",
            BUILDER_JS_MANIFEST_SCHEMA,
        ),
    ]
}

const BUILDER_FILES_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["files"],
  "properties": {
    "files": {
      "type": "array",
      "minItems": 1,
      "maxItems": 48,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["path", "content"],
        "properties": {
          "path": { "type": "string" },
          "content": { "type": "string" }
        }
      }
    }
  }
}"#;

const BUILDER_JS_MANIFEST_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": true,
  "required": ["id", "name", "runtime", "backend", "ui"],
  "properties": {
    "id": { "type": "string" },
    "name": { "type": "string" },
    "runtime": { "const": "js" },
    "backend": { "type": "string" },
    "ui": { "type": "string" },
    "resources": {
      "type": "array",
      "items": { "enum": ["kv", "crdt", "document"] },
      "uniqueItems": true
    }
  }
}"#;
