use std::path::{Path, PathBuf};

use nanoserde::DeJson;
use terrane_api::AppSummary;

/// Dev-mode app discovery (`--apps <dir>`): the folder is re-scanned on every
/// catalog request, so dropping a bundle in (or editing a manifest) shows up
/// on the next `/apps` fetch without an explicit `app add`. Dev apps are
/// auto-cataloged on their first invoke, mirroring how the macOS host
/// catalogs an app lazily on selection.
pub struct DevApps {
    dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct DevApp {
    pub id: String,
    pub name: String,
    pub source: String,
}

#[derive(DeJson)]
struct DevManifest {
    #[nserde(default)]
    id: String,
    #[nserde(default)]
    name: String,
}

impl DevApps {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    pub fn dir_display(&self) -> String {
        self.dir
            .as_deref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default()
    }

    pub fn scan(&self) -> Vec<DevApp> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut apps: Vec<DevApp> = entries
            .flatten()
            .filter_map(|entry| read_dev_app(&entry.path()))
            .collect();
        apps.sort_by(|a, b| a.id.cmp(&b.id));
        apps.dedup_by(|a, b| a.id == b.id);
        apps
    }

    pub fn find(&self, id: &str) -> Option<DevApp> {
        self.scan().into_iter().find(|app| app.id == id)
    }

    pub fn summaries(&self) -> Vec<AppSummary> {
        self.scan()
            .into_iter()
            .map(|app| AppSummary {
                has_ui: terrane_host::app_has_ui(Some(&app.source)),
                id: app.id,
                name: app.name,
            })
            .collect()
    }
}

fn read_dev_app(path: &Path) -> Option<DevApp> {
    if !path.is_dir() {
        return None;
    }
    let manifest = std::fs::read_to_string(path.join("manifest.json")).ok()?;
    let manifest: DevManifest = DeJson::deserialize_json(&manifest).ok()?;
    let id = manifest.id.trim().to_string();
    if !safe_app_id(&id) {
        return None;
    }
    let name = match manifest.name.trim() {
        "" => id.clone(),
        name => name.to_string(),
    };
    Some(DevApp {
        id,
        name,
        source: path.to_string_lossy().into_owned(),
    })
}

/// A dev app id becomes a URL path segment and a catalog key; only accept a
/// single, plainly-named segment.
fn safe_app_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
