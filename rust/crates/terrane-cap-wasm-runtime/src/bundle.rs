use std::path::Path;

use nanoserde::DeJson;
use terrane_cap_interface::{non_empty_or, Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmRuntimeBundle {
    pub module: Vec<u8>,
    pub name: String,
    pub entry: String,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, DeJson)]
pub struct BundleManifest {
    #[nserde(default)]
    pub id: String,
    #[nserde(default)]
    pub name: String,
    #[nserde(default)]
    pub runtime: String,
    #[nserde(default)]
    pub module: String,
    #[nserde(default)]
    pub entry: String,
    #[nserde(default)]
    pub ui: String,
    #[nserde(default)]
    pub resources: Vec<String>,
}

pub fn read_manifest(bundle_dir: &Path) -> Result<BundleManifest> {
    let text = std::fs::read_to_string(bundle_dir.join("manifest.json"))
        .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
    BundleManifest::deserialize_json(&text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))
}

pub(crate) fn load_bundle(source: &str) -> Result<WasmRuntimeBundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = read_manifest(path)?;
        if manifest.runtime != "wasm" {
            return Err(Error::Runtime(format!(
                "manifest runtime {:?} is not wasm",
                manifest.runtime
            )));
        }
        if manifest.module.trim().is_empty() {
            return Err(Error::Runtime(
                "manifest.module is required for wasm".into(),
            ));
        }
        let module_path = path.join(&manifest.module);
        let module = std::fs::read(&module_path)
            .map_err(|e| Error::Runtime(format!("read module {}: {e}", module_path.display())))?;
        Ok(WasmRuntimeBundle {
            module,
            name: manifest.name,
            entry: non_empty_or(manifest.entry, "handle"),
            resources: manifest.resources,
        })
    } else {
        let module = std::fs::read(path)
            .map_err(|e| Error::Runtime(format!("read module {}: {e}", path.display())))?;
        Ok(WasmRuntimeBundle {
            module,
            name: String::new(),
            entry: "handle".to_string(),
            resources: vec!["kv".to_string()],
        })
    }
}
