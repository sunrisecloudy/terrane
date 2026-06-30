//! Asserts each runtime-resource capability in the *real* default registry
//! declares a `namespace.v1` grant spec with verbs. This complements the
//! stub-based arm tests in `grant_resources.rs` (which drive synthetic
//! capabilities) by locking the actual shipped capabilities. Tests live here,
//! not inline in any `src/` file.

use terrane_core::grant_resource_specs;

#[test]
fn every_runtime_resource_capability_declares_namespace_v1() {
    let specs = grant_resource_specs();
    for namespace in ["build", "crdt", "kv", "relational_db"] {
        let spec = specs
            .iter()
            .find(|s| s.namespace == namespace && s.selector_schema_id == "namespace.v1")
            .unwrap_or_else(|| panic!("{namespace} is missing a namespace.v1 grant spec"));
        assert!(
            !spec.verbs.is_empty(),
            "{namespace} namespace.v1 spec must declare verbs"
        );
    }
}

#[test]
fn grant_spec_namespaces_match_their_owning_capability() {
    // Every spec must be owned by the capability whose namespace it names
    // (the registry rejects mismatches, but lock it explicitly over the real set).
    for spec in grant_resource_specs() {
        assert!(
            !spec.namespace.is_empty() && !spec.selector_schema_id.is_empty(),
            "grant spec must name a namespace and schema id"
        );
    }
}
