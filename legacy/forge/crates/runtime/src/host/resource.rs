//! `ctx.resource.invoke` / `read` / `materialize` for [`HostContext`].

use super::HostContext;
use crate::files::{FileWriteRequest, FileWriteResponse};
use crate::resource::{
    ResourceInvokeOptions, ResourceInvokeResult, ResourceMaterializeRequest,
    ResourceMaterializeResponse, ResourceReadRequest, ResourceReadResponse, RESOURCE_KIND_CAMERA,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use forge_domain::{CoreError, ResourceAssetBlob, Result};
use forge_policy::HostCall;
use serde_json::Value;

impl HostContext<'_> {
    /// `ctx.resource.invoke(kind, args?)` — capture a platform resource; returns
    /// metadata + `asset_id` only (bytes stored run-scoped).
    pub fn resource_invoke(&mut self, kind: String, args: Value) -> Result<ResourceInvokeResult> {
        let args_for_record = serde_json::json!([kind.clone(), args.clone()]);

        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for resource.invoke call".into(),
            );
            self.recorder
                .record_denial("resource.invoke", args_for_record, &err)?;
            return Err(err);
        }

        if kind != RESOURCE_KIND_CAMERA {
            let err = CoreError::ValidationError(format!(
                "unsupported resource kind {kind:?}; v1 supports \"camera\" only (forge/spec/resources.md)"
            ));
            self.recorder
                .record_denial("resource.invoke", args_for_record, &err)?;
            return Err(err);
        }

        let options = parse_invoke_options(&args)?;
        let host_call = HostCall::Resource {
            kind: kind.clone(),
            args: args.clone(),
        };
        if let Err(err) = self.policy.check_context_gates(&host_call) {
            self.recorder
                .record_denial("resource.invoke", args_for_record, &err)?;
            return Err(err);
        }
        if let Err(err) = self.policy.check(&host_call) {
            self.recorder
                .record_denial("resource.invoke", args_for_record, &err)?;
            return Err(err);
        }

        self.budgets.check_resource_call()?;

        let bridge = &mut *self.bridge;
        let store = &mut self.resource_store;
        let response_json = self.recorder.host_call(
            "resource.invoke",
            args_for_record,
            || {
                let capture = bridge.resource_invoke(&kind, &options)?;
                let asset_id = store.next_asset_id(&kind);
                let blob = ResourceAssetBlob {
                    bytes_base64: BASE64.encode(&capture.bytes),
                    content_type: capture.content_type.clone(),
                    width: capture.width,
                    height: capture.height,
                };
                store.insert(asset_id.clone(), blob);
                let resp = ResourceInvokeResult {
                    asset_id,
                    content_type: capture.content_type,
                    width: capture.width,
                    height: capture.height,
                    size_bytes: capture.bytes.len() as u64,
                };
                serde_json::to_value(&resp).map_err(|e| {
                    CoreError::RuntimeError(format!("resource.invoke response serialize failed: {e}"))
                })
            },
        )?;

        serde_json::from_value::<ResourceInvokeResult>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("resource.invoke response decode failed: {e}"))
        })
    }

    /// `ctx.resource.read(asset_id, request?)` — lazy byte retrieval.
    pub fn resource_read(
        &mut self,
        asset_id: String,
        request: ResourceReadRequest,
    ) -> Result<ResourceReadResponse> {
        let args = serde_json::json!([asset_id.clone(), request]);
        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets for resource.read call".into(),
            );
            self.recorder.record_denial("resource.read", args, &err)?;
            return Err(err);
        }

        let host_call = HostCall::ResourceRead {
            asset_id: asset_id.clone(),
        };
        if let Err(err) = self.policy.check(&host_call) {
            self.recorder.record_denial("resource.read", args, &err)?;
            return Err(err);
        }
        self.budgets.check_resource_call()?;

        let store = &self.resource_store;
        let response_json = self.recorder.host_call("resource.read", args, || {
            let blob = store.get(&asset_id).ok_or_else(|| {
                CoreError::ValidationError(format!(
                    "ctx.resource.read unknown asset_id {asset_id:?} (invoke camera first)"
                ))
            })?;
            let bytes = BASE64.decode(blob.bytes_base64.as_bytes()).map_err(|e| {
                CoreError::RuntimeError(format!("resource asset bytes_base64 invalid: {e}"))
            })?;
            let resp = ResourceReadResponse {
                asset_id: asset_id.clone(),
                bytes_base64: blob.bytes_base64.clone(),
                size_bytes: bytes.len() as u64,
                content_type: blob.content_type.clone(),
            };
            serde_json::to_value(&resp)
                .map_err(|e| CoreError::RuntimeError(format!("resource.read serialize failed: {e}")))
        })?;

        serde_json::from_value::<ResourceReadResponse>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("resource.read response decode failed: {e}"))
        })
    }

    /// `ctx.resource.materialize(asset_id, request)` — copy blob into files sandbox.
    pub fn resource_materialize(
        &mut self,
        asset_id: String,
        request: ResourceMaterializeRequest,
    ) -> Result<ResourceMaterializeResponse> {
        let args = serde_json::to_value(&(asset_id.clone(), &request))
            .unwrap_or(Value::Null);

        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets for resource.materialize call".into(),
            );
            self.recorder
                .record_denial("resource.materialize", args.clone(), &err)?;
            return Err(err);
        }

        let host_call = HostCall::ResourceMaterialize {
            asset_id: asset_id.clone(),
            handle: request.handle.clone(),
            path: request.path.clone(),
        };
        if let Err(err) = self.policy.check(&host_call) {
            self.recorder
                .record_denial("resource.materialize", args.clone(), &err)?;
            return Err(err);
        }

        let blob = self
            .resource_store
            .get(&asset_id)
            .cloned()
            .ok_or_else(|| {
                let err = CoreError::ValidationError(format!(
                    "ctx.resource.materialize unknown asset_id {asset_id:?} (invoke camera first)"
                ));
                let _ = self.recorder.record_denial("resource.materialize", args.clone(), &err);
                err
            })?;

        self.budgets.check_resource_call()?;

        let write_req = FileWriteRequest {
            handle: request.handle.clone(),
            path: request.path.clone(),
            bytes_base64: blob.bytes_base64.clone(),
            content_type: Some(blob.content_type.clone()),
            mode: "create_or_truncate".into(),
        };

        let write_resp: FileWriteResponse = self.files_write(write_req)?;

        let response_json = self.recorder.host_call("resource.materialize", args, || {
            let resp = ResourceMaterializeResponse {
                asset_id: asset_id.clone(),
                handle: request.handle.clone(),
                path: request.path.clone(),
                written_bytes: write_resp.written_bytes,
                content_type: blob.content_type.clone(),
            };
            serde_json::to_value(&resp).map_err(|e| {
                CoreError::RuntimeError(format!("resource.materialize serialize failed: {e}"))
            })
        })?;

        serde_json::from_value::<ResourceMaterializeResponse>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("resource.materialize response decode failed: {e}"))
        })
    }

    /// Export run-scoped resource blobs for the [`RunRecord`] sidecar.
    pub fn resource_assets(&self) -> std::collections::BTreeMap<String, ResourceAssetBlob> {
        self.resource_store.clone().into_assets()
    }
}

fn parse_invoke_options(args: &Value) -> Result<ResourceInvokeOptions> {
    match args {
        Value::Null => Ok(ResourceInvokeOptions::default()),
        Value::Array(items) => {
            let Some(first) = items.first() else {
                return Ok(ResourceInvokeOptions::default());
            };
            serde_json::from_value(first.clone()).map_err(|e| {
                CoreError::ValidationError(format!(
                    "ctx.resource.invoke args[0] must be an options object: {e}"
                ))
            })
        }
        other => Err(CoreError::ValidationError(format!(
            "ctx.resource.invoke args must be an array or null, got {other}"
        ))),
    }
}