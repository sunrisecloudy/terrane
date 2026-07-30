use std::collections::{BTreeMap, BTreeSet};

use terrane_cap_protocol::is_fundamental;
use terrane_core::default_registry;

const PACKAGE_SCRIPT: &str = include_str!("../../../../scripts/package-native-capabilities.sh");
const WORKER_MANIFEST: &str = include_str!("../Cargo.toml");

#[test]
fn release_script_packages_every_dynamic_capability_with_a_worker_feature() {
    let entries = package_entries();
    let registry = default_registry();
    let dynamic = registry
        .namespaces()
        .filter(|namespace| !is_fundamental(namespace))
        .collect::<BTreeSet<_>>();

    assert_eq!(dynamic.len(), 42, "{dynamic:#?}");
    assert_eq!(
        entries.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        dynamic,
        "release script and dynamic registry diverged"
    );

    for feature in entries.values() {
        assert!(
            WORKER_MANIFEST
                .lines()
                .any(|line| line.starts_with(&format!("{feature} = "))),
            "release worker feature is missing from Cargo.toml: {feature}"
        );
    }
}

fn package_entries() -> BTreeMap<String, String> {
    let mut inside = false;
    let mut entries = BTreeMap::new();
    for line in PACKAGE_SCRIPT.lines() {
        if line == "CAPABILITIES=(" {
            inside = true;
            continue;
        }
        if inside && line == ")" {
            break;
        }
        if !inside {
            continue;
        }
        let entry = line.trim();
        if entry.is_empty() {
            continue;
        }
        let (namespace, feature) = entry
            .split_once(':')
            .unwrap_or_else(|| panic!("invalid capability release entry: {entry}"));
        assert!(
            entries
                .insert(namespace.to_string(), feature.to_string())
                .is_none(),
            "duplicate capability release entry: {namespace}"
        );
    }
    entries
}
