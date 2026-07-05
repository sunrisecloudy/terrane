//! The exported public contract surface stays consistent with the live
//! declarations — so `public-contract.json` (which premium pins) can't lie about
//! what terrane implements.

use nanoserde::{DeJson, SerJson};
use terrane_host::cli::contract_surface;

#[test]
fn surface_is_derived_from_the_live_declarations() {
    let s = contract_surface();

    assert_eq!(s.contract_version, terrane_api::CONTRACT_VERSION);
    // The host half is exactly the terrane-api host contract.
    assert_eq!(s.host, terrane_api::host_contract());

    // Every registered capability is listed.
    for ns in [
        "app",
        "build",
        "builder",
        "harness",
        "kv",
        "relational_db",
        "crdt",
        "net",
        "model",
        "replica",
        "js-runtime",
        "wasm-runtime",
    ] {
        assert!(
            s.capabilities.iter().any(|c| c == ns),
            "missing capability {ns}"
        );
    }
    assert!(s.capability_docs.iter().any(|doc| doc.namespace == "kv"));
    let document = s
        .capability_docs
        .iter()
        .find(|doc| doc.namespace == "document")
        .expect("document docs");
    assert_eq!(document.status, "stable");
    assert!(document
        .schemas
        .iter()
        .any(|schema| schema.id == "document.schema.json"));
    let rdb = s
        .capability_docs
        .iter()
        .find(|doc| doc.namespace == "relational_db")
        .expect("relational_db docs");
    assert_eq!(rdb.status, "stable");
    assert!(rdb
        .schemas
        .iter()
        .any(|schema| schema.id == "terrane.relational_db.tableSpec.v1"));
    assert!(rdb.internal.is_empty());

    // The resource surface carries the declared backend methods.
    let kv = s
        .resources
        .iter()
        .find(|r| r.namespace == "kv")
        .expect("kv resource");
    assert!(kv
        .methods
        .iter()
        .any(|m| m.name == "set" && m.kind == "write"));
    let kv_spec = kv
        .grant_specs
        .iter()
        .find(|spec| spec.selector_schema_id == "namespace.v1")
        .expect("kv namespace.v1 grant spec");
    assert_eq!(kv_spec.verbs, vec!["read", "write"]);
    assert!(kv_spec.compatibility.backward && kv_spec.compatibility.forward);
    assert_eq!(kv_spec.unknown_selector_schema_policy, "deny");
    let crdt = s
        .resources
        .iter()
        .find(|r| r.namespace == "crdt")
        .expect("crdt resource");
    assert!(crdt.methods.iter().any(|m| m.name == "mapSet"));
    let build = s
        .resources
        .iter()
        .find(|r| r.namespace == "build")
        .expect("build resource");
    assert!(build
        .methods
        .iter()
        .any(|m| m.name == "compileTs" && m.kind == "read"));
    let build_spec = build
        .grant_specs
        .iter()
        .find(|spec| spec.selector_schema_id == "namespace.v1")
        .expect("build namespace.v1 grant spec");
    assert_eq!(build_spec.verbs, vec!["read"]);
    assert!(s.capability_docs.iter().any(|d| {
        d.namespace == "build"
            && d.manifest
                .grant_resources
                .iter()
                .any(|spec| spec.selector_schema_id == "namespace.v1")
    }));
    assert!(s.capability_docs.iter().any(|d| {
        d.namespace == "relational_db"
            && d.schemas
                .iter()
                .any(|schema| schema.id == "terrane.relational_db.tableSpec.v1")
    }));

    // The app + sync contracts.
    assert_eq!(s.app.actions_verb, terrane_api::ACTIONS_VERB);
    assert!(s
        .sync
        .syncable_event_kinds
        .iter()
        .any(|k| k == "crdt.update"));

    // It round-trips through JSON (what the export emits and premium parses).
    let back = terrane_api::PublicSurface::deserialize_json(&s.serialize_json()).unwrap();
    assert_eq!(back, s);
}
