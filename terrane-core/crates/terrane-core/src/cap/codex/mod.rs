//! The `codex` capability — requests Codex-generated Terrane artifacts.

use terrane_domain::{Error, EventRecord, Result};

use super::{arg, builder, Capability};
use crate::{Decision, Effect, State};

pub const DEFAULT_HARNESS: &str = "codex";
pub const APP_BUNDLE_OUTPUT_SCHEMA: &str = include_str!("prompts/app_bundle.schema.json");

const APP_BUNDLE_PROMPT: &str = include_str!("prompts/app_bundle.txt");

pub struct CodexCapability;

impl Capability for CodexCapability {
    fn namespace(&self) -> &'static str {
        "codex"
    }

    fn decide(&self, _state: &State, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "codex.generate-app" => {
                let draft_id = builder::validate_id(&arg(args, 0, "draft id")?, "draft id")?;
                let app_id = builder::validate_id(&arg(args, 1, "app id")?, "app id")?;
                let name = non_empty(arg(args, 2, "app name")?, "app name")?;
                let prompt = non_empty(args.get(3..).unwrap_or_default().join(" "), "prompt")?;
                Ok(Decision::Effect(Effect::GenerateAppWithHarness {
                    draft_id,
                    app_id,
                    name,
                    harness: DEFAULT_HARNESS.to_string(),
                    prompt,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, _state: &mut State, _record: &EventRecord) -> Result<()> {
        Ok(())
    }
}

pub fn app_bundle_prompt(app_id: &str, name: &str, user_prompt: &str) -> String {
    APP_BUNDLE_PROMPT
        .replace("{{USER_PROMPT}}", user_prompt)
        .replace("{{APP_ID_JSON}}", &json_string(app_id))
        .replace("{{APP_NAME_JSON}}", &json_string(name))
}

fn non_empty(raw: String, label: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        Err(Error::InvalidInput(format!("{label} must not be empty")))
    } else {
        Ok(value.to_string())
    }
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
