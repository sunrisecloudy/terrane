use terrane_cap_interface::{
    Error, EventRecord, ReadValue, ResourceMethod, RuntimeHost, RuntimeHostHandle,
};
use terrane_cap_js_runtime::{read_manifest, run_js_bundle, JsRuntimeBundle};

struct NoResourceHost;

impl RuntimeHost for NoResourceHost {
    fn resource_methods(
        &self,
        _namespace: &str,
    ) -> terrane_cap_interface::Result<Vec<ResourceMethod>> {
        Ok(Vec::new())
    }

    fn read_resource(
        &mut self,
        namespace: &str,
        method: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<ReadValue> {
        Err(Error::Runtime(format!(
            "unexpected resource read: {namespace}.{method}"
        )))
    }

    fn write_resource(
        &mut self,
        namespace: &str,
        method: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<()> {
        Err(Error::Runtime(format!(
            "unexpected resource write: {namespace}.{method}"
        )))
    }

    fn take_records(&mut self) -> Vec<EventRecord> {
        Vec::new()
    }
}

fn no_resource_host() -> RuntimeHostHandle {
    RuntimeHostHandle::new(Box::new(NoResourceHost))
}

#[test]
fn manifest_reads_js_runtime_metadata() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{
          "id": "demo",
          "name": "Demo",
          "runtime": "js",
          "backend": "main.js",
          "ui": "index.html",
          "resources": ["kv"]
        }"#,
    )
    .unwrap();

    let manifest = read_manifest(dir.path()).unwrap();

    assert_eq!(manifest.id, "demo");
    assert_eq!(manifest.name, "Demo");
    assert_eq!(manifest.runtime, "js");
    assert_eq!(manifest.backend, "main.js");
    assert_eq!(manifest.ui, "index.html");
    assert_eq!(manifest.resources, vec!["kv".to_string()]);
}

#[test]
fn quickjs_runtime_executes_real_backend_source() {
    let bundle = JsRuntimeBundle {
        source: r#"
          function handle(input) {
            return input.join(":").toUpperCase();
          }
        "#
        .to_string(),
        name: "Demo".to_string(),
        resources: Vec::new(),
    };
    let input = vec!["alpha".to_string(), "beta".to_string()];

    let output = run_js_bundle("demo", &input, &bundle, no_resource_host()).unwrap();

    assert_eq!(output, "ALPHA:BETA");
}

#[test]
fn quickjs_runtime_reports_non_string_output() {
    let bundle = JsRuntimeBundle {
        source: "function handle(input) { return input.length; }".to_string(),
        name: "Demo".to_string(),
        resources: Vec::new(),
    };

    let err = run_js_bundle("demo", &[], &bundle, no_resource_host()).unwrap_err();

    assert!(
        err.to_string().contains("handle() must return a string"),
        "{err}"
    );
}
