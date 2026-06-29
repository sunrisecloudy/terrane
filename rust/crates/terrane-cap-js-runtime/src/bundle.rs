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
}

/// Load the bundle for an app whose `source` is either the bundle directory
/// (containing manifest.json + the backend file) or a direct `.js` path.
pub(crate) fn load_bundle(source: &str) -> Result<JsRuntimeBundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = read_manifest(path)?;
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

/// Read and parse `<bundle_dir>/manifest.json`.
pub fn read_manifest(bundle_dir: &Path) -> Result<BundleManifest> {
    let text = std::fs::read_to_string(bundle_dir.join("manifest.json"))
        .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
    BundleManifest::deserialize_json(&text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))
}
