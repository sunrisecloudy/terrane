use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use terrane_cap_interface::{GrantResourceSpec, ResourceMethod};
use terrane_cap_protocol::{
    is_fundamental, sha256_file, ActivationMode, BundleManifest, CapabilityDeclaration,
    CapabilityIndex, CapabilityIndexArtifact, OwnedGrantResourceSpec, OwnedResourceMethod,
    BUNDLE_FORMAT_VERSION, INDEX_FORMAT_VERSION, PROTOCOL_VERSION,
};
use terrane_core::{default_registry, Registry};

pub const DYNAMIC_CAPABILITY_COUNT: usize = 42;

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
    workers: &Path,
    output: &Path,
    signing_key: &SigningKey,
    platform: &str,
    architecture: &str,
) -> Result<Vec<PathBuf>, String> {
    package_all_with_index(workers, output, signing_key, platform, architecture, None)
}

pub fn package_all_with_index(
    workers: &Path,
    output: &Path,
    signing_key: &SigningKey,
    platform: &str,
    architecture: &str,
    download_base_url: Option<String>,
) -> Result<Vec<PathBuf>, String> {
    if !workers.is_file() && !workers.is_dir() {
        return Err(format!(
            "worker binary directory does not exist: {}",
            workers.display()
        ));
    }
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let mut packaged = Vec::new();
    let mut artifacts = BTreeMap::new();
    let registry = default_registry();
    for namespace in registry.namespaces() {
        if is_fundamental(namespace) {
            continue;
        }
        let legacy_dir = output.join(namespace);
        if legacy_dir.is_dir() {
            fs::remove_dir_all(&legacy_dir).map_err(|error| error.to_string())?;
        }
        let worker = if workers.is_file() {
            workers.to_path_buf()
        } else {
            workers.join(namespace)
        };
        if !worker.is_file() {
            return Err(format!(
                "worker binary for {namespace} does not exist: {}",
                worker.display()
            ));
        }
        let executable_sha256 = sha256_file(&worker).map_err(|error| error.to_string())?;
        let mut manifest = manifest_for(
            &registry,
            namespace,
            &executable_sha256,
            platform,
            architecture,
        )?;
        manifest.signature = hex(&signing_key
            .sign(
                &manifest
                    .signing_payload()
                    .map_err(|error| error.to_string())?,
            )
            .to_bytes());
        let dir = output.join(format!(".{namespace}.bundle.tmp"));
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let executable = dir.join(&manifest.executable);
        fs::hard_link(&worker, &executable)
            .or_else(|_| fs::copy(&worker, &executable).map(|_| ()))
            .map_err(|error| error.to_string())?;
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let archive_name = format!("{namespace}-{}.tcap", manifest.version);
        let archive = output.join(&archive_name);
        let temporary_archive = output.join(format!(".{archive_name}.tmp"));
        write_archive(&dir, &temporary_archive, &manifest.executable)?;
        if archive.exists() {
            fs::remove_file(&archive).map_err(|error| error.to_string())?;
        }
        fs::rename(&temporary_archive, &archive).map_err(|error| error.to_string())?;
        fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
        let archive_sha256 = sha256_file(&archive).map_err(|error| error.to_string())?;
        artifacts.insert(
            namespace.to_string(),
            CapabilityIndexArtifact {
                archive: archive_name,
                archive_sha256,
                manifest,
            },
        );
        packaged.push(archive);
    }
    let mut index = CapabilityIndex {
        format_version: INDEX_FORMAT_VERSION,
        download_base_url,
        artifacts,
        signature: String::new(),
    };
    index.signature = hex(&signing_key
        .sign(&index.signing_payload().map_err(|error| error.to_string())?)
        .to_bytes());
    let temporary_index = output.join(".index.json.tmp");
    fs::write(
        &temporary_index,
        serde_json::to_vec_pretty(&index).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(temporary_index, output.join("index.json")).map_err(|error| error.to_string())?;
    Ok(packaged)
}

fn write_archive(bundle: &Path, target: &Path, executable: &str) -> Result<(), String> {
    let file = fs::File::create(target).map_err(|error| error.to_string())?;
    let encoder = zstd::Encoder::new(file, 3).map_err(|error| error.to_string())?;
    let mut archive = tar::Builder::new(encoder);
    append_regular_file(
        &mut archive,
        &bundle.join("manifest.json"),
        "manifest.json",
        0o644,
    )?;
    append_regular_file(&mut archive, &bundle.join(executable), executable, 0o755)?;
    let encoder = archive.into_inner().map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())?;
    Ok(())
}

fn append_regular_file<W: Write>(
    archive: &mut tar::Builder<W>,
    source: &Path,
    archive_path: &str,
    mode: u32,
) -> Result<(), String> {
    let mut file = fs::File::open(source).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    archive
        .append_data(&mut header, archive_path, &mut file)
        .map_err(|error| error.to_string())
}

fn manifest_for(
    registry: &Registry,
    namespace: &str,
    executable_sha256: &str,
    platform: &str,
    architecture: &str,
) -> Result<BundleManifest, String> {
    let capability = registry.get(namespace).map_err(|error| error.to_string())?;
    let manifest = BundleManifest {
        format_version: BUNDLE_FORMAT_VERSION,
        namespace: namespace.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION,
        state_schema_version: 1,
        platform: platform.to_string(),
        architecture: architecture.to_string(),
        executable: format!("terrane-cap-{namespace}-worker"),
        executable_sha256: executable_sha256.to_string(),
        signature: String::new(),
        dependencies: dynamic_dependencies(namespace)
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        activation: activation_mode(namespace),
        declaration: declaration(registry, namespace)?,
    };
    capability
        .doc(false)
        .namespace
        .eq(namespace)
        .then_some(())
        .ok_or_else(|| format!("capability documentation namespace mismatch for {namespace}"))?;
    manifest.validate().map_err(|error| error.to_string())?;
    Ok(manifest)
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
        delegated_events: delegated_events(namespace)
            .iter()
            .map(|value| (*value).to_string())
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
        "query" => &["relational_db"],
        "stream" | "webhook" => &["net"],
        _ => &[],
    }
}

fn delegated_events(namespace: &str) -> &'static [&'static str] {
    match namespace {
        "relational_db" | "search" => &["kv.set", "kv.deleted"],
        _ => &[],
    }
}

pub fn hex(bytes: &[u8]) -> String {
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

    #[test]
    fn capability_archives_contain_only_portable_regular_files() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("bundle");
        fs::create_dir(&bundle).unwrap();
        fs::write(bundle.join("manifest.json"), b"{}").unwrap();
        let original_worker = root.path().join("worker-source");
        fs::write(&original_worker, b"worker").unwrap();
        fs::hard_link(&original_worker, bundle.join("terrane-cap-time-worker")).unwrap();

        let target = root.path().join("time.tcap");
        write_archive(&bundle, &target, "terrane-cap-time-worker").unwrap();

        let decoder = zstd::Decoder::new(fs::File::open(target).unwrap()).unwrap();
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert!(entry.header().entry_type().is_file());
                (
                    entry.path().unwrap().into_owned(),
                    entry.header().mode().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![
                (PathBuf::from("manifest.json"), 0o644),
                (PathBuf::from("terrane-cap-time-worker"), 0o755),
            ]
        );
    }
}
