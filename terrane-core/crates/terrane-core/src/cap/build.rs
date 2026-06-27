//! The `build` capability — in-sandbox code build/check helpers.
//!
//! It exposes a pure TypeScript/JSX-to-JavaScript compiler to backends.
//! Generated agents can use it from QuickJS without gaining shell or filesystem
//! access.

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
        Ok(code) => format!(r#"{{"ok":true,"code":{}}}"#, json_string(&code)),
        Err(error) => format!(r#"{{"ok":false,"error":{}}}"#, json_string(&error)),
    };
    ReadValue::OptString(Some(json))
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch < ' ' => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
