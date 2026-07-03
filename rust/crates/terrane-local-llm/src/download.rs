//! Streaming downloads — write to a `.part` file, then rename atomically so a
//! recorded path never points at a torn download. Used for model weights
//! (Hugging Face) and runtime bootstrap artifacts (uv).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::LlmError;

const CHUNK: usize = 128 * 1024;

/// Download `https://huggingface.co/<repo>/resolve/main/<file>` into
/// the Hugging Face hub cache when available, falling back to `dest_dir`.
/// Redirects (HF resolves to a CDN) are followed by ureq.
pub fn download_model(
    repo: &str,
    file: &str,
    dest_dir: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(PathBuf, u64), LlmError> {
    if let Some(path) = cached_hf_model_file(repo, file) {
        let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        return Ok((path, size));
    }
    let dest_dir = hf_snapshot_download_dir(repo).unwrap_or_else(|| dest_dir.to_path_buf());
    let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
    download_url(&url, &dest_dir, file, on_progress)
}

/// Resolve a file that already exists in the normal Hugging Face hub cache.
///
/// Honors `HF_HUB_CACHE` / `HUGGINGFACE_HUB_CACHE`, then `HF_HOME`, then the
/// default `~/.cache/huggingface/hub` shape used by `huggingface_hub`.
pub fn cached_hf_model_file(repo: &str, file: &str) -> Option<PathBuf> {
    let snapshots = huggingface_hub_cache_dir()?
        .join(hf_repo_cache_name(repo))
        .join("snapshots");
    let entries = fs::read_dir(snapshots).ok()?;
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path().join(file);
            if !path.is_file() {
                return None;
            }
            let modified = fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Some((modified, path))
        })
        .collect();
    candidates.sort_by_key(|(modified, _)| *modified);
    candidates.pop().map(|(_, path)| path)
}

fn hf_snapshot_download_dir(repo: &str) -> Option<PathBuf> {
    Some(
        huggingface_hub_cache_dir()?
            .join(hf_repo_cache_name(repo))
            .join("snapshots")
            .join("terrane"),
    )
}

fn hf_repo_cache_name(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}

fn huggingface_hub_cache_dir() -> Option<PathBuf> {
    nonempty_env_path("HF_HUB_CACHE")
        .or_else(|| nonempty_env_path("HUGGINGFACE_HUB_CACHE"))
        .or_else(|| nonempty_env_path("HF_HOME").map(|home| home.join("hub")))
        .or_else(|| {
            nonempty_env_path("XDG_CACHE_HOME").map(|cache| cache.join("huggingface").join("hub"))
        })
        .or_else(|| nonempty_env_path("HOME").map(|home| home.join(".cache/huggingface/hub")))
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Download `url` into `dest_dir/<file_name>`. `on_progress(written, total)`
/// fires per chunk; `total` is known when the server sends a content length.
/// Returns the final path and byte size.
pub fn download_url(
    url: &str,
    dest_dir: &Path,
    file_name: &str,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(PathBuf, u64), LlmError> {
    fs::create_dir_all(dest_dir)
        .map_err(|e| LlmError::Download(format!("cannot create {}: {e}", dest_dir.display())))?;
    let dest = dest_dir.join(file_name);
    let part = dest_dir.join(format!("{file_name}.part"));

    // ureq's default agent connects with a 30 s timeout and follows redirects.
    let response = ureq::get(url).call().map_err(|e| match e {
        ureq::Error::Status(status, _) => {
            LlmError::Download(format!("{url} returned HTTP {status}"))
        }
        other => LlmError::Download(format!("{url}: {other}")),
    })?;
    let total = response
        .header("content-length")
        .and_then(|raw| raw.parse::<u64>().ok());

    let mut reader = response.into_reader();
    let mut out = fs::File::create(&part)
        .map_err(|e| LlmError::Download(format!("cannot create {}: {e}", part.display())))?;
    let mut written: u64 = 0;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| LlmError::Download(format!("read from {url} failed: {e}")))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read])
            .map_err(|e| LlmError::Download(format!("write to {} failed: {e}", part.display())))?;
        written += read as u64;
        on_progress(written, total);
    }
    out.sync_all()
        .map_err(|e| LlmError::Download(format!("sync {} failed: {e}", part.display())))?;
    drop(out);

    if let Some(total) = total {
        if written != total {
            let _ = fs::remove_file(&part);
            return Err(LlmError::Download(format!(
                "{url} truncated: got {written} of {total} bytes"
            )));
        }
    }
    fs::rename(&part, &dest)
        .map_err(|e| LlmError::Download(format!("finalize {} failed: {e}", dest.display())))?;
    Ok((dest, written))
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn cached_hf_model_file_resolves_common_hub_snapshot() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old = std::env::var_os("HF_HUB_CACHE");
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HF_HUB_CACHE", dir.path());

        let file = dir
            .path()
            .join("models--org--repo")
            .join("snapshots")
            .join("abc123")
            .join("model.gguf");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"gguf").unwrap();

        assert_eq!(
            cached_hf_model_file("org/repo", "model.gguf").as_deref(),
            Some(file.as_path())
        );

        restore_env("HF_HUB_CACHE", old);
    }

    #[test]
    fn download_model_reuses_cached_hf_file_without_network() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old = std::env::var_os("HF_HUB_CACHE");
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HF_HUB_CACHE", dir.path());

        let file = dir
            .path()
            .join("models--org--repo")
            .join("snapshots")
            .join("abc123")
            .join("model.gguf");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"cached").unwrap();

        let mut progress_called = false;
        let (path, size) = download_model(
            "org/repo",
            "model.gguf",
            dir.path().join("fallback").as_path(),
            &mut |_, _| progress_called = true,
        )
        .unwrap();
        assert_eq!(path, file);
        assert_eq!(size, 6);
        assert!(!progress_called);

        restore_env("HF_HUB_CACHE", old);
    }

    fn restore_env(name: &str, old: Option<std::ffi::OsString>) {
        match old {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}
