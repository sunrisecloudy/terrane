use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::UNIX_EPOCH;

use nanoserde::SerJson;

use crate::http::{json_error, json_ok, Resp};

#[derive(Clone, Debug, SerJson)]
struct LiveVersionResponse {
    version: String,
}

/// `GET /apps/{id}/__terrane/live-version` — a dev helper used by the injected
/// browser shim to notice source-bundle changes and reload the page.
pub fn response(core: &mut terrane_host::HostCore, id: &str) -> Resp {
    let Some(source) = core.state().app.apps.get(id).and_then(|a| a.source.clone()) else {
        return json_error(404, &format!("no such app (or no bundle): {id}"));
    };
    match source_version(Path::new(&source)) {
        Ok(version) => json_ok(&LiveVersionResponse { version }),
        Err(e) => json_error(500, &e),
    }
}

fn source_version(source: &Path) -> Result<String, String> {
    let source = std::fs::canonicalize(source)
        .map_err(|e| format!("resolve app source {}: {e}", source.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_source_path(&source, &source, &mut hasher)?;
    Ok(format!("{:016x}", hasher.finish()))
}

fn hash_source_path(
    root: &Path,
    path: &Path,
    hasher: &mut std::collections::hash_map::DefaultHasher,
) -> Result<(), String> {
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("read metadata {}: {e}", path.display()))?;
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().hash(hasher);
    metadata.is_dir().hash(hasher);
    metadata.len().hash(hasher);
    if let Ok(modified) = metadata.modified() {
        if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
            duration.as_nanos().hash(hasher);
        }
    }

    if metadata.is_dir() {
        let mut entries = std::fs::read_dir(path)
            .map_err(|e| format!("read directory {}: {e}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read directory entry {}: {e}", path.display()))?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            hash_source_path(root, &entry.path(), hasher)?;
        }
    }

    Ok(())
}
