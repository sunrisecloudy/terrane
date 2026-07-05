use terrane_cap_native::{
    operation_catalog, NativeRequestRecord, OP_AUDIO_RECORD, OP_CAMERA_CAPTURE_PHOTO,
    OP_CLIPBOARD_READ_TEXT, OP_SCREEN_CAPTURE,
};

use super::{NativeConnector, NativeConnectorInfo, NativeExecutionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedNativeConnector {
    info: NativeConnectorInfo,
}

impl UnsupportedNativeConnector {
    pub fn new(host_id: impl Into<String>, platform: impl Into<String>) -> Self {
        Self {
            info: NativeConnectorInfo::new(
                host_id,
                platform,
                env!("CARGO_PKG_VERSION"),
                conservative_supported_operations(),
            ),
        }
    }

    pub fn with_supported_operations(
        mut self,
        supported_operations: impl IntoIterator<Item = String>,
    ) -> Self {
        self.info.supported_operations = supported_operations.into_iter().collect();
        self
    }
}

impl NativeConnector for UnsupportedNativeConnector {
    fn info(&self) -> NativeConnectorInfo {
        self.info.clone()
    }

    fn execute(&self, request: &NativeRequestRecord) -> NativeExecutionResult {
        NativeExecutionResult::unsupported(request, "this host has no native OS connector")
    }
}

pub fn default_connector() -> UnsupportedNativeConnector {
    UnsupportedNativeConnector::new(
        format!("terrane-host-{}", std::env::consts::OS),
        std::env::consts::OS,
    )
}

fn conservative_supported_operations() -> Vec<String> {
    operation_catalog()
        .into_iter()
        .filter(|entry| entry.status == "v1")
        .map(|entry| entry.id.to_string())
        .filter(|operation| {
            operation != OP_CLIPBOARD_READ_TEXT
                && operation != OP_CAMERA_CAPTURE_PHOTO
                && operation != OP_AUDIO_RECORD
                && operation != OP_SCREEN_CAPTURE
        })
        .collect()
}
