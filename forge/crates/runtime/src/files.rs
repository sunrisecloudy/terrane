//! `ctx.files` host-call types, the sandbox-confinement gate, and the injectable
//! filesystem seam.
//!
//! prd-merged/01 CR-3 (`files` namespace), **CR-8** (deterministic mode: a
//! recorded read replays its recorded bytes — the live filesystem is never
//! consulted on replay); prd-merged/07 SC-8 (capability grammar), SC-10/SC-12
//! (sandboxed, user-granted handles). Source of record: `forge/spec/files.md`
//! and the `forge/fixtures/files/*` validation vectors.
//!
//! ## Model (mirrors `net.rs`)
//!
//! `ctx.files` exposes file reads/writes only through user-granted **handles**. A
//! handle is a stable logical id; the trusted host policy maps it to a *per-applet
//! sandbox root* (an absolute native path the manifest never names). The runtime
//! ([`HostContext`](crate::HostContext)):
//!   1. **capability-checks** the op against the manifest's `files.<read|write>`
//!      grant for the handle — every op is gated *before* any filesystem touch;
//!   2. **confines** the request path to the handle's sandbox root — a `..`
//!      traversal, an absolute path, a URI/drive/NUL path, or a symlink whose
//!      target escapes the root is rejected with `PermissionDenied`;
//!   3. **records / replays** the call so a recorded read serves its recorded
//!      bytes byte-identically offline (the live filesystem is never opened on
//!      replay), exactly like `net.fetch`.
//!
//! The *actual* filesystem is hidden behind the injectable [`FileSystem`] trait so
//! this crate stays wasm-clean and CI never touches a real disk: tests/CI/the demo
//! inject an [`InMemoryFileSystem`]; the one real-I/O implementation lives
//! host-side (forge-core / a shell), out of this crate's scope.

use forge_domain::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// A `ctx.files.read(request)` request as the applet builds it (and the recorder
/// captures into the trace). `handle` + `path` name the resource; `encoding`
/// starts as `base64` only (spec/files.md) so the recorded bytes are byte-exact
/// and engine-independent. A plain serde struct so it round-trips through the JS
/// boundary and the [`RecordedCall`](forge_domain::RecordedCall) trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileReadRequest {
    /// The user-granted handle the read targets, e.g. `workspace_data`.
    pub handle: String,
    /// The relative POSIX path inside the handle, e.g. `data/settings.json`.
    pub path: String,
    /// Byte encoding of the response. `base64` only in M0a (spec/files.md).
    #[serde(default = "default_encoding")]
    pub encoding: String,
}

fn default_encoding() -> String {
    "base64".to_string()
}

/// A `ctx.files.read` response: the file's bytes (base64) plus its size and
/// content-type. Served byte-identically on replay from the recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileReadResponse {
    /// The normalized relative path the bytes came from.
    pub path: String,
    /// The file's bytes, base64-encoded (engine-independent, byte-exact).
    pub bytes_base64: String,
    /// The file's size in bytes.
    pub size: u64,
    /// The file's content-type, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// A `ctx.files.write(request)` request. `handle` + `path` name the resource;
/// `bytes_base64` is the payload; `mode` is the write semantics (`create_or_truncate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileWriteRequest {
    /// The user-granted handle the write targets.
    pub handle: String,
    /// The relative POSIX path inside the handle.
    pub path: String,
    /// The payload bytes, base64-encoded.
    pub bytes_base64: String,
    /// The payload's content-type, if declared (checked against the grant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Write mode. `create_or_truncate` is the only mode in M0a.
    #[serde(default = "default_write_mode")]
    pub mode: String,
}

fn default_write_mode() -> String {
    "create_or_truncate".to_string()
}

/// A `ctx.files.write` response: the normalized path, bytes written, and a
/// version token. Served from the recording on replay (replay never writes a
/// live file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileWriteResponse {
    /// The normalized relative path the bytes were written to.
    pub path: String,
    /// The number of bytes written.
    pub written_bytes: u64,
    /// An opaque version token for the written file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A confined file stored in the sandbox: its normalized relative path, raw
/// bytes, and content-type. The [`FileSystem`] seam serves/accepts these; the
/// runtime base64-encodes/decodes at the host edge so the trace is byte-exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxFile {
    /// The normalized relative path inside the handle root.
    pub path: String,
    /// The raw file bytes.
    pub bytes: Vec<u8>,
    /// The file's content-type, if known.
    pub content_type: Option<String>,
}

