//! forge-runtime: the sandbox. prd-merged/01-core-runtime-prd.md §2.
//!
//! This crate is the heart of the M0a spine:
//!   *transpiled JS → QuickJS realm with ZERO ambient capability → a
//!   capability-checked `ctx` host bridge → enforced resource limits →
//!   deterministic record/replay.*
//!
//! Design (prd-merged/01 CR-1..CR-13, prd-merged/07 SC-1/SC-2):
//!   * [`JsEngine`] — the engine trait. Target-independent so a QuickJS-WASM
//!     backend can slot in at M0a-exit without restructuring.
//!   * [`QuickJsEngine`] — the native rquickjs implementation
//!     (`#[cfg(not(target_arch = "wasm32"))]`). rquickjs ships native C and does
//!     not build for `wasm32-unknown-unknown`, so it is gated out there
//!     (Cargo.toml `target.'cfg(not(target_arch = "wasm32"))'`).
//!   * [`HostBridge`] — the effect seam (storage/db/ui/log); effects are
//!     **injected**, never imported (CR-1). `time`/`random` are deterministic
//!     seams owned by the recorder, not bridge methods (CR-11).
//!   * [`RunRecorder`] — the deterministic record/replay engine (CR-8/CR-11).
//!   * [`HostContext`] — the single hub where policy + recorder + budgets meet.
//!
//! Two-layer defense (CR-13): the **static policy scan** (forge-pipeline) is the
//! first line against `eval`/`Function`/forbidden globals; this engine is the
//! second line — the realm exposes no host globals beyond `ctx` + standard JS
//! (no `fetch`/`process`/`require`), and resource limits contain anything that
//! does run. See the corpus integration test for the ownership split.

mod bridge;
mod files;
mod host;
mod net;
mod recorder;

pub use bridge::{HostBridge, MemoryHostBridge, NullBridge};
pub use files::{
    confine_relative_path, glob_matches, live_files_forbidden, FileReadRequest, FileReadResponse,
    FileSystem, FileWriteRequest, FileWriteResponse, InMemoryFileSystem, SandboxFile,
};
pub use host::HostContext;
pub use net::{
    resolve_secret_headers, HttpClient, InMemorySecretStore, MockHttpClient, NetHeaderValue,
    NetRequest, NetResponse, SecretStore,
};
pub use recorder::{LogicalClock, Mode, RunRecorder, SplitMix64};

use forge_domain::{AppResult, CoreError};

#[cfg(not(target_arch = "wasm32"))]
mod engine;
#[cfg(not(target_arch = "wasm32"))]
mod runner;
#[cfg(not(target_arch = "wasm32"))]
pub use engine::QuickJsEngine;
#[cfg(not(target_arch = "wasm32"))]
pub use runner::{
    record_dispatch, record_dispatch_with_context, record_notification,
    record_notification_with_context, record_run, record_run_with_context, replay, replay_dispatch,
    replay_notification, run_once,
};

// Re-export the SC-10 trusted-source gate types so a caller (forge-core) can build
// the live `ComposedDecisionContext` and install it on the record entry points
// without depending on forge-policy directly (T037). These are the workspace-policy
// / run-profile / platform-permission gates that read TRUSTED workspace/run/platform
// state, never the request payload (review 048/050).
pub use forge_policy::{
    AllowAll, Category as PolicyCategory, ComposedDecisionContext, DecisionContext,
    PlatformPermissions, RunProfile, WorkspacePolicy,
};

/// A unit of executable code handed to the engine: the transpiled JS plus the
/// applet identity used for provenance in the run record.
///
/// In the spine, `forge-pipeline` produces this from TypeScript via SWC; the
/// runtime treats `source` as opaque JS and never re-parses TypeScript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The applet this code belongs to.
    pub applet_id: forge_domain::AppletId,
    /// Transpiled JavaScript exporting `async function main(ctx, input)`.
    pub source: String,
}

