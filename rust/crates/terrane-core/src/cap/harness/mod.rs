//! The `harness` capability — requests generated Terrane artifacts from
//! swappable external code-generation harnesses.

use std::collections::BTreeMap;

use crate::{Error, EventRecord, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use nanoserde::{DeJson, SerJson};

use super::{arg, builder, extract_json_object, truncate, Capability};
use crate::{decode_event, encode_event, Decision, Effect, State};

pub const DEFAULT_HARNESS: &str = "codex";
pub const APP_BUNDLE_OUTPUT_SCHEMA: &str = include_str!("prompts/app_bundle.schema.json");
pub const RUN_JS_OUTPUT_SCHEMA: &str = include_str!("prompts/run_js.schema.json");

const APP_BUNDLE_PROMPT: &str = include_str!("prompts/app_bundle.txt");
const RUN_JS_PROMPT: &str = include_str!("prompts/run_js.txt");

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessState {
    pub runs: BTreeMap<String, HarnessJsRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessJsRun {
    pub id: String,
    pub app_id: String,
    pub prompt: String,
    pub harness: String,
    pub js: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct JsRequested {
    id: String,
    app_id: String,
    prompt: String,
    harness: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct JsGenerated {
    id: String,
    js: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct JsCompleted {
    id: String,
    output: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct JsFailed {
    id: String,
    error: String,
}

#[derive(DeJson)]
struct RunJsPayload {
    js: String,
}

pub struct HarnessCapability;

impl Capability for HarnessCapability {
    fn namespace(&self) -> &'static str {
        "harness"
    }

    fn decide(&self, state: &State, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "harness.generate-app" => {
                let parsed = parse_harness_args(args, 4)?;
                let draft_id = builder::validate_id(&parsed.required[0], "draft id")?;
                let app_id = builder::validate_id(&parsed.required[1], "app id")?;
                let name = non_empty(parsed.required[2].clone(), "app name")?;
                let prompt = non_empty(parsed.tail, "prompt")?;
                Ok(Decision::Effect(Effect::GenerateAppWithHarness {
                    draft_id,
                    app_id,
                    name,
                    harness: parsed.harness,
                    prompt,
                }))
            }
            "harness.run-js" => {
                let parsed = parse_harness_args(args, 3)?;
                let run_id = builder::validate_id(&parsed.required[0], "run id")?;
                let app_id = builder::validate_id(&parsed.required[1], "app id")?;
                if !state.app.apps.contains_key(&app_id) {
                    return Err(Error::AppNotFound(app_id));
                }
                let prompt = non_empty(parsed.tail, "prompt")?;
                Ok(Decision::Effect(Effect::RunHarnessJs {
                    run_id,
                    app_id,
                    harness: parsed.harness,
                    prompt,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "harness.js.requested" | "codex.js.requested" => {
                let e: JsRequested = decode_event(record)?;
                state.harness.runs.insert(
                    e.id.clone(),
                    HarnessJsRun {
                        id: e.id,
                        app_id: e.app_id,
                        prompt: e.prompt,
                        harness: e.harness,
                        js: None,
                        output: None,
                        error: None,
                    },
                );
            }
            "harness.js.generated" | "codex.js.generated" => {
                let e: JsGenerated = decode_event(record)?;
                let run = state.harness.runs.entry(e.id.clone()).or_default();
                run.id = e.id;
                run.js = Some(e.js);
                run.error = None;
            }
            "harness.js.completed" | "codex.js.completed" => {
                let e: JsCompleted = decode_event(record)?;
                let run = state.harness.runs.entry(e.id.clone()).or_default();
                run.id = e.id;
                run.output = Some(e.output);
                run.error = None;
            }
            "harness.js.failed" | "codex.js.failed" => {
                let e: JsFailed = decode_event(record)?;
                let run = state.harness.runs.entry(e.id.clone()).or_default();
                run.id = e.id;
                run.output = None;
                run.error = Some(e.error);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "harness.js.requested" | "codex.js.requested" => {
                let e: JsRequested = decode_event(record).ok()?;
                Some(format!(
                    "harness.js.requested {} via {} for {}: {:?}",
                    e.id,
                    e.harness,
                    e.app_id,
                    truncate(&e.prompt, 48)
                ))
            }
            "harness.js.generated" | "codex.js.generated" => {
                let e: JsGenerated = decode_event(record).ok()?;
                Some(format!(
                    "harness.js.generated {} ({} chars)",
                    e.id,
                    e.js.len()
                ))
            }
            "harness.js.completed" | "codex.js.completed" => {
                let e: JsCompleted = decode_event(record).ok()?;
                Some(format!(
                    "harness.js.completed {}: {}",
                    e.id,
                    truncate(&e.output, 80)
                ))
            }
            "harness.js.failed" | "codex.js.failed" => {
                let e: JsFailed = decode_event(record).ok()?;
                Some(format!(
                    "harness.js.failed {}: {}",
                    e.id,
                    truncate(&e.error, 80)
                ))
            }
            _ => None,
        }
    }
}

pub fn app_bundle_prompt(app_id: &str, name: &str, user_prompt: &str) -> String {
    APP_BUNDLE_PROMPT
        .replace("{{USER_PROMPT}}", user_prompt)
        .replace("{{APP_ID_JSON}}", &app_id.serialize_json())
        .replace("{{APP_NAME_JSON}}", &name.serialize_json())
}

pub fn run_js_prompt(app_id: &str, user_prompt: &str) -> String {
    RUN_JS_PROMPT
        .replace("{{USER_PROMPT}}", user_prompt)
        .replace("{{APP_ID_JSON}}", &app_id.serialize_json())
}

pub fn parse_run_js_output(raw: &str) -> Result<String> {
    let json = extract_json_object(raw, "harness output")?;
    let payload = RunJsPayload::deserialize_json(json)
        .map_err(|e| Error::InvalidInput(format!("harness run-js output JSON: {e}")))?;
    non_empty(payload.js, "generated js")
}

struct ParsedHarnessArgs {
    harness: String,
    required: Vec<String>,
    tail: String,
}

fn parse_harness_args(args: &[String], required_count: usize) -> Result<ParsedHarnessArgs> {
    let mut harness = DEFAULT_HARNESS.to_string();
    let mut rest = args;
    if matches!(args.first().map(String::as_str), Some("--harness")) {
        harness = supported_harness(arg(args, 1, "harness")?)?;
        rest = args.get(2..).unwrap_or_default();
    }
    if rest.len() < required_count {
        return Err(Error::InvalidInput(format!(
            "missing {}",
            match required_count {
                4 => "draft id, app id, app name, or prompt",
                3 => "run id, app id, or prompt",
                _ => "required argument",
            }
        )));
    }
    let required = rest[..required_count - 1].to_vec();
    let tail = rest[required_count - 1..].join(" ");
    Ok(ParsedHarnessArgs {
        harness,
        required,
        tail,
    })
}

fn supported_harness(raw: String) -> Result<String> {
    let harness = raw.trim();
    match harness {
        "codex" | "claude" | "claude-code" | "opencode" => Ok(harness.to_string()),
        "" => Err(Error::InvalidInput("harness must not be empty".into())),
        other => Err(Error::InvalidInput(format!(
            "unsupported harness: {other}; expected codex, claude-code, claude, or opencode"
        ))),
    }
}

pub fn js_requested_event(
    id: &str,
    app_id: &str,
    prompt: &str,
    harness: &str,
) -> Result<EventRecord> {
    encode_event(
        "harness.js.requested",
        &JsRequested {
            id: id.to_string(),
            app_id: app_id.to_string(),
            prompt: prompt.to_string(),
            harness: harness.to_string(),
        },
    )
}

pub fn js_generated_event(id: &str, js: &str) -> Result<EventRecord> {
    encode_event(
        "harness.js.generated",
        &JsGenerated {
            id: id.to_string(),
            js: js.to_string(),
        },
    )
}

pub fn js_completed_event(id: &str, output: &str) -> Result<EventRecord> {
    encode_event(
        "harness.js.completed",
        &JsCompleted {
            id: id.to_string(),
            output: output.to_string(),
        },
    )
}

pub fn js_failed_event(id: &str, error: impl Into<String>) -> Result<EventRecord> {
    encode_event(
        "harness.js.failed",
        &JsFailed {
            id: id.to_string(),
            error: error.into(),
        },
    )
}

fn non_empty(raw: String, label: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        Err(Error::InvalidInput(format!("{label} must not be empty")))
    } else {
        Ok(value.to_string())
    }
}
