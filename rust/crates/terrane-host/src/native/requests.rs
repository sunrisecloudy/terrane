use terrane_cap_native::{NativeRequestRecord, NativeRequestStatus};
use terrane_core::EventRecord;

use crate::{dispatch_on_core, CommandOutcome, HostCore};

use super::{NativeConnector, NativeExecutionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeDrainOutcome {
    Idle,
    Drained(NativeDrainedRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDrainedRequest {
    pub app: String,
    pub request_id: String,
    pub operation_id: String,
    pub records: Vec<EventRecord>,
}

pub fn observe_connector_on_core<C: NativeConnector + ?Sized>(
    core: &mut HostCore,
    connector: &C,
) -> Result<CommandOutcome, String> {
    let info = connector.info();
    let mut args = vec![info.host_id, info.platform, info.connector_version];
    args.extend(info.supported_operations);
    dispatch_on_core(core, "native.platform.observe", &args)
}

pub fn pending_requests_for_connector<C: NativeConnector + ?Sized>(
    core: &HostCore,
    connector: &C,
) -> Vec<NativeRequestRecord> {
    let info = connector.info();
    let mut requests = core
        .state()
        .native
        .requests
        .values()
        .flat_map(|app_requests| app_requests.values())
        .filter(|record| {
            record.status == NativeRequestStatus::Pending && record.executor_host_id == info.host_id
        })
        .cloned()
        .collect::<Vec<_>>();
    requests.sort_by(|a, b| {
        a.sequence
            .cmp(&b.sequence)
            .then_with(|| a.app.cmp(&b.app))
            .then_with(|| a.request_id.cmp(&b.request_id))
    });
    requests
}

pub fn drain_once_on_core<C: NativeConnector + ?Sized>(
    core: &mut HostCore,
    connector: &C,
) -> Result<NativeDrainOutcome, String> {
    let Some(request) = pending_requests_for_connector(core, connector)
        .into_iter()
        .next()
    else {
        return Ok(NativeDrainOutcome::Idle);
    };
    let (command, payload) = match connector.execute(&request) {
        NativeExecutionResult::Completed(result_json) => ("native.complete", result_json),
        NativeExecutionResult::Failed(error_json) => ("native.fail", error_json),
        NativeExecutionResult::Cancelled(reason) => ("native.cancel", reason),
    };
    let args = vec![request.app.clone(), request.request_id.clone(), payload];
    let outcome = dispatch_on_core(core, command, &args)?;
    Ok(NativeDrainOutcome::Drained(NativeDrainedRequest {
        app: request.app,
        request_id: request.request_id,
        operation_id: request.operation_id,
        records: outcome.records,
    }))
}