impl Program {
    pub fn new(applet_id: impl Into<forge_domain::AppletId>, source: impl Into<String>) -> Self {
        Program {
            applet_id: applet_id.into(),
            source: source.into(),
        }
    }

    /// The canonical content hash of the executed code, used as the run record's
    /// `code_hash` (provenance + replay key).
    ///
    /// This delegates to [`forge_domain::code_hash`] — the **single** algorithm
    /// (`"sha256:" + lowercase-hex`) the whole spine agrees on — so a runtime run
    /// trace records exactly the hash forge-pipeline computed over the transpiled
    /// JS (reviews 012 P2 / 013 P1 / 014 P1). The runtime no longer emits its old
    /// `fnv1a64:` digest, which the pipeline could never reproduce; a record now
    /// carries a hash that passes `RunRecord::validate_code_hash`.
    pub fn code_hash(&self) -> String {
        forge_domain::code_hash(&self.source)
    }
}

/// What an engine run produced: the script's `AppResult` (or the `CoreError`
/// that suspended/failed it) plus the captured log lines.
///
/// This is the engine-level outcome before it is folded into a
/// [`forge_domain::RunRecord`]; the record/replay API surfaces below build the
/// full record from it plus the recorder's trace.
#[derive(Debug, Clone)]
pub struct EngineOutcome {
    /// `Ok(result)` when `main` returned; `Err(e)` when a limit/policy/runtime
    /// error suspended the run.
    pub result: Result<AppResult, CoreError>,
    /// Log lines captured during the run (bounded by `Limits::log_bytes`).
    pub logs: Vec<String>,
}

/// The pluggable JS execution engine (prd-merged/01 CR-2).
///
/// Implementors run a [`Program`] in a zero-ambient-capability realm, forwarding
/// every `ctx.*` call through the supplied [`HostContext`] (which enforces
/// policy, recording, and per-call budgets). The trait is intentionally narrow
/// and target-independent so alternative backends (QuickJS-WASM, a future
/// engine) are drop-in.
pub trait JsEngine {
    /// Run `program`, driving its `main(ctx, input)` to completion under
    /// `limits`. The `host` hub carries the policy engine, recorder, bridge, and
    /// budgets; `input` is passed to `main` as its second argument.
    ///
    /// Returns an [`EngineOutcome`]. Resource-limit and runtime errors are
    /// returned as `outcome.result = Err(..)`, never as a panic across the FFI
    /// boundary (prd-merged/01 CR-A4).
    fn run(
        &self,
        program: &Program,
        input: &serde_json::Value,
        host: &mut HostContext<'_>,
        limits: &forge_domain::Limits,
    ) -> EngineOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_hash_is_stable_and_content_sensitive() {
        let a = Program::new("app", "export async function main(){}");
        let b = Program::new("app", "export async function main(){}");
        let c = Program::new("app", "export async function main(){ return 1; }");
        assert_eq!(a.code_hash(), b.code_hash());
        assert_ne!(a.code_hash(), c.code_hash());
    }

    /// Reviews 012/013/014: the runtime emits the canonical `sha256:` hash (the
    /// single spine algorithm), never its old `fnv1a64:` digest, and the result
    /// is accepted by the domain's canonical-hash predicate.
    #[test]
    fn code_hash_is_canonical_sha256_not_fnv1a64() {
        let p = Program::new("app", "export async function main(){}");
        let h = p.code_hash();
        assert!(h.starts_with("sha256:"), "must be canonical sha256: got {h}");
        assert!(!h.starts_with("fnv1a64:"), "must not emit fnv1a64: got {h}");
        assert!(
            forge_domain::is_canonical_code_hash(&h),
            "runtime code_hash must satisfy the domain canonical predicate: {h}"
        );
        // It is byte-identical to what forge-pipeline computes over the same JS.
        assert_eq!(h, forge_domain::code_hash("export async function main(){}"));
    }
}
