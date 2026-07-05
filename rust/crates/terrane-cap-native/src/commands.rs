use terrane_cap_interface::{
    arg, ensure_app_exists, non_empty, replica_peer, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{
    cancelled_event, completed_event, failed_event, platform_observed_event, requested_event,
    RequestedEvent,
};
use crate::operations::{
    default_supported_operations, operation_for_command, result_size_for_operation, CANCEL,
    COMPLETE, DIALOG_OPEN_FILE, DIALOG_SAVE_FILE, EXTERNAL_OPEN_URL, FAIL, MAX_TRAY_ITEMS,
    MAX_TRAY_LABEL_CHARS, NOTIFICATION_SHOW, OP_CLIPBOARD_READ_TEXT, OP_DIALOG_SAVE_FILE,
    OP_SCREEN_CAPTURE, OP_SHORTCUT_REGISTER_GLOBAL, OP_TRAY_SET_MENU, OP_WINDOW_CONTROL,
    PLATFORM_OBSERVE, RESOURCE_CLIPBOARD_READ_TEXT, RESOURCE_CLIPBOARD_WRITE_TEXT,
    RESOURCE_DIALOG_OPEN_FILE, RESOURCE_DIALOG_SAVE_FILE, RESOURCE_EXTERNAL_OPEN_URL,
    RESOURCE_NOTIFICATION_SHOW, RESOURCE_SCREEN_CAPTURE, RESOURCE_SHORTCUT_REGISTER_GLOBAL,
    RESOURCE_TRAY_SET_MENU, RESOURCE_WINDOW_CONTROL, RETENTION_KEEP_LAST, SCREEN_CAPTURE,
    SHORTCUT_REGISTER_GLOBAL, TRAY_SET_MENU, WINDOW_CONTROL,
};
use crate::types::{NativeRequestRecord, NativeRequestStatus, NativeState};

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
    validate_state_limits(state, &app, operation_id, &input_json)?;
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

fn validate_state_limits(
    state: &NativeState,
    app: &str,
    operation_id: &str,
    input_json: &str,
) -> Result<()> {
    if operation_id != OP_SHORTCUT_REGISTER_GLOBAL {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(input_json)
        .map_err(|e| Error::InvalidInput(format!("shortcut input must be valid JSON: {e}")))?;
    let accelerator = value
        .get("accelerator")
        .and_then(|value| value.as_str())
        .ok_or_else(|| Error::InvalidInput("shortcut accelerator is required".into()))?;
    let existing = state.shortcuts.get(app);
    let replaces_existing = existing.is_some_and(|shortcuts| shortcuts.contains_key(accelerator));
    if !replaces_existing
        && existing.map(|shortcuts| shortcuts.len()).unwrap_or_default()
            >= crate::operations::MAX_SHORTCUTS_PER_APP
    {
        return Err(Error::InvalidInput(format!(
            "shortcut.registerGlobal supports at most {} shortcuts per app",
            crate::operations::MAX_SHORTCUTS_PER_APP
        )));
    }
    Ok(())
}

fn decide_complete(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let request_id = non_empty(arg(args, 1, "request id")?, "request id")?;
    let record = pending_record(ctx, &app, &request_id)?;
    let result_json = valid_json(arg(args, 2, "result json")?, "result json")?;
    validate_completion_result(&record.operation_id, &result_json)?;
    Ok(Decision::Commit(vec![completed_event(
        &app,
        &request_id,
        result_json,
    )?]))
}

fn decide_fail(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = non_empty(arg(args, 0, "app")?, "app")?;
    let request_id = non_empty(arg(args, 1, "request id")?, "request id")?;
    pending_record(ctx, &app, &request_id)?;
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
    pending_record(ctx, &app, &request_id)?;
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

fn pending_record(
    ctx: CommandCtx<'_>,
    app: &str,
    request_id: &str,
) -> Result<NativeRequestRecord> {
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
    Ok(record.clone())
}

fn input_json(command: &str, args: &[String]) -> Result<String> {
    match command {
        crate::operations::CLIPBOARD_WRITE_TEXT | RESOURCE_CLIPBOARD_WRITE_TEXT => {
            let text = arg(args, 2, "text")?;
            Ok(serde_json::json!({ "text": text }).to_string())
        }
        crate::operations::CLIPBOARD_READ_TEXT | RESOURCE_CLIPBOARD_READ_TEXT => {
            expect_no_extra(args, 2, "clipboard.readText")?;
            Ok(serde_json::json!({}).to_string())
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
        DIALOG_SAVE_FILE | RESOURCE_DIALOG_SAVE_FILE => {
            let suggested_name = non_empty(arg(args, 2, "suggested name")?, "suggested name")?;
            let blob_name = non_empty(arg(args, 3, "blob name")?, "blob name")?;
            Ok(serde_json::json!({
                "suggestedName": suggested_name,
                "blobName": blob_name,
            })
            .to_string())
        }
        SCREEN_CAPTURE | RESOURCE_SCREEN_CAPTURE => {
            let target = args.get(2).cloned().unwrap_or_else(|| "screen".to_string());
            match target.as_str() {
                "screen" | "window" => {}
                _ => {
                    return Err(Error::InvalidInput(
                        "screen.capture target must be screen or window".into(),
                    ))
                }
            }
            Ok(serde_json::json!({ "target": target }).to_string())
        }
        TRAY_SET_MENU | RESOURCE_TRAY_SET_MENU => {
            let title = non_empty(arg(args, 2, "title")?, "title")?;
            let items_json = valid_json(arg(args, 3, "items json")?, "items json")?;
            let items = validate_tray_items(&items_json)?;
            Ok(serde_json::json!({
                "title": title,
                "items": items,
            })
            .to_string())
        }
        SHORTCUT_REGISTER_GLOBAL | RESOURCE_SHORTCUT_REGISTER_GLOBAL => {
            let accelerator = non_empty(arg(args, 2, "accelerator")?, "accelerator")?;
            let verb = non_empty(arg(args, 3, "verb")?, "verb")?;
            Ok(serde_json::json!({
                "accelerator": accelerator,
                "verb": verb,
            })
            .to_string())
        }
        WINDOW_CONTROL | RESOURCE_WINDOW_CONTROL => {
            let action = non_empty(arg(args, 2, "action")?, "action")?;
            let title = args.get(3).cloned();
            match action.as_str() {
                "focus" | "minimize" => {
                    if title.is_some() {
                        return Err(Error::InvalidInput(format!(
                            "window.control {action} does not take a title"
                        )));
                    }
                }
                "setTitle" => {
                    let Some(value) = title.as_ref() else {
                        return Err(Error::InvalidInput(
                            "window.control setTitle requires title".into(),
                        ));
                    };
                    non_empty(value.clone(), "title")?;
                }
                _ => {
                    return Err(Error::InvalidInput(
                        "window.control action must be focus, minimize, or setTitle".into(),
                    ))
                }
            }
            Ok(serde_json::json!({
                "action": action,
                "title": title,
            })
            .to_string())
        }
        other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
    }
}

fn valid_json(raw: String, label: &str) -> Result<String> {
    serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(|e| Error::InvalidInput(format!("{label} must be valid JSON: {e}")))?;
    Ok(raw)
}

fn expect_no_extra(args: &[String], max_len: usize, label: &str) -> Result<()> {
    if args.len() > max_len {
        return Err(Error::InvalidInput(format!("{label} takes no payload arguments")));
    }
    Ok(())
}

fn validate_tray_items(raw: &str) -> Result<Vec<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("items json must be valid JSON: {e}")))?;
    let items = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("tray.setMenu items must be a JSON array".into()))?;
    if items.len() > MAX_TRAY_ITEMS {
        return Err(Error::InvalidInput(format!(
            "tray.setMenu supports at most {MAX_TRAY_ITEMS} items"
        )));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let object = item.as_object().ok_or_else(|| {
            Error::InvalidInput("tray.setMenu items must be objects".into())
        })?;
        let id = object
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| Error::InvalidInput("tray.setMenu item id is required".into()))?;
        non_empty(id.to_string(), "tray item id")?;
        let label = object
            .get("label")
            .and_then(|value| value.as_str())
            .ok_or_else(|| Error::InvalidInput("tray.setMenu item label is required".into()))?;
        non_empty(label.to_string(), "tray item label")?;
        if label.chars().count() > MAX_TRAY_LABEL_CHARS {
            return Err(Error::InvalidInput(format!(
                "tray.setMenu item labels must be at most {MAX_TRAY_LABEL_CHARS} characters"
            )));
        }
        out.push(serde_json::json!({ "id": id, "label": label }));
    }
    Ok(out)
}

