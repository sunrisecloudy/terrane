use terrane_cap_native::NativeRequestRecord;

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
                Vec::<String>::new(),
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
