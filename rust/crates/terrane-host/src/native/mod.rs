//! Trusted host connector layer for `terrane-cap-native`.
//!
//! The deterministic capability records request/result facts. This module owns
//! the host-side contract that can observe a platform and drain pending native
//! work explicitly.

pub mod android;
pub mod connector;
pub mod ios;
pub mod linux;
pub mod macos;
pub mod requests;
pub mod unsupported;
pub mod windows;

pub use connector::{NativeConnector, NativeConnectorInfo, NativeExecutionResult};
pub use requests::{
    drain_once_on_core, observe_connector_on_core, pending_requests_for_connector,
    NativeDrainOutcome, NativeDrainedRequest,
};
pub use unsupported::UnsupportedNativeConnector;

#[cfg(target_os = "android")]
pub fn default_connector() -> UnsupportedNativeConnector {
    android::default_connector()
}

#[cfg(target_os = "ios")]
pub fn default_connector() -> UnsupportedNativeConnector {
    ios::default_connector()
}

#[cfg(target_os = "linux")]
pub fn default_connector() -> UnsupportedNativeConnector {
    linux::default_connector()
}

#[cfg(target_os = "macos")]
pub fn default_connector() -> UnsupportedNativeConnector {
    macos::default_connector()
}

#[cfg(target_os = "windows")]
pub fn default_connector() -> UnsupportedNativeConnector {
    windows::default_connector()
}

#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
)))]
pub fn default_connector() -> UnsupportedNativeConnector {
    unsupported::default_connector()
}
