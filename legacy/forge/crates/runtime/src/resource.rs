//! `ctx.resource` types and the injectable platform resource provider seam.
//!
//! prd-merged/01 CR-3 platform capabilities; `forge/spec/resources.md`.
//! Camera capture returns a handle (`asset_id`) with metadata only; bytes stay in
//! the run-scoped store until `read` or `materialize`.

use forge_domain::{CoreError, ResourceAssetBlob, Result};
use serde::{Deserialize, Serialize};

/// v1 supported resource kinds.
pub const RESOURCE_KIND_CAMERA: &str = "camera";

/// Optional first argument to `ctx.resource.invoke("camera", [options])`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ResourceInvokeOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_dimension: Option<u32>,
}

/// Success response for `resource.invoke` — metadata only, no inline bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInvokeResult {
    pub asset_id: String,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub size_bytes: u64,
}

/// Optional request shape for `ctx.resource.read(asset_id, request?)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ResourceReadRequest {}

/// Response for `ctx.resource.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceReadResponse {
    pub asset_id: String,
    pub bytes_base64: String,
    pub size_bytes: u64,
    pub content_type: String,
}

/// Request for `ctx.resource.materialize(asset_id, request)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceMaterializeRequest {
    pub handle: String,
    pub path: String,
}

/// Response for `ctx.resource.materialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceMaterializeResponse {
    pub asset_id: String,
    pub handle: String,
    pub path: String,
    pub written_bytes: u64,
    pub content_type: String,
}

/// Raw capture returned by a live platform provider before asset id assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceCapture {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Platform resource effect seam (camera in v1). Native shells implement this;
/// tests inject [`MockResourceProvider`].
pub trait ResourceProvider {
    fn invoke(&mut self, kind: &str, options: &ResourceInvokeOptions) -> Result<ResourceCapture>;
}

/// Deterministic mock camera for tests and the spine demo.
#[derive(Debug, Clone, Default)]
pub struct MockResourceProvider {
    pub cancelled: bool,
    pub unavailable: bool,
}

impl MockResourceProvider {
    /// Fixed JPEG-ish bytes used across all mock captures (deterministic).
    pub fn mock_jpeg_bytes() -> Vec<u8> {
        vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
        ]
    }
}

impl ResourceProvider for MockResourceProvider {
    fn invoke(&mut self, kind: &str, options: &ResourceInvokeOptions) -> Result<ResourceCapture> {
        if kind != RESOURCE_KIND_CAMERA {
            return Err(CoreError::ValidationError(format!(
                "unsupported resource kind {kind:?} in MockResourceProvider (v1 supports \"camera\" only)"
            )));
        }
        if self.unavailable {
            return Err(CoreError::PlatformUnavailable(
                "the host platform has not granted the camera capability".into(),
            ));
        }
        if self.cancelled {
            return Err(CoreError::RuntimeError("resource_cancelled".into()));
        }
        let bytes = Self::mock_jpeg_bytes();
        if let Some(cap) = options.max_bytes {
            if bytes.len() as u64 > cap {
                return Err(CoreError::ResourceLimitExceeded(format!(
                    "camera capture {} bytes exceeds max_bytes = {cap}",
                    bytes.len()
                )));
            }
        }
        Ok(ResourceCapture {
            bytes,
            content_type: options
                .content_type
                .clone()
                .unwrap_or_else(|| "image/jpeg".into()),
            width: Some(640),
            height: Some(480),
        })
    }
}

/// Run-scoped store mapping `asset_id` → blob metadata + bytes.
#[derive(Debug, Clone, Default)]
pub struct ResourceStore {
    assets: std::collections::BTreeMap<String, ResourceAssetBlob>,
    invoke_seq: u64,
}

impl ResourceStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_recorded(assets: std::collections::BTreeMap<String, ResourceAssetBlob>) -> Self {
        let invoke_seq = assets.len() as u64;
        ResourceStore { assets, invoke_seq }
    }

    pub fn next_asset_id(&mut self, kind: &str) -> String {
        let id = format!("res_{kind}_{}", self.invoke_seq);
        self.invoke_seq = self.invoke_seq.saturating_add(1);
        id
    }

    pub fn insert(&mut self, asset_id: String, blob: ResourceAssetBlob) {
        self.assets.insert(asset_id, blob);
    }

    pub fn get(&self, asset_id: &str) -> Option<&ResourceAssetBlob> {
        self.assets.get(asset_id)
    }

    pub fn into_assets(self) -> std::collections::BTreeMap<String, ResourceAssetBlob> {
        self.assets
    }
}

pub fn live_resource_forbidden(kind: &str) -> CoreError {
    CoreError::RuntimeError(format!(
        "ctx.resource.invoke({kind:?}) reached live bridge in replay mode or without a provider"
    ))
}