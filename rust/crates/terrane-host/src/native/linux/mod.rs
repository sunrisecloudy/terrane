use super::UnsupportedNativeConnector;

pub fn default_connector() -> UnsupportedNativeConnector {
    UnsupportedNativeConnector::new("terrane-host-linux", "linux")
}
