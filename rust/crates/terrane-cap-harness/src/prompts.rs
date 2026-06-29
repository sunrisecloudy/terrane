use nanoserde::{DeJson, SerJson};
use terrane_cap_interface::{extract_json_object, non_empty, Error, Result};

pub const APP_BUNDLE_OUTPUT_SCHEMA: &str = include_str!("prompts/app_bundle.schema.json");
pub const RUN_JS_OUTPUT_SCHEMA: &str = include_str!("prompts/run_js.schema.json");

const APP_BUNDLE_PROMPT: &str = include_str!("prompts/app_bundle.txt");
const RUN_JS_PROMPT: &str = include_str!("prompts/run_js.txt");

#[derive(DeJson)]
struct RunJsPayload {
    js: String,
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
