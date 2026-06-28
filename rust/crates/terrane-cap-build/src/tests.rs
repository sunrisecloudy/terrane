use std::any::Any;

use terrane_cap_interface::CapBus;

use super::*;

struct EmptyStore;

impl StateStore for EmptyStore {
    fn get(&self, _namespace: &str) -> Option<&dyn Any> {
        None
    }

    fn get_mut(&mut self, _namespace: &str) -> Option<&mut dyn Any> {
        None
    }
}

struct NoBus;

impl CapBus for NoBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> Result<terrane_cap_interface::QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

#[test]
fn compile_ts_returns_json_success_for_typescript() {
    let result = compile_ts(&[
        "main.ts".into(),
        "const answer: number = 42; export { answer };".into(),
    ]);
    let ReadValue::OptString(Some(json)) = result else {
        panic!("expected JSON string");
    };

    assert!(json.contains(r#""ok":true"#));
    assert!(json.contains("answer"));
}

#[test]
fn resource_read_exposes_compile_ts() {
    let store = EmptyStore;
    let bus = NoBus;
    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "demo",
    };

    assert!(BuildCapability
        .read_resource(ctx, "compileTs", &["main.ts".into(), "export {};".into()])
        .unwrap()
        .to_string_for_test()
        .contains(r#""ok":true"#));
    assert!(BuildCapability
        .read_resource(ctx, "missing", &[])
        .unwrap_err()
        .to_string()
        .contains("unknown resource read"));
}

trait ReadValueTestExt {
    fn to_string_for_test(&self) -> String;
}

impl ReadValueTestExt for ReadValue {
    fn to_string_for_test(&self) -> String {
        match self {
            ReadValue::OptString(Some(value)) => value.clone(),
            other => format!("{other:?}"),
        }
    }
}