fn validate_completion_result(operation_id: &str, raw: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("result json must be valid JSON: {e}")))?;
    match operation_id {
        OP_CLIPBOARD_READ_TEXT => {
            let object = value.as_object().ok_or_else(|| {
                Error::InvalidInput("clipboard.readText result must be an object".into())
            })?;
            let text = object
                .get("text")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    Error::InvalidInput("clipboard.readText result text is required".into())
                })?;
            if text.len() > crate::operations::MAX_CLIPBOARD_TEXT_BYTES {
                return Err(Error::InvalidInput(format!(
                    "clipboard.readText result text must be at most {} bytes",
                    crate::operations::MAX_CLIPBOARD_TEXT_BYTES
                )));
            }
        }
        OP_DIALOG_SAVE_FILE => {
            let object = value.as_object().ok_or_else(|| {
                Error::InvalidInput("dialog.saveFile result must be an object".into())
            })?;
            if object.get("saved").and_then(|value| value.as_bool()).is_none() {
                return Err(Error::InvalidInput(
                    "dialog.saveFile result saved bool is required".into(),
                ));
            }
        }
        OP_SCREEN_CAPTURE => validate_screen_capture_result(&value)?,
        OP_TRAY_SET_MENU => require_bool(&value, "installed", "tray.setMenu")?,
        OP_SHORTCUT_REGISTER_GLOBAL => {
            require_bool(&value, "registered", "shortcut.registerGlobal")?
        }
        OP_WINDOW_CONTROL => require_bool(&value, "ok", "window.control")?,
        _ => {}
    }
    Ok(())
}

fn validate_screen_capture_result(value: &serde_json::Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("screen.capture result must be an object".into()))?;
    for field in ["hash", "mime", "blobName"] {
        if object.get(field).and_then(|value| value.as_str()).is_none() {
            return Err(Error::InvalidInput(format!(
                "screen.capture result {field} string is required"
            )));
        }
    }
    if object.get("mime").and_then(|value| value.as_str()) != Some("image/png") {
        return Err(Error::InvalidInput(
            "screen.capture result mime must be image/png".into(),
        ));
    }
    for field in ["size", "width", "height"] {
        if object.get(field).and_then(|value| value.as_u64()).is_none() {
            return Err(Error::InvalidInput(format!(
                "screen.capture result {field} number is required"
            )));
        }
    }
    Ok(())
}

fn require_bool(value: &serde_json::Value, field: &str, label: &str) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput(format!("{label} result must be an object")))?;
    if object.get(field).and_then(|value| value.as_bool()).is_none() {
        return Err(Error::InvalidInput(format!(
            "{label} result {field} bool is required"
        )));
    }
    Ok(())
}
