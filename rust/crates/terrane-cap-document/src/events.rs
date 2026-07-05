use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, truncate, EventRecord, Result,
    StateStore,
};

use crate::types::{
    apply_metadata_patch, validate_body, validate_metadata_size, Document, DocumentState,
};

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Created {
    pub app: String,
    pub id: String,
    pub title: String,
    pub body: String,
    pub metadata_json: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Patched {
    pub app: String,
    pub id: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub metadata_patch_json: Option<String>,
    pub append: Option<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Deleted {
    pub app: String,
    pub id: String,
}

pub fn created_event(
    app: impl Into<String>,
    id: impl Into<String>,
    title: impl Into<String>,
    body: impl Into<String>,
    metadata_json: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "document.created",
        &Created {
            app: app.into(),
            id: id.into(),
            title: title.into(),
            body: body.into(),
            metadata_json: metadata_json.into(),
        },
    )
}

pub fn patched_event(
    app: impl Into<String>,
    id: impl Into<String>,
    title: Option<String>,
    body: Option<String>,
    metadata_patch_json: Option<String>,
    append: Option<String>,
) -> Result<EventRecord> {
    encode_event(
        "document.patched",
        &Patched {
            app: app.into(),
            id: id.into(),
            title,
            body,
            metadata_patch_json,
            append,
        },
    )
}

pub fn deleted_event(app: impl Into<String>, id: impl Into<String>) -> Result<EventRecord> {
    encode_event(
        "document.deleted",
        &Deleted {
            app: app.into(),
            id: id.into(),
        },
    )
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "document.created" => {
            let e: Created = decode_event(record)?;
            let state = state_mut::<DocumentState>(state, "document")?;
            state.docs.entry(e.app).or_default().insert(
                e.id.clone(),
                Document {
                    id: e.id,
                    title: e.title,
                    body: e.body,
                    metadata_json: e.metadata_json,
                    updated_seq: None,
                },
            );
        }
        "document.patched" => {
            let e: Patched = decode_event(record)?;
            let state = state_mut::<DocumentState>(state, "document")?;
            let Some(document) = state
                .docs
                .get_mut(&e.app)
                .and_then(|docs| docs.get_mut(&e.id))
            else {
                return Ok(());
            };
            if let Some(title) = e.title {
                document.title = title;
            }
            if let Some(body) = e.body {
                document.body = body;
            }
            if let Some(metadata_patch_json) = e.metadata_patch_json {
                document.metadata_json =
                    apply_metadata_patch(&document.metadata_json, &metadata_patch_json)?;
            }
            if let Some(append) = e.append {
                document.body.push_str(&append);
                validate_body(&document.body)?;
            }
            validate_metadata_size(&document.metadata_json)?;
        }
        "document.deleted" => {
            let e: Deleted = decode_event(record)?;
            let state = state_mut::<DocumentState>(state, "document")?;
            if let Some(docs) = state.docs.get_mut(&e.app) {
                docs.remove(&e.id);
                if docs.is_empty() {
                    state.docs.remove(&e.app);
                }
            }
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            state_mut::<DocumentState>(state, "document")?
                .docs
                .remove(&e.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "document.created" => {
            let e: Created = decode_event(record).ok()?;
            Some(format!(
                "document.created {}/{} {} bytes",
                e.app,
                e.id,
                e.body.len()
            ))
        }
        "document.patched" => {
            let e: Patched = decode_event(record).ok()?;
            let append = e.append.as_deref().map(truncate_append).unwrap_or_default();
            Some(format!("document.patched {}/{}{}", e.app, e.id, append))
        }
        "document.deleted" => {
            let e: Deleted = decode_event(record).ok()?;
            Some(format!("document.deleted {}/{}", e.app, e.id))
        }
        _ => None,
    }
}

fn truncate_append(value: &str) -> String {
    format!(" append={}", truncate(value, 40))
}
