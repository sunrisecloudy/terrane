//! The `build` capability — in-sandbox code build/check helpers.
//!
//! It exposes a pure TypeScript/JSX-to-JavaScript compiler to backends.
//! Generated agents can use it from QuickJS without gaining shell or filesystem
//! access.

use nanoserde::SerJson;
use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, Decision, Error, EventRecord, ReadValue, ResourceMethod,
    ResourceReadCtx, Result, StateStore,
};

mod doc;

pub struct BuildCapability;

impl Capability for BuildCapability {
    fn namespace(&self) -> &'static str {
        "build"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: Vec::new(),
            queries: Vec::new(),
            resources: resource_methods(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::build_doc(include_internal)
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(Error::InvalidInput(format!("unknown command: {name}")))
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn read_resource(
        &self,
        _ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "compileTs" => Ok(compile_ts(args)),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: build.{other}"
            ))),
        }
    }
}

fn resource_methods() -> Vec<ResourceMethod> {
    vec![ResourceMethod::Read {
        name: "compileTs",
        params: &["path", "source"],
    }]
}

/// `ctx.resource.build.compileTs(path, source)` — compile one JS/TS/JSX/TSX
/// module string and return a JSON string: `{"ok":true,"code":"..."}` or
/// `{"ok":false,"error":"..."}`.
fn compile_ts(args: &[String]) -> ReadValue {
    let path = args.first().map(String::as_str).unwrap_or("main.ts");
    let source = args.get(1).map(String::as_str).unwrap_or_default();
    let json = match terrane_app_build::compile_script_source(path, source) {
        Ok(code) => format!(r#"{{"ok":true,"code":{}}}"#, code.serialize_json()),
        Err(error) => format!(r#"{{"ok":false,"error":{}}}"#, error.serialize_json()),
    };
    ReadValue::OptString(Some(json))
}

#[cfg(test)]
mod tests;
