use terrane_cap_interface::{
    command_doc, limit, param, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, SchemaDoc,
};

pub fn wasm_runtime_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "wasm-runtime".to_string(),
        title: "WASM Runtime".to_string(),
        summary: concat!(
            "Runs one app backend in Wasmtime with no WASI and with resource access limited ",
            "to the bundle manifest allowlist."
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
            commands: vec!["wasm-runtime.run".to_string()],
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: vec![command_doc(
            "wasm-runtime.run",
            &[
                param("app", "Existing app id whose module should run.", "app_id"),
                param(
                    "input",
                    "Zero or more string arguments encoded as a JSON array.",
                    "string[]",
                ),
            ],
            "string",
            "Load the selected app bundle and call its WASM entry export once in Wasmtime.",
        )
        .with_errors(&[
            "app not found",
            "manifest runtime is not wasm",
            "module cannot be read or instantiated",
            "required ABI export is missing",
            "fuel exhausted or runtime trap",
        ])
        .with_effects(&["RuntimeRequest(wasm-runtime)"])],
        queries: Vec::new(),
        events: Vec::new(),
        resources: Vec::new(),
        schemas: wasm_runtime_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Invoke an installed WASM app".to_string(),
                summary: concat!(
                    "The capability command supplies app id and input; it is separate from ",
                    "the app bundle manifest that names the module and entry export."
                )
                .to_string(),
                language: "sh".to_string(),
                code: "terrane invoke wasm-runtime.run calc \"[1,2,3]\"".to_string(),
                expected: "The host loads the app's module, calls the manifest entry export once, and returns UTF-8 output.".to_string(),
            },
            ExampleDoc {
                title: "Bundle manifest".to_string(),
                summary: "manifest.json declares the WASM module, entry export, UI, and allowed resource namespaces.".to_string(),
                language: "json".to_string(),
                code: concat!(
                    "{\n",
                    "  \"id\": \"calc\",\n",
                    "  \"name\": \"Calculator\",\n",
                    "  \"runtime\": \"wasm\",\n",
                    "  \"module\": \"backend.wasm\",\n",
                    "  \"entry\": \"handle\",\n",
                    "  \"ui\": \"index.html\",\n",
                    "  \"resources\": [\"kv\"]\n",
                    "}"
                )
                .to_string(),
                expected: "Only terrane.resource_read/write calls for `kv` are accepted.".to_string(),
            },
        ],
        constraints: vec![
            "`wasm-runtime.run` is the capability command; bundle `manifest.json` is runtime metadata consumed after the app is selected.".to_string(),
            "The default backend entry is `handle`; manifest.entry can name another exported `(ptr,len)->i64` function.".to_string(),
            "Input is encoded as a JSON array of strings, copied into guest memory, and passed to the entry export.".to_string(),
            "The module must export `memory` and `alloc(len)->ptr`; `dealloc(ptr,len)` is optional and called best-effort.".to_string(),
            "The entry export returns an i64 packed as high 32 bits pointer and low 32 bits length for a UTF-8 output buffer.".to_string(),
            "No WASI or ambient host imports are enabled; the only Terrane imports are `terrane.resource_write` and `terrane.resource_read`.".to_string(),
            "Resource access is least-privilege: imported resource calls are rejected unless the namespace appears in manifest.resources.".to_string(),
            "Replay determinism comes from running WASM only during command execution; replay folds recorded resource-write events and never re-runs the module.".to_string(),
        ],
        limits: vec![
            limit(
                "defaultWasmFuel",
                "10000000",
                "Fuel assigned to one run; override with positive TERRANE_WASM_FUEL.",
            ),
            limit(
                "ptrLenBits",
                "32+32",
                "ABI pointer/length pairs are packed into an i64.",
            ),
            limit(
                "runtimeImports",
                "terrane.resource_write, terrane.resource_read",
                "No WASI or filesystem/network imports are installed.",
            ),
        ],
        compatibility: vec![
            "manifest.runtime must be `wasm` for bundle directories; direct module paths default to entry `handle` and resource `kv` for developer use.".to_string(),
            "The app manifest fields `id`, `name`, and `ui` are catalog/UI metadata and do not change the `wasm-runtime.run` command shape.".to_string(),
            "terrane.resource_read writes a JSON-encoded ReadValue into a caller-provided output buffer and returns the byte length, or a negative required length if the buffer is too small.".to_string(),
            "Host resource errors are surfaced as runtime traps/errors so failed resource effects are not silently replayed.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Wasmtime configuration".to_string(),
                body: concat!(
                    "The engine enables consume_fuel(true), instantiates with a small host ",
                    "state containing the RuntimeHostHandle and manifest resources, and ",
                    "validates all guest memory pointer/length pairs before copying bytes."
                )
                .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn wasm_runtime_schemas() -> Vec<SchemaDoc> {
    vec![schema(
        "terrane.wasm-runtime.manifest.v1",
        "WASM runtime manifest.json",
        WASM_RUNTIME_MANIFEST_SCHEMA,
    )]
}

const WASM_RUNTIME_MANIFEST_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": true,
  "required": ["runtime", "module"],
  "properties": {
    "id": { "type": "string" },
    "name": { "type": "string" },
    "runtime": { "const": "wasm" },
    "module": { "type": "string" },
    "entry": { "type": "string", "default": "handle" },
    "ui": { "type": "string" },
    "resources": {
      "type": "array",
      "items": { "type": "string" },
      "uniqueItems": true
    }
  }
}"#;
