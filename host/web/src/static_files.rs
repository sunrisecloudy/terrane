use std::path::{Path, PathBuf};

/// Canonicalize `target` and confirm it stays within `base` — rejects `..`,
/// absolute escapes, and symlink escapes. `None` if outside or missing.
pub fn safe_within(base: &Path, target: &Path) -> Option<PathBuf> {
    let base = std::fs::canonicalize(base).ok()?;
    let target = std::fs::canonicalize(target).ok()?;
    target.starts_with(&base).then_some(target)
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}
