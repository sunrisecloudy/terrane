use std::collections::BTreeSet;

use crate::operations::{
    operation_catalog, NATIVE_OPERATION_SELECTOR_SCHEMA_ID, OP_EXTERNAL_OPEN_URL,
    OP_SCREEN_CAPTURE,
};
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
    assert!(manifest
        .grant_resources
        .iter()
        .any(|spec| spec.selector_schema_id == NATIVE_OPERATION_SELECTOR_SCHEMA_ID));
}

#[test]
fn operation_constants_are_stable() {
    assert_eq!(OP_EXTERNAL_OPEN_URL, "external.openUrl");
    assert_eq!(OP_SCREEN_CAPTURE, "screen.capture");
    assert_eq!(NativeRequestStatus::Pending.as_str(), "pending");
}

#[test]
fn operation_catalog_covers_common_desktop_and_mobile_groups() {
    let groups = operation_catalog()
        .into_iter()
        .map(|entry| entry.group)
        .collect::<BTreeSet<_>>();
    assert_eq!(groups, BTreeSet::from(["common", "desktop", "mobile"]));
}
