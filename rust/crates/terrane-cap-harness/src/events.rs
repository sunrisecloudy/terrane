use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_event, encode_event, state_mut, truncate, EventRecord, Result, StateStore,
};

use crate::{HarnessJsRun, HarnessState};

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

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "harness.js.requested" | "codex.js.requested" => {
            let e: JsRequested = decode_event(record)?;
            state_mut::<HarnessState>(state, "harness")?.runs.insert(
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
            let run = state_mut::<HarnessState>(state, "harness")?
                .runs
                .entry(e.id.clone())
                .or_default();
            run.id = e.id;
            run.js = Some(e.js);
            run.error = None;
        }
        "harness.js.completed" | "codex.js.completed" => {
            let e: JsCompleted = decode_event(record)?;
            let run = state_mut::<HarnessState>(state, "harness")?
                .runs
                .entry(e.id.clone())
                .or_default();
            run.id = e.id;
            run.output = Some(e.output);
            run.error = None;
        }
        "harness.js.failed" | "codex.js.failed" => {
            let e: JsFailed = decode_event(record)?;
            let run = state_mut::<HarnessState>(state, "harness")?
                .runs
                .entry(e.id.clone())
                .or_default();
            run.id = e.id;
            run.output = None;
            run.error = Some(e.error);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
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
