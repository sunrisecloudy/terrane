use terrane_cap_native::NativeRequestRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeConnectorInfo {
    pub host_id: String,
    pub platform: String,
    pub connector_version: String,
    pub supported_operations: Vec<String>,
}

impl NativeConnectorInfo {
    pub fn new(
        host_id: impl Into<String>,
        platform: impl Into<String>,
        connector_version: impl Into<String>,
        supported_operations: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            host_id: host_id.into(),
            platform: platform.into(),
            connector_version: connector_version.into(),
            supported_operations: supported_operations.into_iter().collect(),
        }
    }
}

pub trait NativeConnector {
    fn info(&self) -> NativeConnectorInfo;

    fn execute(&self, request: &NativeRequestRecord) -> NativeExecutionResult;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeExecutionResult {
    Completed(String),
    Failed(String),
    Cancelled(String),
}

impl NativeExecutionResult {
    pub fn completed_json(value: serde_json::Value) -> Self {
        Self::Completed(value.to_string())
    }

    pub fn failed_json(value: serde_json::Value) -> Self {
        Self::Failed(value.to_string())
    }

    pub fn unsupported(request: &NativeRequestRecord, message: &str) -> Self {
        Self::failed_json(serde_json::json!({
            "error": "unsupported_native_operation",
            "operation": request.operation_id,
            "message": message,
        }))
    }
}
