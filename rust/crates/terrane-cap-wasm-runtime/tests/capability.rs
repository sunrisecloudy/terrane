use terrane_cap_interface::{
    Capability, Error, EventRecord, ReadValue, ResourceMethod, RuntimeCtx, RuntimeHost,
    RuntimeHostHandle, RuntimeRequest,
};
use terrane_cap_wasm_runtime::{run_wasm_bundle, WasmRuntimeBundle, WasmRuntimeCapability};

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

fn wat_bytes(source: &str) -> Vec<u8> {
    wat::parse_str(source).unwrap()
}

fn static_output_module(output: &str) -> Vec<u8> {
    let len = output.len();
    wat_bytes(&format!(
        r#"
        (module
          (memory (export "memory") 1)
          (global $heap (mut i32) (i32.const 1024))
          (data (i32.const 64) "{output}")
          (func $pack (param $ptr i32) (param $len i32) (result i64)
            local.get $ptr
            i64.extend_i32_u
            i64.const 32
            i64.shl
            local.get $len
            i64.extend_i32_u
            i64.or)
          (func (export "alloc") (param $len i32) (result i32)
            (local $ptr i32)
            global.get $heap
            local.set $ptr
            global.get $heap
            local.get $len
            i32.add
            global.set $heap
            local.get $ptr)
          (func (export "handle") (param $ptr i32) (param $len i32) (result i64)
            i32.const 64
            i32.const {len}
            call $pack))
        "#
    ))
}

#[test]
fn wasmtime_runtime_executes_real_wasm_module() {
    let bundle = WasmRuntimeBundle {
        module: static_output_module("pong"),
        name: "Demo".to_string(),
        entry: "handle".to_string(),
        resources: Vec::new(),
    };

    let output = run_wasm_bundle(&["ignored".to_string()], &bundle, no_resource_host()).unwrap();

    assert_eq!(output, "pong");
}

#[test]
fn wasm_runtime_loads_bundle_manifest_and_module_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.wasm"), static_output_module("loaded")).unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{
          "id": "wasm-demo",
          "name": "WASM Demo",
          "runtime": "wasm",
          "module": "main.wasm",
          "entry": "handle",
          "resources": []
        }"#,
    )
    .unwrap();

    let cap = WasmRuntimeCapability;
    let output = cap
        .run_runtime(
            RuntimeCtx {
                source: dir.path().to_str().unwrap().to_string(),
                source_files: None,
                app_name: "WASM Demo".to_string(),
                host: no_resource_host(),
            },
            RuntimeRequest {
                app: "wasm-demo".to_string(),
                input: Vec::new(),
            },
        )
        .unwrap();

    assert_eq!(output.output, "loaded");
}

#[test]
fn wasm_runtime_uses_fuel_to_stop_runaway_guest_code() {
    let module = wat_bytes(
        r#"
        (module
          (memory (export "memory") 1)
          (global $heap (mut i32) (i32.const 1024))
          (func (export "alloc") (param $len i32) (result i32)
            (local $ptr i32)
            global.get $heap
            local.set $ptr
            global.get $heap
            local.get $len
            i32.add
            global.set $heap
            local.get $ptr)
          (func (export "handle") (param $ptr i32) (param $len i32) (result i64)
            (loop $again
              br $again)
            i64.const 0))
        "#,
    );
    let bundle = WasmRuntimeBundle {
        module,
        name: "Loop".to_string(),
        entry: "handle".to_string(),
        resources: Vec::new(),
    };

    let err = run_wasm_bundle(&[], &bundle, no_resource_host()).unwrap_err();

    assert!(err.to_string().contains("error while executing"), "{err}");
}
