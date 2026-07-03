//! Edge importer that walks checked-in i18n catalogs and seeds the public KV
//! bucket deterministically.
//!
//! This is the effectful host side: it reads the filesystem, parses catalog
//! JSON, and emits one trusted-host `kv.public.import` commit. The core
//! (`kv.public.import`) is pure and replay-safe; this module only assembles the
//! sorted payload and dispatches it.
//!
//! Catalog layout (each file is a flat `{ "<key>": "<value>" }` map):
//! ```text
//! i18n/system/<code>.json        # domain "system": host/shell chrome strings
//! apps/<id>/i18n/<code>.json     # domain <id>: per-app strings
//! ```
//! File `i18n/system/es.json` key `menu.file` becomes public key
//! `i18n/es/system.menu.file`; `apps/todo/i18n/es.json` key `added` becomes
//! `i18n/es/todo.added`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use terrane_cap_kv::PUBLIC_BUCKET_APP_ID;

use crate::{dispatch_on_core, HostCore};

// Re-export the pure negotiation surface so thin host adapters (web/CLI) reach
// it through terrane_host::i18n without depending on terrane-i18n directly.
pub use terrane_i18n::{
    canonical, dir_for, from_accept_language, from_preferred_list, is_supported, DEFAULT, SUPPORTED,
};

/// A catalog entry discovered during a walk: its source path, the domain
/// (`system` or an app id), and the language code stem.
#[derive(Debug, Clone)]
struct CatalogFile {
    path: PathBuf,
    domain: String,
    code_stem: String,
}

/// The outcome of an [`import_i18n_dir`] run: how many strings were seeded,
/// across how many languages and domains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I18nImportOutcome {
    pub entries: usize,
    pub languages: usize,
    pub domains: usize,
}

impl I18nImportOutcome {
    pub fn message(&self) -> String {
        format!(
            "imported {} i18n strings across {} languages and {} domains",
            self.entries, self.languages, self.domains
        )
    }
}

/// Walk `i18n/system/*.json` and `apps/*/i18n/*.json` under `root` (in sorted
/// path order), validate every language code against the supported set, and
/// commit the merged, sorted catalog into the public KV bucket via one
/// `kv.public.import`. Idempotent: re-importing identical content yields
/// identical events.
pub fn import_i18n_dir(core: &mut HostCore, root: &Path) -> Result<I18nImportOutcome, String> {
    let mut files = discover_catalogs(root)?;
    // Deterministic processing order: sort by path so duplicate-key overwrite
    // precedence is stable. The final emit order is fixed by BTreeMap key order.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    let mut languages = std::collections::BTreeSet::new();
    let mut domains = std::collections::BTreeSet::new();

    for file in &files {
        let text = std::fs::read_to_string(&file.path)
            .map_err(|e| format!("{}: {e}", file.path.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.path.display()))?;
        let obj = value.as_object().ok_or_else(|| {
            format!("{}: expected a JSON object", file.path.display())
        })?;

        let code = terrane_i18n::canonical(&file.code_stem).ok_or_else(|| {
            format!(
                "{}: unsupported language code {:?}; supported codes are {:?}",
                file.path.display(),
                file.code_stem,
                terrane_i18n::SUPPORTED
            )
        })?;

        domains.insert(file.domain.clone());
        languages.insert(code);

        for (key, val) in obj {
            let value = val.as_str().ok_or_else(|| {
                format!(
                    "{}: value for key {:?} must be a string",
                    file.path.display(),
                    key
                )
            })?;
            // i18n/<code>/<domain>.<key> — i18n keys are deliberately non-reserved.
            let public_key = format!("i18n/{code}/{domain}.{key}", domain = file.domain);
            merged.insert(public_key, value.to_string());
        }
    }

    let payload = serde_json::to_string(&merged)
        .map_err(|e| format!("encode i18n payload: {e}"))?;

    dispatch_on_core(core, "kv.public.import", &[payload])?;

    Ok(I18nImportOutcome {
        entries: merged.len(),
        languages: languages.len(),
        domains: domains.len(),
    })
}

