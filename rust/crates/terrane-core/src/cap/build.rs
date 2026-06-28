//! The `build` capability — in-sandbox code build/check helpers.
//!
//! It exposes a pure TypeScript/JSX-to-JavaScript compiler to backends.
//! Generated agents can use it from QuickJS without gaining shell or filesystem
//! access.

use nanoserde::SerJson;
use terrane_domain::{EventRecord, Result};

use super::{Capability, ReadValue, ResourceMethod};
use crate::{Decision, State};

pub struct BuildCapability;

impl Capability for BuildCapability {
    fn namespace(&self) -> &'static str {
        "build"
    }

    fn decide(&self, _state: &State, name: &str, _args: &[String]) -> Result<Decision> {
        Err(terrane_domain::Error::InvalidInput(format!(
            "unknown command: {name}"
        )))
    }

    fn fold(&self, _state: &mut State, _record: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn resource_api(&self) -> Vec<ResourceMethod> {
        vec![ResourceMethod::Read {
            name: "compileTs",
            params: &["path", "source"],
            read: compile_ts,
        }]
    }
}

/// `ctx.resource.build.compileTs(path, source)` — compile one JS/TS/JSX/TSX
/// module string and return a JSON string: `{"ok":true,"code":"..."}` or
/// `{"ok":false,"error":"..."}`.
fn compile_ts(_state: &State, _app: &str, args: &[String]) -> ReadValue {
    let path = args.first().map(String::as_str).unwrap_or("main.ts");
    let source = args.get(1).map(String::as_str).unwrap_or_default();
    let json = match terrane_app_build::compile_script_source(path, source) {
        Ok(code) => format!(r#"{{"ok":true,"code":{}}}"#, code.serialize_json()),
        Err(error) => format!(r#"{{"ok":false,"error":{}}}"#, error.serialize_json()),
    };
    ReadValue::OptString(Some(json))
}
