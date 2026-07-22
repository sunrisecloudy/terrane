use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use terrane_cap_interface::{GrantResourceSpec, ResourceMethod};
use terrane_cap_protocol::{
    is_fundamental, sha256_file, ActivationMode, BundleManifest, CapabilityDeclaration,
    OwnedGrantResourceSpec, OwnedResourceMethod, BUNDLE_FORMAT_VERSION, PROTOCOL_VERSION,
};
use terrane_core::{default_registry, Registry};

pub const DYNAMIC_CAPABILITY_COUNT: usize = 41;

pub fn manifests(
    executable_sha256: &str,
    platform: &str,
    architecture: &str,
) -> Result<BTreeMap<String, BundleManifest>, String> {
    let registry = default_registry();
    let mut output = BTreeMap::new();
    for namespace in registry.namespaces() {
        if is_fundamental(namespace) {
            continue;
        }
        let capability = registry.get(namespace).map_err(|error| error.to_string())?;
        let declaration = declaration(&registry, namespace)?;
        let manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            namespace: namespace.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            state_schema_version: 1,
            platform: platform.to_string(),
            architecture: architecture.to_string(),
            executable: "terrane-cap-worker".to_string(),
            executable_sha256: executable_sha256.to_string(),
            signature: String::new(),
            dependencies: dynamic_dependencies(namespace)
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            activation: activation_mode(namespace),
            declaration,
        };
        capability
            .doc(false)
            .namespace
            .eq(namespace)
            .then_some(())
            .ok_or_else(|| {
                format!("capability documentation namespace mismatch for {namespace}")
            })?;
        manifest.validate().map_err(|error| error.to_string())?;
        output.insert(namespace.to_string(), manifest);
    }
    if output.len() != DYNAMIC_CAPABILITY_COUNT {
        return Err(format!(
            "expected {DYNAMIC_CAPABILITY_COUNT} dynamic capabilities, found {}",
            output.len()
        ));
    }
    Ok(output)
}

pub fn package_all(
    worker: &Path,
    output: &Path,
    signing_key: &SigningKey,
    platform: &str,
    architecture: &str,
) -> Result<Vec<PathBuf>, String> {
    if !worker.is_file() {
        return Err(format!(
            "worker binary does not exist: {}",
            worker.display()
        ));
    }
    let executable_sha256 = sha256_file(worker).map_err(|error| error.to_string())?;
    let mut packaged = Vec::new();
    for (namespace, mut manifest) in manifests(&executable_sha256, platform, architecture)? {
        manifest.signature = hex(&signing_key
            .sign(
                &manifest
                    .signing_payload()
                    .map_err(|error| error.to_string())?,
            )
            .to_bytes());
        let dir = output.join(&namespace);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let executable = dir.join(&manifest.executable);
        fs::hard_link(worker, &executable)
            .or_else(|_| fs::copy(worker, &executable).map(|_| ()))
            .map_err(|error| error.to_string())?;
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        packaged.push(dir);
    }
    Ok(packaged)
}

fn declaration(registry: &Registry, namespace: &str) -> Result<CapabilityDeclaration, String> {
    let manifest = registry
        .get(namespace)
        .map_err(|error| error.to_string())?
        .manifest();
    Ok(CapabilityDeclaration {
        commands: manifest
            .commands
            .into_iter()
            .map(|value| value.name.to_string())
            .collect(),
        events: manifest
            .events
            .into_iter()
            .map(|value| value.kind.to_string())
            .collect(),
        queries: manifest
            .queries
            .into_iter()
            .map(|value| value.name.to_string())
            .collect(),
        resources: manifest
            .resources
            .into_iter()
            .map(resource_method)
            .collect(),
        grant_resources: manifest
            .grant_resources
            .into_iter()
            .map(grant_resource)
            .collect(),
        subscriptions: manifest
            .subscriptions
            .into_iter()
            .map(|value| value.kind.to_string())
            .collect(),
    })
}

fn resource_method(method: ResourceMethod) -> OwnedResourceMethod {
    OwnedResourceMethod {
        name: method.name().to_string(),
        kind: method.kind().to_string(),
        params: method
            .params()
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    }
}

fn grant_resource(spec: GrantResourceSpec) -> OwnedGrantResourceSpec {
    OwnedGrantResourceSpec {
        namespace: spec.namespace.to_string(),
        selector_schema_id: spec.selector_schema_id.to_string(),
        selector_schema_json: spec.selector_schema_json.to_string(),
        verbs: spec
            .verbs
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        summary: spec.summary.to_string(),
    }
}

fn activation_mode(namespace: &str) -> ActivationMode {
    if matches!(
        namespace,
        "automation" | "job" | "scheduler" | "stream" | "webhook"
    ) {
        ActivationMode::Background
    } else {
        ActivationMode::Demand
    }
}

fn dynamic_dependencies(namespace: &str) -> &'static [&'static str] {
    match namespace {
        "automation" => &["query"],
        "harness" => &["builder"],
        "js-runtime" => &["web-publish"],
        "local-model" => &["model"],
        "migration" => &["js-runtime"],
        "query" => &["relational-db"],
        "stream" | "webhook" => &["net"],
        _ => &[],
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_fundamental_capability_has_a_valid_manifest() {
        let manifests = manifests(&"0".repeat(64), "macos", "aarch64").unwrap();
        assert_eq!(manifests.len(), DYNAMIC_CAPABILITY_COUNT);
        for fundamental in terrane_cap_protocol::FUNDAMENTAL_CAPABILITIES {
            assert!(!manifests.contains_key(fundamental));
        }
        assert_eq!(manifests["automation"].dependencies, vec!["query"]);
        assert_eq!(manifests["webhook"].activation, ActivationMode::Background);
    }
}
