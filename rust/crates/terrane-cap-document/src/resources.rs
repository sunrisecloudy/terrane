use terrane_cap_interface::{
    arg, Error, ReadValue, ResourceMethod, ResourceReadCtx, Result,
};

use crate::types::{document_list_json, export_markdown, get_document_json};

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "create",
            params: &["id", "title", "body", "metadataJson"],
        },
        ResourceMethod::Write {
            name: "patch",
            params: &["id", "patchJson"],
        },
        ResourceMethod::Write {
            name: "append",
            params: &["id", "text"],
        },
        ResourceMethod::Write {
            name: "delete",
            params: &["id"],
        },
        ResourceMethod::Read {
            name: "get",
            params: &["id"],
        },
        ResourceMethod::Read {
            name: "list",
            params: &[],
        },
        ResourceMethod::Read {
            name: "exportMarkdown",
            params: &["id"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "get" => {
            let id = arg(args, 0, "id")?;
            Ok(ReadValue::OptString(get_document_json(ctx.state, ctx.app, &id)?))
        }
        "list" => Ok(ReadValue::OptString(Some(document_list_json(
            ctx.state, ctx.app,
        )?))),
        "exportMarkdown" => {
            let id = arg(args, 0, "id")?;
            Ok(ReadValue::OptString(Some(export_markdown(
                ctx.state, ctx.app, &id,
            )?)))
        }
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: document.{other}"
        ))),
    }
}
