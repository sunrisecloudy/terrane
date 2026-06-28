use crate::{EventRecord, Result};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{cap::truncate, decode_event, encode_event, State};

use super::{BuilderDraft, BuilderFile};

#[derive(BorshSerialize, BorshDeserialize)]
struct Requested {
    id: String,
    app_id: String,
    name: String,
    prompt: String,
    harness: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Generated {
    id: String,
    files: Vec<BuilderFile>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Failed {
    id: String,
    error: String,
}

pub fn requested_event(
    id: &str,
    app_id: &str,
    name: &str,
    prompt: &str,
    harness: &str,
) -> Result<EventRecord> {
    encode_event(
        "builder.requested",
        &Requested {
            id: id.to_string(),
            app_id: app_id.to_string(),
            name: name.to_string(),
            prompt: prompt.to_string(),
            harness: harness.to_string(),
        },
    )
}

pub fn generated_event(id: &str, files: Vec<BuilderFile>) -> Result<EventRecord> {
    encode_event(
        "builder.generated",
        &Generated {
            id: id.to_string(),
            files,
        },
    )
}

pub fn failed_event(id: &str, error: impl Into<String>) -> Result<EventRecord> {
    encode_event(
        "builder.failed",
        &Failed {
            id: id.to_string(),
            error: error.into(),
        },
    )
}

pub fn fold(state: &mut State, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "builder.requested" => {
            let e: Requested = decode_event(record)?;
            state.builder.drafts.insert(
                e.id.clone(),
                BuilderDraft {
                    id: e.id,
                    app_id: e.app_id,
                    name: e.name,
                    prompt: e.prompt,
                    harness: e.harness,
                    files: Vec::new(),
                    error: None,
                },
            );
        }
        "builder.generated" => {
            let e: Generated = decode_event(record)?;
            let draft = state.builder.drafts.entry(e.id.clone()).or_default();
            draft.id = e.id;
            draft.files = e.files;
            draft.error = None;
        }
        "builder.failed" => {
            let e: Failed = decode_event(record)?;
            let draft = state.builder.drafts.entry(e.id.clone()).or_default();
            draft.id = e.id;
            draft.files.clear();
            draft.error = Some(e.error);
        }
        _ => {}
    }
    Ok(())
}

pub fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "builder.requested" => {
            let e: Requested = decode_event(record).ok()?;
            Some(format!(
                "builder.requested {} via {}: {:?}",
                e.app_id,
                e.harness,
                truncate(&e.prompt, 48)
            ))
        }
        "builder.generated" => {
            let e: Generated = decode_event(record).ok()?;
            Some(format!(
                "builder.generated {} ({} files)",
                e.id,
                e.files.len()
            ))
        }
        "builder.failed" => {
            let e: Failed = decode_event(record).ok()?;
            Some(format!(
                "builder.failed {}: {}",
                e.id,
                truncate(&e.error, 80)
            ))
        }
        _ => None,
    }
}
