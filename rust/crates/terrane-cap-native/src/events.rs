use std::collections::BTreeSet;

use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::types::{
    terminal_retention_limit, Cancelled, NativePlatformObservation, NativeRequestRecord,
    NativeRequestStatus, NativeState, PlatformObserved, Requested, Terminal,
};

pub fn platform_observed_event(
    host_id: &str,
    platform: &str,
    connector_version: &str,
    supported_operations: Vec<String>,
) -> Result<EventRecord> {
    encode_event(
        "native.platform.observed",
        &PlatformObserved {
            host_id: host_id.to_string(),
            platform: platform.to_string(),
            connector_version: connector_version.to_string(),
            supported_operations,
        },
    )
}

pub(crate) struct RequestedEvent<'a> {
    pub app: &'a str,
    pub request_id: &'a str,
    pub operation_id: &'a str,
    pub executor_host_id: &'a str,
    pub origin_replica: Option<u64>,
    pub sequence: u64,
    pub input_json: String,
    pub result_size_class: &'a str,
    pub retention_class: &'a str,
}

pub(crate) fn requested_event(event: RequestedEvent<'_>) -> Result<EventRecord> {
    encode_event(
        "native.requested",
        &Requested {
            request_id: event.request_id.to_string(),
            app: event.app.to_string(),
            operation_id: event.operation_id.to_string(),
            executor_host_id: event.executor_host_id.to_string(),
            origin_replica: event.origin_replica,
            sequence: event.sequence,
            input_json: event.input_json,
            result_size_class: event.result_size_class.to_string(),
            retention_class: event.retention_class.to_string(),
        },
    )
}

pub fn completed_event(app: &str, request_id: &str, result_json: String) -> Result<EventRecord> {
    encode_event(
        "native.completed",
        &Terminal {
            app: app.to_string(),
            request_id: request_id.to_string(),
            payload_json: result_json,
        },
    )
}

pub fn failed_event(app: &str, request_id: &str, error_json: String) -> Result<EventRecord> {
    encode_event(
        "native.failed",
        &Terminal {
            app: app.to_string(),
            request_id: request_id.to_string(),
            payload_json: error_json,
        },
    )
}

pub fn cancelled_event(app: &str, request_id: &str, reason: &str) -> Result<EventRecord> {
    encode_event(
        "native.cancelled",
        &Cancelled {
            app: app.to_string(),
            request_id: request_id.to_string(),
            reason: reason.to_string(),
        },
    )
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "native.platform.observed" => {
            let event: PlatformObserved = decode_event(record)?;
            let state = state_mut::<NativeState>(state, "native")?;
            state.active_host_id = Some(event.host_id.clone());
            state.platforms.insert(
                event.host_id.clone(),
                NativePlatformObservation {
                    host_id: event.host_id,
                    platform: event.platform,
                    connector_version: event.connector_version,
                    supported_operations: event.supported_operations.into_iter().collect(),
                },
            );
        }
        "native.requested" => {
            let event: Requested = decode_event(record)?;
            let state = state_mut::<NativeState>(state, "native")?;
            state.next_sequence = state.next_sequence.max(event.sequence);
            state
                .requests
                .entry(event.app.clone())
                .or_default()
                .entry(event.request_id.clone())
                .or_insert_with(|| NativeRequestRecord {
                    request_id: event.request_id,
                    app: event.app,
                    operation_id: event.operation_id,
                    status: NativeRequestStatus::Pending,
                    executor_host_id: event.executor_host_id,
                    origin_replica: event.origin_replica,
                    sequence: event.sequence,
                    input_json: event.input_json,
                    result_size_class: event.result_size_class,
                    retention_class: event.retention_class,
                    result_json: None,
                    error_json: None,
                });
        }
        "native.completed" => {
            let event: Terminal = decode_event(record)?;
            let state = state_mut::<NativeState>(state, "native")?;
            if let Some(record) = request_mut(state, &event.app, &event.request_id) {
                if !record.status.is_terminal() {
                    record.status = NativeRequestStatus::Completed;
                    record.result_json = Some(event.payload_json);
                }
            }
            enforce_retention(state, &event.app);
        }
        "native.failed" => {
            let event: Terminal = decode_event(record)?;
            let state = state_mut::<NativeState>(state, "native")?;
            if let Some(record) = request_mut(state, &event.app, &event.request_id) {
                if !record.status.is_terminal() {
                    record.status = NativeRequestStatus::Failed;
                    record.error_json = Some(event.payload_json);
                }
            }
            enforce_retention(state, &event.app);
        }
        "native.cancelled" => {
            let event: Cancelled = decode_event(record)?;
            let state = state_mut::<NativeState>(state, "native")?;
            if let Some(record) = request_mut(state, &event.app, &event.request_id) {
                if !record.status.is_terminal() {
                    record.status = NativeRequestStatus::Cancelled;
                    record.error_json = Some(cancelled_json(&event.reason));
                }
            }
            enforce_retention(state, &event.app);
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            state_mut::<NativeState>(state, "native")?
                .requests
                .remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "native.platform.observed" => {
            let event: PlatformObserved = decode_event(record).ok()?;
            Some(format!(
                "native.platform.observed {} {} ({} ops)",
                event.host_id,
                event.platform,
                event.supported_operations.len()
            ))
        }
        "native.requested" => {
            let event: Requested = decode_event(record).ok()?;
            Some(format!(
                "native.requested {} {} {} -> {}",
                event.app, event.request_id, event.operation_id, event.executor_host_id
            ))
        }
        "native.completed" => {
            let event: Terminal = decode_event(record).ok()?;
            Some(format!(
                "native.completed {} {} ({} bytes)",
                event.app,
                event.request_id,
                event.payload_json.len()
            ))
        }
        "native.failed" => {
            let event: Terminal = decode_event(record).ok()?;
            Some(format!("native.failed {} {}", event.app, event.request_id))
        }
        "native.cancelled" => {
            let event: Cancelled = decode_event(record).ok()?;
            Some(format!(
                "native.cancelled {} {} {}",
                event.app, event.request_id, event.reason
            ))
        }
        _ => None,
    }
}

fn request_mut<'a>(
    state: &'a mut NativeState,
    app: &str,
    request_id: &str,
) -> Option<&'a mut NativeRequestRecord> {
    state.requests.get_mut(app)?.get_mut(request_id)
}

fn enforce_retention(state: &mut NativeState, app: &str) {
    let Some(requests) = state.requests.get_mut(app) else {
        return;
    };
    let mut terminal: Vec<(u64, String)> = requests
        .iter()
        .filter(|(_, record)| record.status.is_terminal())
        .map(|(id, record)| (record.sequence, id.clone()))
        .collect();
    let limit = terminal_retention_limit();
    if terminal.len() <= limit {
        return;
    }
    terminal.sort_by_key(|(sequence, id)| (*sequence, id.clone()));
    let remove_count = terminal.len() - limit;
    let to_remove: BTreeSet<String> = terminal
        .into_iter()
        .take(remove_count)
        .map(|(_, id)| id)
        .collect();
    for id in to_remove {
        requests.remove(&id);
    }
}

fn cancelled_json(reason: &str) -> String {
    serde_json::json!({
        "status": "cancelled",
        "reason": reason,
    })
    .to_string()
}
