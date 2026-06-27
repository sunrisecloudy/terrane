//! The exported public contract surface stays consistent with the live
//! declarations — so `public-contract.json` (which premium pins) can't lie about
//! what terrane implements.

use nanoserde::{DeJson, SerJson};
use terrane_cli::contract_surface;

#[test]
fn surface_is_derived_from_the_live_declarations() {
    let s = contract_surface();

    assert_eq!(s.contract_version, terrane_api::CONTRACT_VERSION);
    // The host half is exactly the terrane-api host contract.
    assert_eq!(s.host, terrane_api::host_contract());

    // Every registered capability is listed.
    for ns in [
        "app", "builder", "codex", "kv", "crdt", "net", "model", "host", "replica",
    ] {
        assert!(
            s.capabilities.iter().any(|c| c == ns),
            "missing capability {ns}"
        );
    }

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
    let crdt = s
        .resources
        .iter()
        .find(|r| r.namespace == "crdt")
        .expect("crdt resource");
    assert!(crdt.methods.iter().any(|m| m.name == "mapSet"));

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
