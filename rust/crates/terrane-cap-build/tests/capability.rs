use std::any::Any;

use terrane_cap_build::BuildCapability;
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Error, QueryValue, ReadValue, ResourceReadCtx, StateStore,
};

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
    ) -> terrane_cap_interface::Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

#[test]
fn build_capability_compiles_typescript_resource_reads() {
    let cap = BuildCapability;
    let store = EmptyStore;
    let bus = NoBus;
    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "demo",
        host: None,
    };

    let ReadValue::OptString(Some(json)) = cap
        .read_resource(
            ctx,
            "compileTs",
            &[
                "main.tsx".into(),
                "const n: number = 1; export const view = <button>{n}</button>;".into(),
            ],
        )
        .unwrap()
    else {
        panic!("compileTs should return a JSON string");
    };

    assert!(json.contains(r#""ok":true"#));
    assert!(json.contains("view"));
}

#[test]
fn build_capability_has_no_commands_and_reports_unknown_resources() {
    let cap = BuildCapability;
    let store = EmptyStore;
    let bus = NoBus;

    assert!(cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "build.compileTs",
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("unknown command"));
    assert!(cap
        .read_resource(
            ResourceReadCtx {
                state: &store,
                bus: &bus,
                app: "demo",
                host: None,
            },
            "missing",
            &[],
        )
        .unwrap_err()
        .to_string()
        .contains("unknown resource read"));
}
