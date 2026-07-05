//! The `document` capability — app-owned text documents with metadata.

use terrane_cap_interface::{
    arg, ensure_app_exists, state_ref, CapManifest, Capability, CommandCtx, CommandSpec, Decision,
    Error, EventPattern, EventRecord, EventSpec, GrantResourceSpec, ReadValue, ResourceReadCtx,
    Result, StateStore,
};

mod doc;
mod events;
mod resources;
mod types;

pub use types::{
    document_json, document_list_json, export_markdown, get_document_json, validate_document_id,
    Document, DocumentPatch, DocumentState, MAX_BODY_BYTES, MAX_DOCUMENTS_PER_APP,
    MAX_METADATA_BYTES, MAX_TITLE_CHARS,
};

pub struct DocumentCapability;

impl Capability for DocumentCapability {
    fn namespace(&self) -> &'static str {
        "document"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "document.create",
                },
                CommandSpec {
                    name: "document.patch",
                },
                CommandSpec {
                    name: "document.append",
                },
                CommandSpec {
                    name: "document.delete",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "document.created",
                },
                EventSpec {
                    kind: "document.patched",
                },
                EventSpec {
                    kind: "document.deleted",
                },
            ],
            queries: Vec::new(),
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "document",
                &["read", "write"],
                "App-owned document storage.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::document_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "document.create" => decide_create(ctx, args),
            "document.patch" => decide_patch(ctx, args),
            "document.append" => decide_append(ctx, args),
            "document.delete" => decide_delete(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        resources::read(ctx, name, args)
    }
}

fn decide_create(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let id = arg(args, 1, "id")?;
    let title = arg(args, 2, "title")?;
    let body = arg(args, 3, "body")?;
    let metadata_json = types::parse_metadata_json(args.get(4).map(String::as_str))?;
    ensure_app_exists(ctx.bus, &app)?;
    types::validate_document_id(&id)?;
    types::validate_title(&title)?;
    types::validate_body(&body)?;
    types::enforce_document_quota(state_ref::<DocumentState>(ctx.state, "document")?, &app, &id)?;
    Ok(Decision::Commit(vec![events::created_event(
        app,
        id,
        title,
        body,
        metadata_json,
    )?]))
}

fn decide_patch(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let id = arg(args, 1, "id")?;
    let patch_json = arg(args, 2, "patchJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    types::validate_document_id(&id)?;
    let patch = types::parse_patch_json(&patch_json)?;
    let document = document_for(ctx.state, &app, &id)?;
    if let Some(body) = &patch.body {
        types::validate_body(body)?;
    }
    if let Some(metadata_patch_json) = &patch.metadata_patch_json {
        let merged = types::apply_metadata_patch(&document.metadata_json, metadata_patch_json)?;
        types::validate_metadata_size(&merged)?;
    }
    if patch.is_empty() {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![events::patched_event(
        app,
        id,
        patch.title,
        patch.body,
        patch.metadata_patch_json,
        None,
    )?]))
}

fn decide_append(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let id = arg(args, 1, "id")?;
    let text = arg(args, 2, "text")?;
    ensure_app_exists(ctx.bus, &app)?;
    types::validate_document_id(&id)?;
    let document = document_for(ctx.state, &app, &id)?;
    let new_len = document.body.len().saturating_add(text.len());
    if new_len > MAX_BODY_BYTES {
        return Err(Error::InvalidInput(format!(
            "document body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    Ok(Decision::Commit(vec![events::patched_event(
        app,
        id,
        None,
        None,
        None,
        Some(text),
    )?]))
}

fn decide_delete(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let id = arg(args, 1, "id")?;
    ensure_app_exists(ctx.bus, &app)?;
    types::validate_document_id(&id)?;
    let exists = state_ref::<DocumentState>(ctx.state, "document")?
        .docs
        .get(&app)
        .is_some_and(|docs| docs.contains_key(&id));
    if !exists {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![events::deleted_event(app, id)?]))
}

fn document_for<'a>(
    state: &'a dyn StateStore,
    app: &str,
    id: &str,
) -> Result<&'a Document> {
    state_ref::<DocumentState>(state, "document")?
        .docs
        .get(app)
        .and_then(|docs| docs.get(id))
        .ok_or_else(|| Error::InvalidInput(format!("missing document: {app}/{id}")))
}
