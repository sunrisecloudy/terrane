use std::collections::BTreeMap;
use std::path::Path;

use nanoserde::DeJson;
use terrane_cap_interface::{Error, Result};

/// A memory-backed JS backend bundle: source, display name, and granted
/// resource namespaces. Preview and tests use this without disk I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsRuntimeBundle {
    pub source: String,
    pub name: String,
    pub resources: Vec<String>,
}

/// The fields of `manifest.json` terrane reads. Public so the CLI (`app
/// install`) and the edge hosts can read a bundle's catalog metadata (id/name/ui)
/// without re-implementing the parse.
#[derive(Debug, Clone, DeJson)]
pub struct BundleManifest {
    /// Stable app id (matches the catalog entry). Empty if the manifest omits it.
    #[nserde(default)]
    pub id: String,
    /// Display name.
    #[nserde(default)]
    pub name: String,
    /// The backend JS file, e.g. `"main.js"`.
    pub backend: String,
    /// Runtime engine. Empty means JS for source-only developer use.
    #[nserde(default)]
    pub runtime: String,
    /// The UI entry file (e.g. `"index.html"`); empty for CLI-only apps.
    #[nserde(default)]
    pub ui: String,
    /// Resource namespaces the backend may reach (least privilege; empty default).
    #[nserde(default)]
    pub resources: Vec<String>,
    /// Common API interfaces this app advertises.
    #[nserde(default)]
    pub interfaces: Vec<String>,
    /// App data version expected by this bundle. Empty/omitted manifests default
    /// to version 1.
    #[nserde(default, rename = "dataVersion")]
    pub data_version: u64,
    /// Forward migration scripts, sorted by target version.
    #[nserde(default)]
    pub migrations: Vec<MigrationSpec>,
}

#[derive(Debug, Clone, DeJson)]
pub struct MigrationSpec {
    #[nserde(default)]
    pub to: u64,
    #[nserde(default)]
    pub script: String,
}

/// Load the bundle for an app whose `source` is either the bundle directory
/// (containing manifest.json + the backend file) or a direct `.js` path.
pub(crate) fn load_bundle(source: &str) -> Result<JsRuntimeBundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = read_manifest(path)?;
        validate_manifest_migrations(&manifest, Some(path))?;
        if !manifest.runtime.is_empty() && manifest.runtime != "js" {
            return Err(Error::Runtime(format!(
                "manifest runtime {:?} is not js",
                manifest.runtime
            )));
        }
        let js_path = path.join(&manifest.backend);
        let source = std::fs::read_to_string(&js_path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", js_path.display())))?;
        Ok(JsRuntimeBundle {
            source,
            name: manifest.name,
            resources: manifest.resources,
        })
    } else {
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", path.display())))?;
        Ok(JsRuntimeBundle {
            source,
            name: String::new(),
            resources: vec!["kv".to_string()],
        })
    }
}

pub fn bundle_from_files(files: &BTreeMap<String, String>) -> Result<JsRuntimeBundle> {
    let manifest = read_manifest_from_files(files)?;
    if !manifest.runtime.is_empty() && manifest.runtime != "js" {
        return Err(Error::Runtime(format!(
            "manifest runtime {:?} is not js",
            manifest.runtime
        )));
    }
    let source = files
        .get(&manifest.backend)
        .ok_or_else(|| {
            Error::Runtime(format!(
                "kv app bundle is missing backend file {}",
                manifest.backend
            ))
        })?
        .clone();
    Ok(JsRuntimeBundle {
        source,
        name: manifest.name,
        resources: manifest.resources,
    })
}

pub fn read_manifest_from_files(files: &BTreeMap<String, String>) -> Result<BundleManifest> {
    let manifest_text = files
        .get("manifest.json")
        .ok_or_else(|| Error::Runtime("kv app bundle is missing manifest.json".into()))?;
    let manifest = BundleManifest::deserialize_json(manifest_text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))?;
    validate_manifest_migrations(&manifest, None)?;
    Ok(manifest)
}

pub(crate) fn load_bundle_files(files: &BTreeMap<String, String>) -> Result<JsRuntimeBundle> {
    bundle_from_files(files)
}

/// Read and parse `<bundle_dir>/manifest.json`.
pub fn read_manifest(bundle_dir: &Path) -> Result<BundleManifest> {
    let text = std::fs::read_to_string(bundle_dir.join("manifest.json"))
        .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
    let manifest = BundleManifest::deserialize_json(&text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))?;
    validate_manifest_migrations(&manifest, Some(bundle_dir))?;
    Ok(manifest)
}

pub fn manifest_data_version(manifest: &BundleManifest) -> u64 {
    if manifest.data_version == 0 {
        1
    } else {
        manifest.data_version
    }
}

pub fn validate_manifest_migrations(
    manifest: &BundleManifest,
    bundle_dir: Option<&Path>,
) -> Result<()> {
    let data_version = manifest_data_version(manifest);
    if data_version == 1 && manifest.migrations.is_empty() {
        return Ok(());
    }
    if data_version < 1 {
        return Err(Error::InvalidInput(
            "manifest dataVersion must be at least 1".into(),
        ));
    }
    let mut expected = 2u64;
    for step in &manifest.migrations {
        if step.to != expected {
            return Err(Error::InvalidInput(format!(
                "manifest migrations must be consecutive: expected to={expected}, got {}",
                step.to
            )));
        }
        if step.script.trim().is_empty() {
            return Err(Error::InvalidInput(
                "manifest migration script must not be empty".into(),
            ));
        }
        if let Some(dir) = bundle_dir {
            let script_path = dir.join(&step.script);
            if !script_path.is_file() {
                return Err(Error::InvalidInput(format!(
                    "manifest migration script does not exist: {}",
                    step.script
                )));
            }
        }
        expected += 1;
    }
    if expected - 1 != data_version {
        return Err(Error::InvalidInput(format!(
            "manifest migrations end at {}, but dataVersion is {data_version}",
            expected - 1
        )));
    }
    Ok(())
}
