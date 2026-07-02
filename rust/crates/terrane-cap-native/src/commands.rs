use terrane_cap_interface::{
    arg, ensure_app_exists, non_empty, replica_peer, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{
    cancelled_event, completed_event, failed_event, platform_observed_event, requested_event,
    RequestedEvent,
};
use crate::operations::{
    default_supported_operations, operation_for_command, result_size_for_operation, CANCEL,
    COMPLETE, DIALOG_OPEN_FILE, EXTERNAL_OPEN_URL, FAIL, NOTIFICATION_SHOW, PLATFORM_OBSERVE,
    RESOURCE_CLIPBOARD_WRITE_TEXT, RESOURCE_DIALOG_OPEN_FILE, RESOURCE_EXTERNAL_OPEN_URL,
    RESOURCE_NOTIFICATION_SHOW, RETENTION_KEEP_LAST,
};
use crate::types::{NativeRequestStatus, NativeState};

pub(crate) fn decide(ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
    match name {
        PLATFORM_OBSERVE => decide_platform_observe(args),
        COMPLETE => decide_complete(ctx, args),
        FAIL => decide_fail(ctx, args),
        CANCEL => decide_cancel(ctx, args),
        command => decide_request(ctx, command, args),
    }
}

fn decide_platform_observe(args: &[String]) -> Result<Decision> {
    let host_id = non_empty(arg(args, 0, "host id")?, "host id")?;
    let platform = non_empty(arg(args, 1, "platform")?, "platform")?;
    let connector_version = non_empty(arg(args, 2, "connector version")?, "connector version")?;
    let supported_operations = if args.len() > 3 {
        args[3..]
            .iter()
            .map(|op| non_empty(op.clone(), "supported operation"))
            .collect::<Result<Vec<_>>>()?
    } else {
        default_supported_operations()
    };
    Ok(Decision::Commit(vec![platform_observed_event(
        &host_id,
        &platform,
        &connector_version,
        supported_operations,
    )?]))
}

fn decide_request(ctx: CommandCtx<'_>, command: &str, args: &[String]) -> Result<Decision> {
    let Some(operation_id) = operation_for_command(command) else {
        return Err(Error::InvalidInput(format!("unknown command: {command}")));
    };
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let request_id = non_empty(arg(args, 1, "request id")?, "request id")?;
    let input_json = input_json(command, args)?;
    let state = state_ref::<NativeState>(ctx.state, "native")?;
    if state
        .requests
        .get(&app)
        .is_some_and(|requests| requests.contains_key(&request_id))
    {
        return Err(Error::InvalidInput(format!(
            "native request already exists: {app}/{request_id}"
        )));
    }
    let host_id = active_supported_host(state, operation_id)?;
    let sequence = state.next_sequence + 1;
    let origin_replica = replica_peer(ctx.bus)?;
    Ok(Decision::Commit(vec![requested_event(RequestedEvent {
        app: &app,
        request_id: &request_id,
        operation_id,
        executor_host_id: &host_id,
        origin_replica,
        sequence,
        input_json,
        result_size_class: result_size_for_operation(operation_id),
        retention_class: RETENTION_KEEP_LAST,
    })?]))
}

fn decide_complete(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let request_id = non_empty(arg(args, 1, "request id")?, "request id")?;
    ensure_pending(ctx, &app, &request_id)?;
    let result_json = valid_json(arg(args, 2, "result json")?, "result json")?;
    Ok(Decision::Commit(vec![completed_event(
        &app,
        &request_id,
        result_json,
    )?]))
}

fn decide_fail(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let request_id = non_empty(arg(args, 1, "request id")?, "request id")?;
    ensure_pending(ctx, &app, &request_id)?;
    let error_json = valid_json(arg(args, 2, "error json")?, "error json")?;
    Ok(Decision::Commit(vec![failed_event(
        &app,
        &request_id,
        error_json,
    )?]))
}

fn decide_cancel(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let request_id = non_empty(arg(args, 1, "request id")?, "request id")?;
    ensure_pending(ctx, &app, &request_id)?;
    let reason = non_empty(arg(args, 2, "reason")?, "reason")?;
    Ok(Decision::Commit(vec![cancelled_event(
        &app,
        &request_id,
        &reason,
    )?]))
}

fn active_supported_host(state: &NativeState, operation_id: &str) -> Result<String> {
    let host_id = state.active_host_id.as_ref().ok_or_else(|| {
        Error::InvalidInput("native platform has not been observed by a trusted host".into())
    })?;
    let platform = state.platforms.get(host_id).ok_or_else(|| {
        Error::InvalidInput(format!("native platform observation missing for {host_id}"))
    })?;
    if !platform.supported_operations.contains(operation_id) {
        return Err(Error::InvalidInput(format!(
            "native operation is not supported on this host: {operation_id}"
        )));
    }
    Ok(host_id.clone())
}

fn ensure_pending(ctx: CommandCtx<'_>, app: &str, request_id: &str) -> Result<()> {
    let state = state_ref::<NativeState>(ctx.state, "native")?;
    let record = state
        .requests
        .get(app)
        .and_then(|requests| requests.get(request_id))
        .ok_or_else(|| {
            Error::InvalidInput(format!("unknown native request: {app}/{request_id}"))
        })?;
    if record.status != NativeRequestStatus::Pending {
        return Err(Error::InvalidInput(format!(
            "native request is not pending: {app}/{request_id} ({})",
            record.status.as_str()
        )));
    }
    Ok(())
}

fn input_json(command: &str, args: &[String]) -> Result<String> {
    match command {
        crate::operations::CLIPBOARD_WRITE_TEXT | RESOURCE_CLIPBOARD_WRITE_TEXT => {
            let text = arg(args, 2, "text")?;
            Ok(serde_json::json!({ "text": text }).to_string())
        }
        EXTERNAL_OPEN_URL | RESOURCE_EXTERNAL_OPEN_URL => {
            let url = non_empty(arg(args, 2, "url")?, "url")?;
            Ok(serde_json::json!({ "url": url }).to_string())
        }
        NOTIFICATION_SHOW | RESOURCE_NOTIFICATION_SHOW => {
            let title = non_empty(arg(args, 2, "title")?, "title")?;
            let body = args.get(3).cloned().unwrap_or_default();
            Ok(serde_json::json!({ "title": title, "body": body }).to_string())
        }
        DIALOG_OPEN_FILE | RESOURCE_DIALOG_OPEN_FILE => {
            let options = valid_json(arg(args, 2, "options json")?, "options json")?;
            Ok(options)
        }
        other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }
}

fn valid_json(raw: String, label: &str) -> Result<String> {
    serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(|e| Error::InvalidInput(format!("{label} must be valid JSON: {e}")))?;
    Ok(raw)
}
