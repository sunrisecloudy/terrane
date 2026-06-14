//! `ctx.files.read`/`ctx.files.write` for [`HostContext`]: the CR-3
//! capability-checked, sandbox-confined, recorded file handlers plus the shared
//! confinement/cap/content-type gate helpers.
//!
//! The handlers keep every effect inside `recorder.host_call(method, args, ||
//! bridge_call)`, route the deterministic capability+confinement decision through
//! [`gate_files_op`](HostContext::gate_files_op) (consulting only the recorded
//! grant, never the live fs), and run the fail-closed escape-check seam
//! (`handle_root` / `symlink_escapes_root` / `write_parent_escapes_root`) inside
//! the record-mode closure so replay never touches the filesystem.

use super::HostContext;
use crate::files::{
    confine_relative_path, glob_matches, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use forge_domain::{CoreError, FileRule, Result};

impl HostContext<'_> {
    // --- Files (capability-checked, sandbox-confined, recorded) ---------

    /// `ctx.files.read(request)` — read a sandboxed file, gated by the CR-3 files
    /// grant + path confinement and recorded for deterministic replay.
    ///
    /// Order (prd-merged/01 CR-3/CR-4/CR-8, `forge/spec/files.md` "Gates"):
    ///   1. **Role gate** (SC-10): a non-runnable actor cannot read — recorded as
    ///      the run's denial, then fail.
    ///   2. **Capability + confinement gate** (CR-4, runs on record AND replay so
    ///      the decision is deterministic): the manifest's `files.read` grant must
    ///      list the handle and its `path_glob` must match the **normalized** path;
    ///      the path must confine to the handle root (no `..`/absolute/URI/drive/
    ///      NUL). An empty grant ⇒ `CapabilityRequired`; a non-matching path ⇒
    ///      `CapabilityRequired`; a confinement violation ⇒ `PermissionDenied`. A
    ///      denied read is recorded as the run's denial and **no filesystem is
    ///      touched**.
    ///   3. **Host-call budget** (SC-2): a permitted read counts against
    ///      `max_host_calls` (files counts its own calls, like net/log).
    ///   4. **Record/replay** (CR-8): in record mode the host resolves the handle
    ///      root, runs the symlink-escape check, reads the confined bytes, and
    ///      captures the base64 response; on replay the recorded bytes are
    ///      **served** and the live filesystem is never consulted (offline-safe,
    ///      byte-identical even if the file has changed or gone missing).
    pub fn files_read(&mut self, request: FileReadRequest) -> Result<FileReadResponse> {
        let args = serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);

        // 1. Role gate (SC-10): record the denial so it is replayable, then fail.
        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for files.read call".to_string(),
            );
            self.recorder.record_denial("files.read", args, &err)?;
            return Err(err);
        }

        // 1b. Encoding gate (spec/files.md: `base64` is the ONLY read encoding in
        //     M0a). A request asking for any other encoding (e.g. `utf8`) is
        //     rejected as a ValidationError BEFORE the grant/confinement gate or any
        //     filesystem touch — otherwise a recorded read would claim a non-base64
        //     encoding while still returning `bytes_base64`, an inconsistent trace.
        //     Recorded as the run's denial so record/replay stay consistent.
        if request.encoding != "base64" {
            let err = CoreError::ValidationError(format!(
                "ctx.files.read encoding {:?} is not supported; only \"base64\" is supported in M0a (spec/files.md)",
                request.encoding
            ));
            self.recorder.record_denial("files.read", args, &err)?;
            return Err(err);
        }

        // 2. Capability + confinement gate (deterministic, both modes).
        let rel_path = match self.gate_files_op(&request.handle, &request.path, FileAction::Read) {
            Ok(p) => p,
            Err(err) => {
                self.recorder.record_denial("files.read", args, &err)?;
                return Err(err);
            }
        };

        // 3. Host-call budget (SC-2): only a permitted read consumes a slot.
        self.budgets.check_files_call()?;

        // 4. Record/replay (CR-8). INSIDE the closure (record mode only) the host
        //    touches the live filesystem: resolve the handle root, run the
        //    symlink-escape check, then read the confined bytes. On replay the
        //    recorder serves the recorded response and this closure never runs, so
        //    no filesystem is consulted (offline-safe, byte-identical).
        // Compute the byte cap BEFORE borrowing the bridge mutably (the closure
        // captures the bridge, so `self` is no longer reachable from inside it).
        let max_bytes = self.read_rule_max_bytes(&request.handle, &rel_path);
        let bridge = &mut *self.bridge;
        let handle = request.handle.clone();
        let path = rel_path.clone();
        let response_json = self.recorder.host_call("files.read", args, || {
            let fs = bridge.file_system();
            // Sandbox-root resolution (trusted policy): an ungranted handle has no
            // per-applet root → fail closed (PermissionDenied), never a path leak.
            if fs.handle_root(&handle).is_none() {
                return Err(CoreError::PermissionDenied(format!(
                    "ctx.files.read denied: no sandbox root is granted for handle {handle:?}"
                )));
            }
            // Symlink-escape check (post-resolution): the canonical target must
            // stay under the handle root even when the glob matched.
            if fs.symlink_escapes_root(&handle, &path) {
                return Err(CoreError::PermissionDenied(format!(
                    "ctx.files.read denied: symlink target escapes handle root for {path:?}"
                )));
            }
            let file = fs.read(&handle, &path)?;
            let Some(file) = file else {
                // A missing file under an otherwise valid grant is a clean
                // not_found StorageError (spec/files.md), never a panic.
                return Err(CoreError::StorageError(format!(
                    "ctx.files.read not_found: {path:?} does not exist under handle {handle:?}"
                )));
            };
            // Byte cap (SC-5 per-action budget): enforce before serving bytes.
            if let Some(cap) = max_bytes {
                if file.bytes.len() as u64 > cap {
                    return Err(CoreError::ResourceLimitExceeded(format!(
                        "ctx.files.read denied: {} bytes exceeds max_bytes = {cap}",
                        file.bytes.len()
                    )));
                }
            }
            let resp = FileReadResponse {
                path: path.clone(),
                bytes_base64: BASE64.encode(&file.bytes),
                size: file.bytes.len() as u64,
                content_type: file.content_type.clone(),
            };
            serde_json::to_value(&resp).map_err(|e| {
                CoreError::RuntimeError(format!("files.read response serialize failed: {e}"))
            })
        })?;

        let response =
            serde_json::from_value::<FileReadResponse>(response_json).map_err(|e| {
                CoreError::RuntimeError(format!("files.read response decode failed: {e}"))
            })?;

        // 5. Content-type constraint (spec/files.md per-action constraint). The
        //    file's content-type is only known once the response is in hand, so —
        //    like net's response-leg caps — it is checked here on BOTH record and
        //    replay (a recorded response whose content-type violates the grant is
        //    denied identically on replay). A violation surfaces as PermissionDenied
        //    and the body never reaches the applet.
        Self::check_files_content_type(
            &self.files_grant.read,
            &request.handle,
            &rel_path,
            FileAction::Read,
            response.content_type.as_deref(),
        )?;

        Ok(response)
    }

    /// `ctx.files.write(request)` — write a sandboxed file, gated by the CR-3
    /// `files.write` grant + path confinement and recorded for deterministic
    /// replay. Same gate order as [`files_read`](Self::files_read) against the
    /// `write` action. The write leg adds one confinement check the read leg does
    /// not need: because the final target may not exist yet, the **canonical
    /// parent directory** is checked for a symlink escape *in addition to* the
    /// final-target symlink check (spec/files.md "Gates"). On **replay** the
    /// recorded write response is served and the live filesystem is **never**
    /// created/truncated/modified (CR-8).
    pub fn files_write(&mut self, request: FileWriteRequest) -> Result<FileWriteResponse> {
        let args = serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);

        // 1. Role gate (SC-10).
        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for files.write call".to_string(),
            );
            self.recorder.record_denial("files.write", args, &err)?;
            return Err(err);
        }

        // 1b. Write-mode gate (spec/files.md / files.rs: `create_or_truncate` is the
        //     ONLY write mode in M0a). A request asking for any other mode (e.g.
        //     `append`) is rejected as a ValidationError BEFORE the payload decode,
        //     the grant/confinement gate, or any filesystem touch — otherwise an
        //     `append` request would silently TRUNCATE the file while the recorded
        //     trace claims `append`. Recorded as the run's denial so record/replay
        //     stay consistent.
        if request.mode != "create_or_truncate" {
            let err = CoreError::ValidationError(format!(
                "ctx.files.write mode {:?} is not supported; only \"create_or_truncate\" is supported in M0a (spec/files.md)",
                request.mode
            ));
            self.recorder.record_denial("files.write", args, &err)?;
            return Err(err);
        }

        // 1c. Decode the payload BEFORE the gate so an invalid base64 body is a
        //     ValidationError (recorded denial), never an fs touch.
        let bytes = match BASE64.decode(request.bytes_base64.as_bytes()) {
            Ok(b) => b,
            Err(e) => {
                let err = CoreError::ValidationError(format!(
                    "ctx.files.write bytes_base64 is not valid base64: {e}"
                ));
                self.recorder.record_denial("files.write", args, &err)?;
                return Err(err);
            }
        };

        // 2. Capability + confinement gate (deterministic, both modes).
        let rel_path = match self.gate_files_op(&request.handle, &request.path, FileAction::Write) {
            Ok(p) => p,
            Err(err) => {
                self.recorder.record_denial("files.write", args, &err)?;
                return Err(err);
            }
        };

        // 2b. Byte cap (SC-5): enforce on the decoded payload before any fs touch.
        if let Some(cap) = self.write_rule_max_bytes(&request.handle, &rel_path) {
            if bytes.len() as u64 > cap {
                let err = CoreError::ResourceLimitExceeded(format!(
                    "ctx.files.write denied: {} bytes exceeds max_bytes = {cap}",
                    bytes.len()
                ));
                self.recorder.record_denial("files.write", args, &err)?;
                return Err(err);
            }
        }

        // 2c. Content-type constraint (spec/files.md per-action constraint): the
        //     write payload's declared content-type must satisfy every matching
        //     rule that constrains it — enforced on the request before any fs
        //     touch. A violation is a recorded denial (PermissionDenied), never a
        //     write.
        if let Err(err) = Self::check_files_content_type(
            &self.files_grant.write,
            &request.handle,
            &rel_path,
            FileAction::Write,
            request.content_type.as_deref(),
        ) {
            self.recorder.record_denial("files.write", args, &err)?;
            return Err(err);
        }

        // 3. Host-call budget (SC-2).
        self.budgets.check_files_call()?;

        // 4. Record/replay (CR-8). The write touches the live fs only inside the
        //    closure (record mode); on replay the recorder serves the recorded
        //    write response and no live file is created/truncated/modified.
        let bridge = &mut *self.bridge;
        let handle = request.handle.clone();
        let path = rel_path.clone();
        let content_type = request.content_type.clone();
        let response_json = self.recorder.host_call("files.write", args, || {
            // Sandbox-root resolution + symlink-escape check, as for read.
            if bridge.file_system().handle_root(&handle).is_none() {
                return Err(CoreError::PermissionDenied(format!(
                    "ctx.files.write denied: no sandbox root is granted for handle {handle:?}"
                )));
            }
            // Write-only parent-directory confinement (spec/files.md "Gates": "For
            // writes, the canonical parent directory stays under the root"). The
            // final target may not exist yet, so the final-target symlink check
            // alone cannot catch a symlinked PARENT directory that redirects the
            // write outside the root — check the canonical parent first.
            if bridge.file_system().write_parent_escapes_root(&handle, &path) {
                return Err(CoreError::PermissionDenied(format!(
                    "ctx.files.write denied: canonical parent directory escapes handle root for {path:?}"
                )));
            }
            if bridge.file_system().symlink_escapes_root(&handle, &path) {
                return Err(CoreError::PermissionDenied(format!(
                    "ctx.files.write denied: symlink target escapes handle root for {path:?}"
                )));
            }
            let written = bridge.files_write(&handle, &path, &bytes, content_type.as_deref())?;
            let resp = FileWriteResponse {
                path: path.clone(),
                written_bytes: written,
                version: Some("file_version_1".to_string()),
            };
            serde_json::to_value(&resp).map_err(|e| {
                CoreError::RuntimeError(format!("files.write response serialize failed: {e}"))
            })
        })?;

        serde_json::from_value::<FileWriteResponse>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("files.write response decode failed: {e}"))
        })
    }

    /// The shared `ctx.files` capability + confinement gate (CR-3 / spec/files.md
    /// "Gates"), used by both [`files_read`](Self::files_read) and
    /// [`files_write`](Self::files_write). Returns the **normalized relative path**
    /// on success, or a `CapabilityRequired` / `PermissionDenied` error.
    ///
    /// This is deterministic (it consults only the recorded `files_grant`, never
    /// the live filesystem), so it runs identically on record and replay — a call
    /// the grant denied at record time is denied identically on replay.
    fn gate_files_op(&self, handle: &str, path: &str, action: FileAction) -> Result<String> {
        let rules: &[FileRule] = match action {
            FileAction::Read => &self.files_grant.read,
            FileAction::Write => &self.files_grant.write,
        };
        // An empty action list ⇒ the applet never requested this files action ⇒
        // CapabilityRequired (distinct from a path that matches no rule). The
        // message carries the T028 fixture vocabulary for BOTH absent-capability
        // shapes a verifier pins: "manifest did not request files.<action>"
        // (`absent_files_capability_rejected`) and "files.<action> grant required
        // for <handle>:<path>" (`write_without_write_grant_rejected`).
        if rules.is_empty() {
            return Err(CoreError::CapabilityRequired(format!(
                "ctx.files.{action} denied: manifest did not request files.{action}; \
                 a files.{action} grant required for {handle}:{path}"
            )));
        }
        // Confine FIRST: a `..`/absolute/URI/drive/NUL path is a PermissionDenied
        // regardless of any glob, and must never be matched against a rule.
        let rel_path = confine_relative_path(path)?;
        // The normalized path must match a rule for THIS handle.
        let matched = rules
            .iter()
            .any(|r| r.handle == handle && glob_matches(&r.path_glob, &rel_path));
        if !matched {
            // Report the granted globs for this handle so the denial names what
            // WAS allowed (T028 `read_outside_grant_rejected`: the path "is
            // outside granted glob <glob>").
            let globs: Vec<&str> = rules
                .iter()
                .filter(|r| r.handle == handle)
                .map(|r| r.path_glob.as_str())
                .collect();
            return Err(CoreError::CapabilityRequired(format!(
                "ctx.files.{action} path {rel_path} is outside granted glob {} for handle {handle:?}",
                if globs.is_empty() { "(none for this handle)".to_string() } else { globs.join(", ") }
            )));
        }
        Ok(rel_path)
    }

    /// The smallest `max_bytes` cap among the `files.read` rules that match
    /// `handle`/`rel_path` (the most restrictive applicable cap), or `None` if no
    /// matching rule caps the size.
    fn read_rule_max_bytes(&self, handle: &str, rel_path: &str) -> Option<u64> {
        Self::min_matching_max_bytes(&self.files_grant.read, handle, rel_path)
    }

    /// The smallest `max_bytes` cap among the matching `files.write` rules.
    fn write_rule_max_bytes(&self, handle: &str, rel_path: &str) -> Option<u64> {
        Self::min_matching_max_bytes(&self.files_grant.write, handle, rel_path)
    }

    fn min_matching_max_bytes(rules: &[FileRule], handle: &str, rel_path: &str) -> Option<u64> {
        rules
            .iter()
            .filter(|r| r.handle == handle && glob_matches(&r.path_glob, rel_path))
            .filter_map(|r| r.max_bytes)
            .min()
    }

    /// Enforce the per-action `content_types` constraint (spec/files.md: "`max_bytes`
    /// and `content_types` are per-action constraints, not comments. They must be
    /// enforced before a read response or write payload is accepted").
    ///
    /// `content_type` is the actual content-type in hand (the file's, on read; the
    /// write request's, on write). Every matching rule that *constrains*
    /// content-types (a non-empty `content_types`) must permit it — the most
    /// restrictive interpretation, matching how the smallest `max_bytes` is applied.
    /// A constraint with a missing actual content-type is a fail-closed
    /// `PermissionDenied`, mirroring net's `content_type_allowed`. Returns
    /// `PermissionDenied` (spec error vocabulary) on a violation, `Ok(())` otherwise.
    fn check_files_content_type(
        rules: &[FileRule],
        handle: &str,
        rel_path: &str,
        action: FileAction,
        content_type: Option<&str>,
    ) -> Result<()> {
        for rule in rules
            .iter()
            .filter(|r| r.handle == handle && glob_matches(&r.path_glob, rel_path))
        {
            if rule.content_types.is_empty() {
                continue; // unconstrained rule
            }
            match content_type {
                Some(ct) if rule.content_types.iter().any(|a| a.eq_ignore_ascii_case(ct)) => {}
                Some(ct) => {
                    return Err(CoreError::PermissionDenied(format!(
                        "ctx.files.{action} denied: content-type {ct:?} is not in the grant's allowlisted set {:?} for {rel_path:?}",
                        rule.content_types
                    )));
                }
                None => {
                    return Err(CoreError::PermissionDenied(format!(
                        "ctx.files.{action} denied: the grant constrains content-type to {:?} but {rel_path:?} declares none",
                        rule.content_types
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Which `ctx.files` action a gate check is for. Picks the `read`/`write` rule
/// list and renders the `files.<action>` error vocabulary (spec/files.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAction {
    Read,
    Write,
}

impl std::fmt::Display for FileAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileAction::Read => f.write_str("read"),
            FileAction::Write => f.write_str("write"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::MemoryHostBridge;
    use crate::files::{InMemoryFileSystem, SandboxFile};
    use crate::host::HostContext;
    use crate::recorder::RunRecorder;
    use forge_domain::{ActorContext, Capabilities, FilesGrant, Limits, Manifest};

    /// A manifest whose `files` grant has the given read/write rules.
    fn manifest_with_files(files: FilesGrant, max_host_calls: u64) -> Manifest {
        Manifest {
            entrypoint: "main.ts".into(),
            min_api: "forge-api@0.1".into(),
            deterministic: true,
            capabilities: Capabilities { files, ..Capabilities::default() },
            limits: Limits { max_host_calls, ..Limits::default() },
        }
    }

    fn file_rule(handle: &str, path_glob: &str) -> FileRule {
        FileRule {
            handle: handle.into(),
            path_glob: path_glob.into(),
            max_bytes: Some(65536),
            content_types: vec![],
        }
    }

    fn read_req(handle: &str, path: &str) -> FileReadRequest {
        FileReadRequest {
            handle: handle.into(),
            path: path.into(),
            encoding: "base64".into(),
        }
    }

    /// An in-memory bridge with one granted handle root and a single seeded file.
    fn bridge_with_file(handle: &str, root: &str, path: &str, bytes: &[u8]) -> MemoryHostBridge {
        let fs = InMemoryFileSystem::new()
            .with_handle_root(handle, root)
            .with_file(handle, path, bytes.to_vec(), Some("application/json"));
        MemoryHostBridge::new().with_file_system(fs)
    }

    /// A granted read whose normalized path matches the grant glob returns the
    /// file's bytes (base64), and records the call as `files.read`.
    #[test]
    fn files_read_granted_is_allowed_and_recorded() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = bridge_with_file(
            "workspace_data",
            "/sandbox/app/workspace_data",
            "data/settings.json",
            br#"{"ok":true}"#,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .files_read(read_req("workspace_data", "data/settings.json"))
            .unwrap();
        assert_eq!(resp.path, "data/settings.json");
        assert_eq!(resp.size, 11);
        assert_eq!(BASE64.decode(resp.bytes_base64.as_bytes()).unwrap(), br#"{"ok":true}"#);
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "files.read");
    }

    /// A read whose path is outside the grant glob is denied with
    /// CapabilityRequired; the filesystem is never touched and the denial is
    /// recorded as the run's `{"denied": …}` entry.
    #[test]
    fn files_read_outside_grant_is_capability_required() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        // The file exists, but the path is outside the granted glob.
        let mut bridge = bridge_with_file(
            "workspace_data",
            "/root",
            "secrets/private.json",
            b"{}",
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "secrets/private.json"))
            .unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "files.read");
        assert!(calls[0].response.get("denied").is_some());
    }

    /// An applet with no `files` capability at all gets CapabilityRequired.
    #[test]
    fn files_read_without_capability_is_capability_required() {
        let manifest = manifest_with_files(FilesGrant::default(), 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "data/x.json"))
            .unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
    }

    /// A `..` traversal, an absolute path, and a symlink whose target escapes the
    /// root are each rejected with PermissionDenied (sandbox confinement).
    #[test]
    fn files_read_traversal_absolute_and_symlink_escape_are_permission_denied() {
        let grant = FilesGrant {
            // Broad glob so the rejection is the CONFINEMENT, not the glob.
            read: vec![file_rule("workspace_data", "**")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");

        // `..` traversal — denied before any fs touch.
        {
            let mut bridge =
                MemoryHostBridge::new().with_file_system(
                    InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
                );
            let mut host = HostContext::new(
                &manifest, &actor, RunRecorder::recording(1, 0), &mut bridge,
            )
            .unwrap();
            let err = host
                .files_read(read_req("workspace_data", "data/../../etc/passwd"))
                .unwrap_err();
            assert_eq!(err.code(), "PermissionDenied", "{err}");
            assert!(err.to_string().contains(".."), "{err}");
        }

        // Absolute path — denied.
        {
            let mut bridge =
                MemoryHostBridge::new().with_file_system(
                    InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
                );
            let mut host = HostContext::new(
                &manifest, &actor, RunRecorder::recording(1, 0), &mut bridge,
            )
            .unwrap();
            let err = host
                .files_read(read_req("workspace_data", "/etc/passwd"))
                .unwrap_err();
            assert_eq!(err.code(), "PermissionDenied", "{err}");
        }

        // Symlink whose resolved target escapes the root — glob matches, path
        // confines, but the symlink-escape check (post-resolution) denies it.
        {
            let fs = InMemoryFileSystem::new()
                .with_handle_root("workspace_data", "/root")
                .with_escaping_symlink("workspace_data", "links/outside.md");
            let mut bridge = MemoryHostBridge::new().with_file_system(fs);
            let mut host = HostContext::new(
                &manifest, &actor, RunRecorder::recording(1, 0), &mut bridge,
            )
            .unwrap();
            let err = host
                .files_read(read_req("workspace_data", "links/outside.md"))
                .unwrap_err();
            assert_eq!(err.code(), "PermissionDenied", "{err}");
            assert!(err.to_string().contains("symlink"), "{err}");
        }
    }

    /// A missing file under an otherwise-valid read grant is a clean `not_found`
    /// StorageError, never a panic.
    #[test]
    fn files_read_missing_file_is_not_found() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        // Root granted, but the file does not exist.
        let mut bridge = MemoryHostBridge::new().with_file_system(
            InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "data/missing.json"))
            .unwrap_err();
        assert_eq!(err.code(), "StorageError");
        assert!(err.to_string().contains("not_found"), "{err}");
    }

    /// A handle the host has not granted a root for is denied (no root → no
    /// access), even when the manifest grant matches the path.
    #[test]
    fn files_read_ungranted_handle_root_is_permission_denied() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        // Empty fs: no granted root for any handle.
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "data/x.json"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "{err}");
        assert!(err.to_string().contains("sandbox root"), "{err}");
    }

    /// A recorded read replays its recorded bytes byte-identically, WITHOUT
    /// touching the live filesystem (deterministic, offline-safe): record a read,
    /// then replay the trace through a NullBridge (no live fs) and the live file
    /// is absent — yet the replayed response is identical (CR-8).
    #[test]
    fn files_read_recorded_replays_byte_identical_without_live_fs() {
        use crate::bridge::NullBridge;

        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "cache/*.txt")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let request = read_req("workspace_data", "cache/value.txt");

        // Record against a live fs holding the file.
        let mut rec_bridge = bridge_with_file(
            "workspace_data",
            "/root",
            "cache/value.txt",
            b"recorded bytes v1",
        );
        let mut rec_host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut rec_bridge,
        )
        .unwrap();
        let recorded_resp = rec_host.files_read(request.clone()).unwrap();
        let (recorder, _logs) = rec_host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);

        // Replay through a NullBridge: the live fs is never consulted (the
        // recorder serves the recorded bytes). The file is ABSENT live, proving
        // replay does not re-read it.
        let mut replay_bridge = NullBridge::new();
        let mut replay_host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::replaying(1, 0, calls),
            &mut replay_bridge,
        )
        .unwrap();
        let replayed_resp = replay_host.files_read(request).unwrap();
        assert_eq!(recorded_resp, replayed_resp);
        assert_eq!(
            BASE64.decode(replayed_resp.bytes_base64.as_bytes()).unwrap(),
            b"recorded bytes v1"
        );
    }

    /// A write with a matching `files.write` grant commits the bytes and a
    /// follow-up read returns them; a write without a write grant is denied.
    #[test]
    fn files_write_granted_then_read_back_and_write_without_grant_denied() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "drafts/*.txt")],
            write: vec![file_rule("workspace_data", "drafts/*.txt")],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new().with_file_system(
            InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let write = FileWriteRequest {
            handle: "workspace_data".into(),
            path: "drafts/note.txt".into(),
            bytes_base64: BASE64.encode(b"draft v1"),
            content_type: Some("text/plain".into()),
            mode: "create_or_truncate".into(),
        };
        let w = host.files_write(write).unwrap();
        assert_eq!(w.path, "drafts/note.txt");
        assert_eq!(w.written_bytes, 8);
        // Read it back through the same handle's read grant.
        let r = host.files_read(read_req("workspace_data", "drafts/note.txt")).unwrap();
        assert_eq!(BASE64.decode(r.bytes_base64.as_bytes()).unwrap(), b"draft v1");
        drop(host);
        // The bytes are committed to the sandbox.
        assert_eq!(
            bridge.peek_file("workspace_data", "drafts/note.txt").map(|f| f.bytes.clone()),
            Some(b"draft v1".to_vec())
        );
    }

    /// A write without any `files.write` grant is CapabilityRequired and never
    /// touches the filesystem.
    #[test]
    fn files_write_without_write_grant_is_capability_required() {
        let grant = FilesGrant {
            // Read-only grant: no write rules.
            read: vec![file_rule("workspace_data", "drafts/*.txt")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new().with_file_system(
            InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let write = FileWriteRequest {
            handle: "workspace_data".into(),
            path: "drafts/note.txt".into(),
            bytes_base64: BASE64.encode(b"draft v1"),
            content_type: Some("text/plain".into()),
            mode: "create_or_truncate".into(),
        };
        let err = host.files_write(write).unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
        drop(host);
        assert!(
            bridge.peek_file("workspace_data", "drafts/note.txt").is_none(),
            "a denied write must not touch the filesystem"
        );
    }

    /// A read asking for a non-`base64` encoding (`utf8`) is rejected as a
    /// ValidationError BEFORE the grant/confinement gate, recorded as the run's
    /// denial, and the filesystem is never touched — even though the path is
    /// inside the grant and the file exists (spec/files.md: base64 only in M0a).
    #[test]
    fn files_read_unsupported_encoding_is_validation_error() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = bridge_with_file(
            "workspace_data",
            "/root",
            "data/settings.json",
            br#"{"ok":true}"#,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let mut req = read_req("workspace_data", "data/settings.json");
        req.encoding = "utf8".into();
        let err = host.files_read(req).unwrap_err();
        assert_eq!(err.code(), "ValidationError", "{err}");
        assert!(err.to_string().contains("only \"base64\" is supported"), "{err}");
        // The denial is recorded so record/replay stays consistent.
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "files.read");
        assert!(calls[0].response.get("denied").is_some());
    }

    /// A write asking for a non-`create_or_truncate` mode (`append`) is rejected
    /// as a ValidationError BEFORE the payload decode / grant gate / any fs touch,
    /// recorded as the run's denial, and never truncates the file (spec/files.md:
    /// create_or_truncate is the only write mode in M0a).
    #[test]
    fn files_write_unsupported_mode_is_validation_error() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "drafts/*.txt")],
            write: vec![file_rule("workspace_data", "drafts/*.txt")],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new().with_file_system(
            InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let write = FileWriteRequest {
            handle: "workspace_data".into(),
            path: "drafts/note.txt".into(),
            bytes_base64: BASE64.encode(b"draft v1"),
            content_type: Some("text/plain".into()),
            mode: "append".into(),
        };
        let err = host.files_write(write).unwrap_err();
        assert_eq!(err.code(), "ValidationError", "{err}");
        assert!(err.to_string().contains("only \"create_or_truncate\" is supported"), "{err}");
        drop(host);
        // The rejected append never created/truncated the file.
        assert!(
            bridge.peek_file("workspace_data", "drafts/note.txt").is_none(),
            "a rejected write mode must not touch the filesystem"
        );
    }

    /// A read whose file exceeds the rule's `max_bytes` is denied
    /// (ResourceLimitExceeded) and the over-cap bytes are not served.
    #[test]
    fn files_read_over_max_bytes_is_resource_limit_exceeded() {
        let grant = FilesGrant {
            read: vec![FileRule {
                handle: "workspace_data".into(),
                path_glob: "data/*.json".into(),
                max_bytes: Some(4),
                content_types: vec![],
            }],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        // 11-byte file, cap is 4.
        let mut bridge = bridge_with_file(
            "workspace_data",
            "/root",
            "data/big.json",
            br#"{"ok":true}"#,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "data/big.json"))
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded", "{err}");
        assert!(err.to_string().contains("max_bytes"), "{err}");
    }

    /// A `content_types`-constrained grant: a read of a file whose content-type
    /// is outside the allowlisted set is denied (PermissionDenied) and the bytes
    /// are not served (spec/files.md per-action content-type constraint).
    #[test]
    fn files_read_wrong_content_type_is_permission_denied() {
        let grant = FilesGrant {
            read: vec![FileRule {
                handle: "workspace_data".into(),
                path_glob: "data/*.json".into(),
                max_bytes: Some(65536),
                content_types: vec!["application/json".into()],
            }],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        // The seeded file is text/html — outside the grant's application/json set.
        let fs = InMemoryFileSystem::new()
            .with_handle_root("workspace_data", "/root")
            .with_file("workspace_data", "data/page.json", b"<html></html>".to_vec(), Some("text/html"));
        let mut bridge = MemoryHostBridge::new().with_file_system(fs);
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "data/page.json"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "{err}");
        assert!(err.to_string().contains("content-type"), "{err}");
    }

    /// A `content_types`-constrained read whose file matches the allowlisted set is
    /// served unchanged: the content-type check must not over-deny a compliant
    /// response (positive control).
    #[test]
    fn files_read_matching_content_type_is_allowed() {
        let grant = FilesGrant {
            read: vec![FileRule {
                handle: "workspace_data".into(),
                path_glob: "data/*.json".into(),
                max_bytes: Some(65536),
                content_types: vec!["application/json".into()],
            }],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = bridge_with_file(
            "workspace_data",
            "/root",
            "data/settings.json",
            br#"{"ok":true}"#,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .files_read(read_req("workspace_data", "data/settings.json"))
            .unwrap();
        assert_eq!(resp.content_type.as_deref(), Some("application/json"));
    }

    /// A write whose declared content-type is outside the grant's allowlisted set
    /// is denied (PermissionDenied) and never touches the filesystem.
    #[test]
    fn files_write_wrong_content_type_is_permission_denied() {
        let grant = FilesGrant {
            read: vec![],
            write: vec![FileRule {
                handle: "workspace_data".into(),
                path_glob: "drafts/*.txt".into(),
                max_bytes: Some(65536),
                content_types: vec!["text/plain".into()],
            }],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new().with_file_system(
            InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let write = FileWriteRequest {
            handle: "workspace_data".into(),
            path: "drafts/note.txt".into(),
            bytes_base64: BASE64.encode(b"<html>"),
            content_type: Some("text/html".into()), // outside text/plain
            mode: "create_or_truncate".into(),
        };
        let err = host.files_write(write).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "{err}");
        assert!(err.to_string().contains("content-type"), "{err}");
        drop(host);
        assert!(
            bridge.peek_file("workspace_data", "drafts/note.txt").is_none(),
            "a content-type-denied write must not touch the filesystem"
        );
    }

    /// A write that declares NO content-type against a grant that *constrains*
    /// content-types is fail-closed (PermissionDenied) — mirrors net's behavior
    /// when a rule constrains a content-type the request omits.
    #[test]
    fn files_write_missing_content_type_against_constraint_is_permission_denied() {
        let grant = FilesGrant {
            read: vec![],
            write: vec![FileRule {
                handle: "workspace_data".into(),
                path_glob: "drafts/*.txt".into(),
                max_bytes: Some(65536),
                content_types: vec!["text/plain".into()],
            }],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new().with_file_system(
            InMemoryFileSystem::new().with_handle_root("workspace_data", "/root"),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let write = FileWriteRequest {
            handle: "workspace_data".into(),
            path: "drafts/note.txt".into(),
            bytes_base64: BASE64.encode(b"draft"),
            content_type: None, // omitted, but the grant constrains the type
            mode: "create_or_truncate".into(),
        };
        let err = host.files_write(write).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "{err}");
        drop(host);
        assert!(bridge.peek_file("workspace_data", "drafts/note.txt").is_none());
    }

    /// A write whose **canonical parent directory** escapes the handle root via a
    /// symlinked parent is denied (PermissionDenied) and never touches the
    /// filesystem (spec/files.md "Gates": "For writes, the canonical parent
    /// directory stays under the root"). The grant matches and the final target
    /// does not yet exist, so this is caught by the write-only parent-escape check,
    /// not the final-target symlink check.
    #[test]
    fn files_write_parent_directory_symlink_escape_is_permission_denied() {
        let grant = FilesGrant {
            read: vec![],
            write: vec![file_rule("workspace_data", "drafts/*.txt")],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        // `drafts/` is a symlink whose canonical target is outside the root, so a
        // write to `drafts/note.txt` would land outside the sandbox even though the
        // final file does not exist yet.
        let fs = InMemoryFileSystem::new()
            .with_handle_root("workspace_data", "/root")
            .with_escaping_parent("workspace_data", "drafts/note.txt");
        let mut bridge = MemoryHostBridge::new().with_file_system(fs);
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let write = FileWriteRequest {
            handle: "workspace_data".into(),
            path: "drafts/note.txt".into(),
            bytes_base64: BASE64.encode(b"draft v1"),
            content_type: Some("text/plain".into()),
            mode: "create_or_truncate".into(),
        };
        let err = host.files_write(write).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "{err}");
        assert!(err.to_string().contains("parent directory"), "{err}");
        drop(host);
        assert!(
            bridge.peek_file("workspace_data", "drafts/note.txt").is_none(),
            "a parent-escape-denied write must not touch the filesystem"
        );
    }

    /// The read content-type constraint is enforced on **replay** too: a recorded
    /// read whose response content-type violates the grant is denied identically
    /// when replayed (the recording is policy-bound, like net's response caps).
    #[test]
    fn files_read_content_type_is_enforced_on_replay() {
        use crate::bridge::NullBridge;
        use forge_domain::RecordedCall;

        let grant = FilesGrant {
            read: vec![FileRule {
                handle: "workspace_data".into(),
                path_glob: "data/*.json".into(),
                max_bytes: Some(65536),
                content_types: vec!["application/json".into()],
            }],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext::owner("dev");
        let request = read_req("workspace_data", "data/page.json");

        // A recorded read whose response content-type is text/html (off-grant).
        let recorded_resp = FileReadResponse {
            path: "data/page.json".into(),
            bytes_base64: BASE64.encode(b"<html></html>"),
            size: 13,
            content_type: Some("text/html".into()),
        };
        let recorded = vec![RecordedCall {
            seq: 0,
            method: "files.read".into(),
            args: serde_json::to_value(&request).unwrap(),
            response: serde_json::to_value(&recorded_resp).unwrap(),
        }];

        let mut bridge = NullBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::replaying(1, 0, recorded),
            &mut bridge,
        )
        .unwrap();
        let err = host.files_read(request).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "{err}");
        assert!(err.to_string().contains("content-type"), "{err}");
    }

    /// `ctx.files` counts against the host-call flood cap (SC-2): the (n+1)th
    /// allowed read over `max_host_calls` trips ResourceLimitExceeded.
    #[test]
    fn files_read_counts_against_host_call_budget() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 1);
        let actor = ActorContext::owner("dev");
        let fs = InMemoryFileSystem::new()
            .with_handle_root("workspace_data", "/root")
            .with_file("workspace_data", "data/a.json", b"{}".to_vec(), None)
            .with_file("workspace_data", "data/b.json", b"{}".to_vec(), None);
        let mut bridge = MemoryHostBridge::new().with_file_system(fs);
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        assert!(host.files_read(read_req("workspace_data", "data/a.json")).is_ok());
        let err = host
            .files_read(read_req("workspace_data", "data/b.json"))
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
    }

    /// A non-runnable actor (Viewer) cannot read; the denial is recorded.
    #[test]
    fn files_read_denied_for_non_runnable_role() {
        let grant = FilesGrant {
            read: vec![file_rule("workspace_data", "data/*.json")],
            write: vec![],
        };
        let manifest = manifest_with_files(grant, 100);
        let actor = ActorContext { actor: "viewer".into(), role: forge_domain::Role::Viewer };
        let mut bridge = bridge_with_file("workspace_data", "/root", "data/x.json", b"{}");
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .files_read(read_req("workspace_data", "data/x.json"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].response.get("denied").is_some());
    }

    // Touch SandboxFile so the import is exercised even if a refactor drops a use.
    #[allow(dead_code)]
    fn _assert_sandbox_file_constructs() -> SandboxFile {
        SandboxFile { path: "x".into(), bytes: vec![], content_type: None }
    }
}