/// Discover every catalog file under `root` for the two known layouts. Missing
/// layout directories are skipped; an empty result is an error so typos in the
/// path surface immediately.
fn discover_catalogs(root: &Path) -> Result<Vec<CatalogFile>, String> {
    let mut out = Vec::new();
    collect_domain_dir(root, &root.join("i18n").join("system"), "system", &mut out)?;
    let apps_dir = root.join("apps");
    if apps_dir.is_dir() {
        for app_entry in std::fs::read_dir(&apps_dir)
            .map_err(|e| format!("read {}: {e}", apps_dir.display()))?
        {
            let app_entry = app_entry.map_err(|e| format!("read apps dir: {e}"))?;
            if !app_entry
                .file_type()
                .map_err(|e| format!("read apps entry: {e}"))?
                .is_dir()
            {
                continue;
            }
            let app_id = app_entry.file_name();
            let app_id = app_id.to_str().ok_or_else(|| {
                format!("app id is not valid UTF-8: {}", app_entry.path().display())
            })?;
            collect_domain_dir(root, &app_entry.path().join("i18n"), app_id, &mut out)?;
        }
    }
    if out.is_empty() {
        return Err(format!(
            "no i18n catalogs found under {} (expected i18n/system/*.json and/or apps/*/i18n/*.json)",
            root.display()
        ));
    }
    Ok(out)
}

/// Collect `<dir>/<code>.json` files as catalog entries for one `domain`.
fn collect_domain_dir(
    root: &Path,
    dir: &Path,
    domain: &str,
    out: &mut Vec<CatalogFile>,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let path = entry.path();
        if !is_json_file(&path) {
            continue;
        }
        let code_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("language code is not valid UTF-8: {}", path.display()))?
            .to_string();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        out.push(CatalogFile {
            path: path.clone(),
            domain: domain.to_string(),
            code_stem,
        });
        // `rel` is computed for debugging but the sort uses `path` directly;
        // keep it referenced so future diagnostics stay anchored here.
        let _ = rel;
    }
    Ok(())
}

fn is_json_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json") && path.is_file()
}

/// Build the message bundle a host pushes to a UI for locale `code` across the
/// given `domains` (e.g. `["system"]` for shell chrome, `["system", "todo"]`
/// for the todo frame). English is laid down first as the fallback, then `code`
/// overlays it, so any key missing a `code` translation keeps its `en` value.
/// Keys are `<domain>.<key>` — the public `i18n/<code>/` prefix stripped — so a
/// UI looks up `t("todo.add")` / `t("system.action.add")`. Returns an empty map
/// when the public bucket is unseeded.
pub fn bundle(core: &HostCore, code: &str, domains: &[&str]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(public) = core.state().kv.data.get(PUBLIC_BUCKET_APP_ID) else {
        return out;
    };
    // `en` first (fallback), then the requested code so it wins on collisions.
    for lang in dedup_langs(code) {
        let prefix = format!("i18n/{lang}/");
        for (key, value) in public {
            let Some(short) = key.strip_prefix(&prefix) else {
                continue;
            };
            let Some((domain, _)) = short.split_once('.') else {
                continue;
            };
            if domains.contains(&domain) {
                out.insert(short.to_string(), value.clone());
            }
        }
    }
    out
}

/// `en` then `code`, dropping the duplicate when `code` is already `en`.
fn dedup_langs(code: &str) -> Vec<&str> {
    if code == terrane_i18n::DEFAULT {
        vec![terrane_i18n::DEFAULT]
    } else {
        vec![terrane_i18n::DEFAULT, code]
    }
}

/// The shell-chrome bundle (the `system` domain) for `code`. Backs the web
/// shell's own strings and the macOS native chrome.
pub fn system_bundle(core: &HostCore, code: &str) -> BTreeMap<String, String> {
    bundle(core, code, &["system"])
}

/// The bundle pushed to an app frame: the shared `system` domain (so apps can
/// reuse common words) plus the app's own domain.
pub fn app_bundle(core: &HostCore, code: &str, app_id: &str) -> BTreeMap<String, String> {
    bundle(core, code, &["system", app_id])
}