/// The injectable filesystem seam for `ctx.files` (mirrors
/// [`HttpClient`](crate::HttpClient) for net). The runtime resolves a *handle* to
/// a per-applet sandbox root, confines the request path to that root, and only
/// then calls [`read`](FileSystem::read) / [`write`](FileSystem::write) here — so
/// an implementor performs the *capability-checked, confined* effect and need not
/// re-implement policy. It is reached **only in record mode**; on replay the
/// recorder serves the recorded bytes and this trait is never called (CR-8).
///
/// `handle_root(handle)` is the **trusted sandbox-root resolution** (SC-10/SC-12):
/// it maps a user-granted handle to the per-applet root the confinement gate
/// canonicalizes against. A handle the host has not granted a root for resolves to
/// `None`, which the runtime turns into a `PermissionDenied` (no root, no access).
///
/// `symlink_escapes_root(handle, rel_path)` lets the confinement gate ask the
/// host whether the *resolved* target of a path (after following any symlink)
/// escapes the handle root — a fact only the real filesystem knows. The default
/// returns `false` (no symlink escape) so an in-memory backend without symlinks is
/// safe; the real backend canonicalizes and compares against the root.
///
/// This trait is **wasm-clean** by design (no `std::fs` in the signature); the one
/// real-I/O implementation lives host-side. This crate ships only the in-memory
/// test backend [`InMemoryFileSystem`].
pub trait FileSystem {
    /// The per-applet sandbox root for `handle`, or `None` if the host has not
    /// granted a root for it. The returned value is an opaque root id the
    /// confinement gate only compares for presence (the real backend uses it as
    /// the canonicalization base); the runtime never exposes it to the applet.
    fn handle_root(&self, handle: &str) -> Option<String>;

    /// Whether the resolved target of `rel_path` under `handle` escapes the
    /// handle root after following any symlink. Default `false` (no symlinks).
    fn symlink_escapes_root(&self, _handle: &str, _rel_path: &str) -> bool {
        false
    }

    /// Whether the **canonical parent directory** of `rel_path` under `handle`
    /// escapes the handle root (spec/files.md "Gates": "For writes, the canonical
    /// parent directory stays under the root"). This is a distinct, write-only
    /// check from [`symlink_escapes_root`](FileSystem::symlink_escapes_root): a
    /// write target need not yet exist, so the *final* path is not a symlink, but
    /// its parent directory may be (or contain) a symlink that redirects the write
    /// outside the root. The real backend canonicalizes the parent and compares it
    /// against the root; the default returns `false` (an in-memory backend without
    /// symlinks has no parent escape). Reached only in record mode, before the
    /// write commits.
    fn write_parent_escapes_root(&self, _handle: &str, _rel_path: &str) -> bool {
        false
    }

    /// Read the confined file at the normalized `rel_path` under `handle`. Returns
    /// `Ok(None)` for a missing file (the runtime maps that to a clean
    /// `not_found` `StorageError`, never a panic). Reached only in record mode.
    fn read(&self, handle: &str, rel_path: &str) -> Result<Option<SandboxFile>>;

