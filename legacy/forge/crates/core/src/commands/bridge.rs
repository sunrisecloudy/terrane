//! Bridge security commands (C8/C10/C11): network preflight, envelope gate,
//! deterministic audit record shapes.

use forge_domain::{CoreCommand, CoreError, Result, WebappNetworkPolicy, WebappResourceBudget};
use forge_policy::{
    bridge_call_id, core_action_id, core_event_id, runtime_session_id, state_version_before,
    validate_bridge_envelope, BridgeCallRecord, BridgeEnvelopeRequest, BridgePlatformIds,
    CoreActionRecord, CoreEventRecord, RuntimeSessionMetadata, WebappNetRequest, check_webapp_network,
};
use serde::Deserialize;

use super::super::WorkspaceCore;
use super::{take_field, bool_field};

impl WorkspaceCore {
    pub(in crate::workspace) fn cmd_bridge_validate_network_request(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let request: WebappNetRequest = take_field(cmd, "request")?;
        let policy: WebappNetworkPolicy = take_field(cmd, "network_policy")?;
        let budget: Option<WebappResourceBudget> = cmd
            .payload
            .get("resource_budget")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| CoreError::ValidationError(format!("resource_budget is malformed: {e}")))?;
        let decision = check_webapp_network(&policy, &request, budget.as_ref());
        Ok(serde_json::to_value(decision).expect("WebappNetDecision serializes"))
    }

    pub(in crate::workspace) fn cmd_bridge_validate_envelope(
        &mut self,
        _cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let input: BridgeEnvelopeRequest = take_field(_cmd, "input")?;
        let decision = validate_bridge_envelope(&input);
        Ok(serde_json::to_value(decision).expect("BridgeEnvelopeDecision serializes"))
    }

    pub(in crate::workspace) fn cmd_bridge_prepare_session(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let ids: BridgePlatformIds = take_field(cmd, "platform_ids")?;
        let app_id: String = take_field(cmd, "app_id")?;
        let mount_token: String = take_field(cmd, "mount_token")?;
        let metadata: RuntimeSessionMetadata = cmd
            .payload
            .get("metadata")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| CoreError::ValidationError(format!("metadata is malformed: {e}")))?
            .unwrap_or_else(|| RuntimeSessionMetadata::running_webview("native-bridge"));
        Ok(serde_json::json!({
            "session_id": runtime_session_id(&ids, &app_id, &mount_token),
            "metadata": metadata,
        }))
    }

    pub(in crate::workspace) fn cmd_bridge_record_call(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct Payload {
            platform_ids: BridgePlatformIds,
            session_id: String,
            request_id: String,
            app_id: String,
            install_id: Option<String>,
            method: String,
            params: serde_json::Value,
            ok: bool,
            #[serde(default)]
            result: Option<serde_json::Value>,
            #[serde(default)]
            error: Option<serde_json::Value>,
            #[serde(default)]
            duration_ms: i64,
        }
        let payload: Payload = take_field(cmd, "record")?;
        let record = BridgeCallRecord {
            bridge_call_id: bridge_call_id(&payload.platform_ids, &payload.request_id),
            session_id: payload.session_id,
            app_id: payload.app_id,
            install_id: payload.install_id,
            method: payload.method,
            params_json: payload.params,
            result_json: if payload.ok { payload.result } else { None },
            error_json: if payload.ok { None } else { payload.error },
            duration_ms: payload.duration_ms,
        };
        Ok(serde_json::to_value(record).expect("BridgeCallRecord serializes"))
    }

    pub(in crate::workspace) fn cmd_bridge_record_core_event(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct Payload {
            platform_ids: BridgePlatformIds,
            session_id: String,
            request_id: String,
            app_id: String,
            install_id: Option<String>,
            event: serde_json::Value,
            #[serde(default)]
            result_state_version: Option<i64>,
            #[serde(default)]
            actions: Vec<serde_json::Value>,
        }
        let payload: Payload = take_field(cmd, "record")?;
        let event_id = core_event_id(&payload.platform_ids, &payload.request_id);
        let event = CoreEventRecord {
            event_id: event_id.clone(),
            session_id: payload.session_id.clone(),
            app_id: payload.app_id.clone(),
            install_id: payload.install_id,
            state_version_before: state_version_before(payload.result_state_version),
            event_json: payload.event,
        };
        let actions: Vec<CoreActionRecord> = payload
            .actions
            .into_iter()
            .enumerate()
            .map(|(index, action)| CoreActionRecord {
                action_id: core_action_id(&payload.platform_ids, &payload.request_id, index),
                event_id: event_id.clone(),
                session_id: payload.session_id.clone(),
                app_id: payload.app_id.clone(),
                action_json: action,
            })
            .collect();
        Ok(serde_json::json!({ "event": event, "actions": actions }))
    }

    pub(in crate::workspace) fn cmd_bridge_record_crash_recovery(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let source: String = take_field(cmd, "source")?;
        let can_auto_remount = bool_field(cmd, "can_auto_remount")?;
        let metadata = RuntimeSessionMetadata::terminated(source, can_auto_remount);
        Ok(serde_json::json!({
            "reloadOffered": metadata.reload_offered,
            "canAutoRemount": metadata.can_auto_remount,
            "metadata": metadata,
        }))
    }
}