use terrane_cap_interface::{
    command_doc, limit, param, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, SchemaDoc,
};

pub fn js_runtime_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "js-runtime".to_string(),
        title: "JS Runtime".to_string(),
        summary: concat!(
            "Runs one app backend in QuickJS with only the resource namespaces declared ",
            "by that app bundle."
        )
        .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["js-runtime.run".to_string()],
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: vec![command_doc(
            "js-runtime.run",
            &[
                param("app", "Existing app id whose backend should run.", "app_id"),
                param(
                    "input",
                    "Zero or more string arguments passed as handle(input).",
                    "string[]",
                ),
            ],
            "string",
            "Load the selected app bundle and run its JS backend once in QuickJS.",
        )
        .with_errors(&[
            "app not found",
            "manifest runtime is not js",
            "backend source cannot be read",
            "handle(input) does not return a string",
            "runtime budget exceeded",
        ])
        .with_effects(&["RuntimeRequest(js-runtime)"])],
        queries: Vec::new(),
        events: Vec::new(),
        resources: Vec::new(),
        schemas: js_runtime_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Invoke an installed JS app".to_string(),
                summary: concat!(
                    "The capability command selects an existing app and supplies runtime input; ",
                    "it is not the app's manifest.json."
                )
                .to_string(),
                language: "sh".to_string(),
                code: "terrane invoke js-runtime.run notes \"create todo\"".to_string(),
                expected: "The host loads the app source, runs its backend once, and returns handle(input)'s string output.".to_string(),
            },
            ExampleDoc {
                title: "Bundle manifest and backend contract".to_string(),
                summary: concat!(
                    "manifest.json declares the backend file and resource allowlist; ",
                    "the backend defines handle(input) or an actions table."
                )
                .to_string(),
                language: "json+js".to_string(),
                code: concat!(
                    "{\n",
                    "  \"id\": \"notes\",\n",
                    "  \"name\": \"Notes\",\n",
                    "  \"runtime\": \"js\",\n",
                    "  \"backend\": \"main.js\",\n",
                    "  \"ui\": \"index.html\",\n",
                    "  \"resources\": [\"kv\"]\n",
                    "}\n\n",
                    "function handle(input) {\n",
                    "  ctx.resource.kv.set(\"last-input\", JSON.stringify(input));\n",
                    "  return \"ok\";\n",
                    "}"
                )
                .to_string(),
                expected: "Only ctx.resource.kv is installed; undeclared resources are absent from ctx.resource.".to_string(),
            },
        ],
        constraints: vec![
            "`js-runtime.run` is the capability command; bundle `manifest.json` is app metadata consumed by the runtime.".to_string(),
            "The backend contract is `handle(input)` returning a string; the injected app prelude may synthesize handle from an `actions` table.".to_string(),
            "Input is the remaining command arguments as a JavaScript array of strings.".to_string(),
            "Resource access is least-privilege: only namespaces named in manifest.resources are installed under `ctx.resource`.".to_string(),
            "Resource arguments must already be strings; QuickJS calls do not coerce numbers, objects, or booleans for resource parameters.".to_string(),
            "QuickJS gets app globals and declared resources only; `eval` and `Function` are disabled in the global scope.".to_string(),
            "Replay determinism comes from running JavaScript only during command execution; replay folds the recorded resource-write events and never re-runs the backend.".to_string(),
            "Direct `.js` developer sources default to the `kv` resource; bundle directories use their manifest allowlist exactly.".to_string(),
        ],
        limits: vec![
            limit(
                "defaultBackendBudgetMs",
                "5000",
                "QuickJS interrupt budget for one backend run; override with TERRANE_BACKEND_BUDGET_MS.",
            ),
            limit(
                "quickJsMemoryLimitBytes",
                "67108864",
                "Runtime memory limit for one QuickJS context.",
            ),
            limit(
                "quickJsMaxStackBytes",
                "524288",
                "Runtime stack limit for one QuickJS context.",
            ),
        ],
        compatibility: vec![
            "manifest.runtime may be empty for source-only developer use or `js` for bundle execution.".to_string(),
            "The app manifest fields `id`, `name`, and `ui` are catalog/UI metadata; they do not change the `js-runtime.run` command shape.".to_string(),
            "Generated JS apps should declare every resource namespace they call; missing declarations produce missing ctx.resource entries instead of ambient host access.".to_string(),
            "Runtime output is a UTF-8 string. Structured data should be JSON-encoded by the backend before returning.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "QuickJS prelude".to_string(),
                body: concat!(
                    "src/runtime/app_runtime.js is evaled after backend source. It preserves ",
                    "the public backend contract while allowing generated apps to expose ",
                    "actions instead of a hand-written handle(input)."
                )
                .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn js_runtime_schemas() -> Vec<SchemaDoc> {
    vec![schema(
        "terrane.js-runtime.manifest.v1",
        "JS runtime manifest.json",
        JS_RUNTIME_MANIFEST_SCHEMA,
    )]
}

const JS_RUNTIME_MANIFEST_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": true,
  "required": ["backend"],
  "properties": {
    "id": { "type": "string" },
    "name": { "type": "string" },
    "runtime": { "enum": ["", "js"] },
    "backend": { "type": "string" },
    "ui": { "type": "string" },
    "resources": {
      "type": "array",
      "items": { "type": "string" },
      "uniqueItems": true
    }
  }
}"#;