    /// Write `bytes` (with optional `content_type`) to the confined `rel_path`
    /// under `handle`, returning the bytes-written count. Reached only in record
    /// mode (replay serves the recorded write response, never writes a live file).
    fn write(
        &mut self,
        handle: &str,
        rel_path: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> Result<u64>;
}

/// Normalize and **confine** a request path to a handle's sandbox root
/// (spec/files.md "Gates"). Returns the normalized relative POSIX path on
/// success, or a `PermissionDenied` describing the rejected confinement rule.
///
/// Rejected (fail-closed, before any filesystem touch):
///   * an **empty** path;
///   * an **absolute** path (`/...`), a Windows **drive** path (`C:\...`), or a
///     **URI** (`scheme://...`);
///   * a path containing a **NUL** byte;
///   * a **`..` traversal** segment (rejected before join — a `.` segment is
///     stripped, a `..` segment is denied);
///   * a backslash separator (Windows-style), normalized-then-rejected as a
///     drive/traversal escape rather than silently treated as a filename.
///
/// On success the path is the segments joined by `/` with `.` segments removed —
/// the form the grant glob and the [`FileSystem`] read/write are evaluated
/// against. Symlink escape is a *post*-resolution check the runtime runs via
/// [`FileSystem::symlink_escapes_root`] (the in-memory normalization here cannot
/// see a symlink target).
pub fn confine_relative_path(path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(CoreError::PermissionDenied(
            "ctx.files path is empty".to_string(),
        ));
    }
    if path.contains('\0') {
        return Err(CoreError::PermissionDenied(
            "ctx.files path contains a NUL byte".to_string(),
        ));
    }
    // A URI (scheme://...) or a Windows drive path (C:\ or C:/) is not a relative
    // POSIX path: deny before any normalization.
    if path.contains("://") {
        return Err(CoreError::PermissionDenied(format!(
            "ctx.files path must be a relative POSIX path, not a URI: {path:?}"
        )));
    }
    if is_windows_drive_path(path) {
        return Err(CoreError::PermissionDenied(format!(
            "ctx.files path must be a relative POSIX path, not a drive path: {path:?}"
        )));
    }
    // Reject backslashes outright: a Windows separator would otherwise be treated
    // as part of a filename, hiding a `..\..` traversal from the segment scan.
    if path.contains('\\') {
        return Err(CoreError::PermissionDenied(format!(
            "ctx.files path must use '/' separators, not '\\': {path:?}"
        )));
    }
    // Absolute path: a leading '/' escapes the sandbox root.
    if path.starts_with('/') {
        return Err(CoreError::PermissionDenied(format!(
            "ctx.files path must be relative, not absolute: {path:?}"
        )));
    }

    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            // Strip empty segments (`a//b`) and `.` (current dir, no-op).
            "" | "." => continue,
            // A `..` segment would traverse above the handle root: deny before
            // join (spec/files.md: any `..` segment is rejected).
            ".." => {
                return Err(CoreError::PermissionDenied(format!(
                    "ctx.files path escapes the handle root via '..': {path:?}"
                )))
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        // The path normalized to nothing (e.g. `.` or `./`): there is no file.
        return Err(CoreError::PermissionDenied(format!(
            "ctx.files path does not name a file inside the handle: {path:?}"
        )));
    }
    Ok(segments.join("/"))
}

/// Whether `path` looks like a Windows drive path (`C:\...`, `C:/...`, or a bare
/// `C:` drive-relative path). A single-letter drive followed by `:` is the tell.
fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Match a normalized relative `path` against a `path_glob` (spec/files.md): `*`
/// matches within a single path segment (it does **not** cross `/`); `**` may
/// cross segment boundaries. Anchored at both ends (the whole path must match).
///
/// This is a small, dependency-free matcher — the same role the policy's URL glob
/// plays for net. It walks the glob and the path together, treating `**` as
/// "match any run of characters including `/`" and `*` as "match any run of
/// non-`/` characters".
pub fn glob_matches(path_glob: &str, path: &str) -> bool {
    glob_match_bytes(path_glob.as_bytes(), path.as_bytes())
}

fn glob_match_bytes(glob: &[u8], text: &[u8]) -> bool {
    // A run of `*` is `**` (segment-crossing) iff it contains two or more stars;
    // a single `*` matches within one segment (does not cross '/').
    if glob.is_empty() {
        return text.is_empty();
    }
    if glob[0] == b'*' {
        // Count the wildcard run.
        let mut stars = 0usize;
        while stars < glob.len() && glob[stars] == b'*' {
            stars += 1;
        }
        let rest = &glob[stars..];
        let double = stars >= 2;
        // `**/` matches zero-or-more directories *including* the trailing slash,
        // so `data/**/*.json` also matches `data/x.json` (zero directories). Try
        // matching the glob after the `**/` against the text first.
        if double && rest.first() == Some(&b'/') && glob_match_bytes(&rest[1..], text) {
            return true;
        }
        // Try to match `rest` at every position the wildcard may extend to. A
        // single `*` may not consume a '/', so it stops at the first separator.
        let mut t = 0usize;
        loop {
            if glob_match_bytes(rest, &text[t..]) {
                return true;
            }
            if t >= text.len() {
                return false;
            }
            // A single `*` cannot cross a path separator.
            if !double && text[t] == b'/' {
                return false;
            }
            t += 1;
        }
    }
    // Literal byte: must match the head of the text.
    if !text.is_empty() && glob[0] == text[0] {
        return glob_match_bytes(&glob[1..], &text[1..]);
    }
    false
}

