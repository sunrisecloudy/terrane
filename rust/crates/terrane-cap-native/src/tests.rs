use crate::operations::OP_EXTERNAL_OPEN_URL;
use crate::{NativeCapability, NativeRequestStatus};
use terrane_cap_interface::Capability;

#[test]
fn manifest_declares_native_resources_and_grants() {
    let manifest = NativeCapability.manifest();
    assert!(manifest
        .commands
        .iter()
        .any(|spec| spec.name == "native.external.open-url"));
    assert!(manifest
        .resources
        .iter()
        .any(|method| method.name() == "externalOpenUrl"));
    assert_eq!(manifest.grant_resources[0].namespace, "native");
}

#[test]
fn operation_constants_are_stable() {
    assert_eq!(OP_EXTERNAL_OPEN_URL, "external.openUrl");
    assert_eq!(NativeRequestStatus::Pending.as_str(), "pending");
}
