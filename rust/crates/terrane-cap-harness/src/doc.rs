use terrane_cap_interface::{
    command_doc, event_doc, param, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, SchemaDoc,
};

pub fn harness_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "harness".to_string(),
        title: "Harness".to_string(),
        summary: concat!(
            "Turns app-generation and one-shot JS-generation requests into edge effects ",
            "that are recorded as builder, harness, and resource events."
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
            commands: vec![
                "harness.generate-app".to_string(),
                "harness.run-js".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "harness.js.requested".to_string(),
                "harness.js.generated".to_string(),
                "harness.js.completed".to_string(),
                "harness.js.failed".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: harness_commands(),
        queries: Vec::new(),
        events: harness_events(),
        resources: Vec::new(),
        schemas: harness_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Generate an app bundle draft".to_string(),
                summary: concat!(
                    "Creates a GenerateAppWithHarness effect. The edge runner asks the selected ",
                    "harness for files that satisfy the app-bundle schema, then records builder events."
                )
                .to_string(),
                language: "sh".to_string(),
                code: "terrane invoke harness.generate-app --harness codex draft_1 notes Notes \"build a tiny notes app\"".to_string(),
                expected: "builder.requested followed by builder.generated or builder.failed.".to_string(),
            },
            ExampleDoc {
                title: "Generate and run one JS program".to_string(),
                summary: concat!(
                    "Creates a RunHarnessJs effect. The edge runner validates JSON against the ",
                    "run-js schema, runs the generated JS once in QuickJS, and records the lifecycle."
                )
                .to_string(),
                language: "sh".to_string(),
                code: "terrane invoke harness.run-js run_1 notes \"write a migration script\"".to_string(),
                expected: "harness.js.requested, harness.js.generated, resource writes, then harness.js.completed; or harness.js.failed.".to_string(),
            },
        ],
        constraints: vec![
            "Harness commands do not directly mutate durable state; they return edge effects that must be executed once by the host.".to_string(),
            "The supported harness selectors are `codex`, `claude`, `claude-code`, and `opencode`; omitted selector defaults to `codex`.".to_string(),
            "`harness.generate-app` validates ids and prompt text, then delegates draft/event ownership to builder events.".to_string(),
            "`harness.run-js` requires the target app to exist before the effect is created.".to_string(),
            "Prompt outputs are constrained by the checked JSON schemas embedded from src/prompts/*.schema.json.".to_string(),
            "Replay folds recorded builder, harness, and resource events; replay never re-runs an external harness CLI or generated JS.".to_string(),
            "Generated run-js code executes with the JS runtime and the host-provided resources for that effect, not with ambient filesystem or network access.".to_string(),
        ],
        limits: vec![
            terrane_cap_interface::limit(
                "defaultHarness",
                "codex",
                "Stable default when --harness is omitted.",
            ),
            terrane_cap_interface::limit(
                "runJsOutputSchema",
                "one non-empty js string",
                "Keeps one-shot code generation parseable and reviewable.",
            ),
            terrane_cap_interface::limit(
                "appBundleOutputSchema",
                "files array with at least 4 files",
                "Keeps generated app drafts complete enough for builder validation.",
            ),
        ],
        compatibility: vec![
            "Older `codex.js.*` event names are still folded for compatibility, but new events are emitted under `harness.js.*`.".to_string(),
            "The harness app-bundle schema matches builder::parse_generated_files input: an object with a `files` array of `{ path, content }` objects.".to_string(),
            "Generated run-js resource writes are interleaved before harness.js.completed so replay sees the same durable effects without re-executing generated code.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Prompt assets".to_string(),
                body: concat!(
                    "src/prompts/app_bundle.txt and src/prompts/run_js.txt are filled with ",
                    "JSON-escaped ids/names; their output schemas are included in this doc ",
                    "from the same files the edge runner passes to harness CLIs."
                )
                .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn harness_commands() -> Vec<terrane_cap_interface::CommandDoc> {
    vec![
        command_doc(
            "harness.generate-app",
            &[
                param(
                    "draft_id",
                    "Builder draft id to own the result.",
                    "draft_id",
                ),
                param("app_id", "Target app id for generated files.", "app_id"),
                param("name", "Display name expected in manifest.json.", "string"),
                param(
                    "prompt",
                    "User request passed through the harness prompt template.",
                    "string",
                ),
            ],
            "effect",
            "Ask an external harness to generate a JS app bundle draft.",
        )
        .with_errors(&[
            "invalid id",
            "empty app name",
            "empty prompt",
            "unsupported harness",
        ])
        .with_effects(&["GenerateAppWithHarness"]),
        command_doc(
            "harness.run-js",
            &[
                param("run_id", "Harness JS run id.", "run_id"),
                param(
                    "app_id",
                    "Existing app id whose state/resources are visible.",
                    "app_id",
                ),
                param(
                    "prompt",
                    "User request passed through the run-js prompt template.",
                    "string",
                ),
            ],
            "effect",
            "Ask an external harness to generate one JS program and execute it once.",
        )
        .with_errors(&[
            "invalid id",
            "app not found",
            "empty prompt",
            "unsupported harness",
        ])
        .with_effects(&["RunHarnessJs"]),
    ]
}

fn harness_events() -> Vec<terrane_cap_interface::EventDoc> {
    vec![
        event_doc(
            "harness.js.requested",
            &[
                param("id", "Run id.", "run_id"),
                param("app_id", "Target app id.", "app_id"),
                param("prompt", "Original user prompt.", "string"),
                param("harness", "Selected harness CLI.", "string"),
            ],
            "Recorded before the host asks a harness to generate one JS program.",
        ),
        event_doc(
            "harness.js.generated",
            &[
                param("id", "Run id.", "run_id"),
                param("js", "Generated JavaScript source.", "string"),
            ],
            "Records the generated JavaScript that was executed once by the edge runner.",
        ),
        event_doc(
            "harness.js.completed",
            &[
                param("id", "Run id.", "run_id"),
                param(
                    "output",
                    "String returned by generated handle(input).",
                    "string",
                ),
            ],
            "Records successful one-shot JS execution after any resource writes.",
        ),
        event_doc(
            "harness.js.failed",
            &[
                param("id", "Run id.", "run_id"),
                param(
                    "error",
                    "Failure message from harness generation or JS runtime.",
                    "string",
                ),
            ],
            "Records failure instead of generated/completed state.",
        ),
    ]
}

fn harness_schemas() -> Vec<SchemaDoc> {
    vec![
        schema(
            "terrane.harness.appBundleOutput.v1",
            "Harness app bundle output",
            include_str!("prompts/app_bundle.schema.json"),
        ),
        schema(
            "terrane.harness.runJsOutput.v1",
            "Harness run-js output",
            include_str!("prompts/run_js.schema.json"),
        ),
    ]
}