/// An in-memory, network-free [`FileSystem`] for tests, CI, and the demo. It
/// holds a per-handle map of normalized-relative-path → [`SandboxFile`] plus the
/// set of granted handle roots and (optionally) a set of paths whose symlink
/// target escapes the root. NO real disk is ever touched.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    /// handle → (relative path → file).
    files: std::collections::BTreeMap<String, std::collections::BTreeMap<String, SandboxFile>>,
    /// The granted per-applet roots: handle → opaque root id.
    roots: std::collections::BTreeMap<String, String>,
    /// (handle, relative path) pairs whose resolved symlink target escapes root.
    escaping_symlinks: std::collections::BTreeSet<(String, String)>,
    /// (handle, relative path) pairs whose canonical *parent directory* escapes
    /// root (the write-only parent-escape confinement case).
    escaping_parents: std::collections::BTreeSet<(String, String)>,
}

impl InMemoryFileSystem {
    /// An empty filesystem with no granted handles (every read is `not_found`,
    /// every handle resolves to no root → `PermissionDenied`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Grant a sandbox root for `handle` (the trusted policy resolution). Returns
    /// `self` for chaining. Without this a handle has no root and every op is
    /// denied.
    pub fn with_handle_root(mut self, handle: impl Into<String>, root: impl Into<String>) -> Self {
        self.roots.insert(handle.into(), root.into());
        self
    }

    /// Seed a file at `rel_path` under `handle` (test convenience). The path is
    /// stored verbatim — callers pass the normalized relative path.
    pub fn with_file(
        mut self,
        handle: impl Into<String>,
        rel_path: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        content_type: Option<&str>,
    ) -> Self {
        let handle = handle.into();
        let rel_path = rel_path.into();
        self.files.entry(handle).or_default().insert(
            rel_path.clone(),
            SandboxFile {
                path: rel_path,
                bytes: bytes.into(),
                content_type: content_type.map(|s| s.to_string()),
            },
        );
        self
    }

    /// Mark `rel_path` under `handle` as a symlink whose target escapes the root
    /// (test convenience for the symlink-escape confinement case).
    pub fn with_escaping_symlink(
        mut self,
        handle: impl Into<String>,
        rel_path: impl Into<String>,
    ) -> Self {
        self.escaping_symlinks
            .insert((handle.into(), rel_path.into()));
        self
    }

    /// Mark `rel_path` under `handle` as a write target whose canonical *parent
    /// directory* escapes the root (test convenience for the write-only
    /// parent-escape confinement case — a symlinked parent dir).
    pub fn with_escaping_parent(
        mut self,
        handle: impl Into<String>,
        rel_path: impl Into<String>,
    ) -> Self {
        self.escaping_parents
            .insert((handle.into(), rel_path.into()));
        self
    }

    /// Direct read of a stored file (test convenience; bypasses confinement).
    pub fn peek(&self, handle: &str, rel_path: &str) -> Option<&SandboxFile> {
        self.files.get(handle).and_then(|m| m.get(rel_path))
    }
}

impl FileSystem for InMemoryFileSystem {
    fn handle_root(&self, handle: &str) -> Option<String> {
        self.roots.get(handle).cloned()
    }

    fn symlink_escapes_root(&self, handle: &str, rel_path: &str) -> bool {
        self.escaping_symlinks
            .contains(&(handle.to_string(), rel_path.to_string()))
    }

    fn write_parent_escapes_root(&self, handle: &str, rel_path: &str) -> bool {
        self.escaping_parents
            .contains(&(handle.to_string(), rel_path.to_string()))
    }

    fn read(&self, handle: &str, rel_path: &str) -> Result<Option<SandboxFile>> {
        Ok(self
            .files
            .get(handle)
            .and_then(|m| m.get(rel_path))
            .cloned())
    }

