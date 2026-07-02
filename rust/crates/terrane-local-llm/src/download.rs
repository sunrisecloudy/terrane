//! Streaming downloads — write to a `.part` file, then rename atomically so a
//! recorded path never points at a torn download. Used for model weights
//! (Hugging Face) and runtime bootstrap artifacts (uv).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::LlmError;

const CHUNK: usize = 128 * 1024;

/// Download `https://huggingface.co/<repo>/resolve/main/<file>` into
/// `dest_dir/<file>`. Redirects (HF resolves to a CDN) are followed by ureq.
pub fn download_model(
    repo: &str,
    file: &str,
    dest_dir: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(PathBuf, u64), LlmError> {
    let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
    download_url(&url, dest_dir, file, on_progress)
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
