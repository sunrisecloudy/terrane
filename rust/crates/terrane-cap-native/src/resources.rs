use terrane_cap_interface::{
    arg, state_ref, Error, ReadValue, ResourceMethod, ResourceReadCtx, Result,
};

use crate::types::{NativeRequestStatus, NativeState};

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "clipboardWriteText",
            params: &["requestId", "text"],
        },
        ResourceMethod::Write {
            name: "externalOpenUrl",
            params: &["requestId", "url"],
        },
        ResourceMethod::Write {
            name: "notificationShow",
            params: &["requestId", "title", "body"],
        },
        ResourceMethod::Write {
            name: "dialogOpenFile",
            params: &["requestId", "optionsJson"],
        },
        ResourceMethod::Read {
            name: "result",
            params: &["requestId"],
        },
        ResourceMethod::Read {
            name: "pending",
            params: &[],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "result" => read_result(ctx, args),
        "pending" => read_pending(ctx),
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: native.{other}"
        ))),
    }
}

fn read_result(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let request_id = arg(args, 0, "request id")?;
    let state = state_ref::<NativeState>(ctx.state, "native")?;
    let value = state
        .requests
        .get(ctx.app)
        .and_then(|requests| requests.get(&request_id))
        .and_then(|record| match record.status {
            NativeRequestStatus::Completed => record.result_json.clone(),
            NativeRequestStatus::Failed | NativeRequestStatus::Cancelled => {
                record.error_json.clone()
            }
            NativeRequestStatus::Pending => None,
        });
    Ok(ReadValue::OptString(value))
}

fn read_pending(ctx: ResourceReadCtx<'_>) -> Result<ReadValue> {
    let state = state_ref::<NativeState>(ctx.state, "native")?;
    let pending = state
        .requests
        .get(ctx.app)
        .map(|requests| {
            requests
                .iter()
                .filter_map(|(id, record)| {
                    (record.status == NativeRequestStatus::Pending).then(|| id.clone())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ReadValue::StringList(pending))
}