    fn write(
        &mut self,
        handle: &str,
        rel_path: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> Result<u64> {
        let n = bytes.len() as u64;
        self.files.entry(handle.to_string()).or_default().insert(
            rel_path.to_string(),
            SandboxFile {
                path: rel_path.to_string(),
                bytes: bytes.to_vec(),
                content_type: content_type.map(|s| s.to_string()),
            },
        );
        Ok(n)
    }
}

/// Build the canonical "live filesystem forbidden" error for a context with no
/// [`FileSystem`] wired (mirrors [`live_network_forbidden`](crate::net::live_network_forbidden)).
/// Used by bridges that have no filesystem (e.g. the replay `NullBridge`): a live
/// file effect reaching such a bridge is a determinism bug (CR-8).
pub fn live_files_forbidden(op: &str) -> CoreError {
    CoreError::RuntimeError(format!(
        "ctx.files.{op} attempted a live filesystem effect in a context with no filesystem wired; \
         live file access is forbidden unless a recorded response is being replayed (CR-8)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- confine_relative_path -------------------------------------------

    #[test]
    fn confine_accepts_a_normal_relative_path() {
        assert_eq!(
            confine_relative_path("data/settings.json").unwrap(),
            "data/settings.json"
        );
        // `.` segments and double slashes are stripped, not rejected.
        assert_eq!(
            confine_relative_path("data/./nested//file.json").unwrap(),
            "data/nested/file.json"
        );
    }

    #[test]
    fn confine_rejects_parent_traversal() {
        let err = confine_relative_path("data/../../etc/passwd").unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains(".."), "{err}");
    }

    #[test]
    fn confine_rejects_absolute_path() {
        let err = confine_relative_path("/etc/passwd").unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("absolute"), "{err}");
    }

    #[test]
    fn confine_rejects_uri_drive_nul_and_backslash() {
        assert_eq!(
            confine_relative_path("file:///etc/passwd").unwrap_err().code(),
            "PermissionDenied"
        );
        assert_eq!(
            confine_relative_path("C:\\Windows\\System32").unwrap_err().code(),
            "PermissionDenied"
        );
        assert_eq!(
            confine_relative_path("data/a\0b.json").unwrap_err().code(),
            "PermissionDenied"
        );
        assert_eq!(
            confine_relative_path("data\\..\\escape").unwrap_err().code(),
            "PermissionDenied"
        );
        assert_eq!(
            confine_relative_path("").unwrap_err().code(),
            "PermissionDenied"
        );
    }

    // --- glob_matches -----------------------------------------------------

    #[test]
    fn single_star_matches_within_a_segment_only() {
        assert!(glob_matches("data/*.json", "data/settings.json"));
        assert!(glob_matches("drafts/*.txt", "drafts/note.txt"));
        // `*` does not cross a path separator.
        assert!(!glob_matches("data/*.json", "data/nested/file.json"));
    }

    #[test]
    fn double_star_crosses_segments() {
        assert!(glob_matches("data/**/*.json", "data/a/b/c.json"));
        assert!(glob_matches("data/**/*.json", "data/x.json"));
        assert!(!glob_matches("data/**/*.json", "other/x.json"));
    }

    #[test]
    fn glob_is_anchored_at_both_ends() {
        assert!(!glob_matches("data/*.json", "xdata/settings.json"));
        assert!(!glob_matches("data/*.json", "data/settings.jsonx"));
    }

    // --- InMemoryFileSystem ----------------------------------------------

    #[test]
    fn in_memory_fs_resolves_roots_and_reads_files() {
        let fs = InMemoryFileSystem::new()
            .with_handle_root("workspace_data", "/sandbox/app/workspace_data")
            .with_file("workspace_data", "data/x.json", b"{}".to_vec(), Some("application/json"));
        assert!(fs.handle_root("workspace_data").is_some());
        assert!(fs.handle_root("ungranted").is_none());
        let f = fs.read("workspace_data", "data/x.json").unwrap().unwrap();
        assert_eq!(f.bytes, b"{}");
        // A missing file is Ok(None), not an error.
        assert!(fs.read("workspace_data", "data/missing.json").unwrap().is_none());
    }

    #[test]
    fn in_memory_fs_write_then_read_roundtrips() {
        let mut fs = InMemoryFileSystem::new().with_handle_root("h", "/root");
        let n = fs.write("h", "drafts/note.txt", b"draft v1", Some("text/plain")).unwrap();
        assert_eq!(n, 8);
        let f = fs.read("h", "drafts/note.txt").unwrap().unwrap();
        assert_eq!(f.bytes, b"draft v1");
        assert_eq!(f.content_type.as_deref(), Some("text/plain"));
    }
}
