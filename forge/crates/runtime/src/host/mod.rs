//! The shared host context: the single mutable hub the `ctx.*` forwarders call.
//!
//! Every `ctx.*` host call funnels through [`HostContext::call`], which is the
//! one place that enforces the full chain for a host effect:
//!   1. policy/capability check (forge-policy [`PolicyEngine`], prd-merged/01
//!      CR-4 call-time checks);
//!   2. the deterministic record/replay recorder (prd-merged/01 CR-8/CR-11);
//!   3. log/storage byte budgets (prd-merged/01 CR-5).
//!
//! Keeping this target-independent (no QuickJS) means the policy + record/replay
//! seam is testable and `wasm32`-clean; the engine only marshals JS values to
//! and from `serde_json::Value` and calls in here.

use crate::bridge::HostBridge;
use crate::files::{
    confine_relative_path, glob_matches, FileReadRequest, FileReadResponse, FileWriteRequest,
    FileWriteResponse,
};
use crate::net::{resolve_secret_headers, NetHeaderValue, NetRequest, NetResponse};
use crate::recorder::RunRecorder;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use forge_domain::{
    ActorContext, CoreError, FileRule, FilesGrant, Limits, Manifest, NetGrant, PermissionSnapshot,
    Result,
};
use forge_policy::{Access, HostCall, NetPolicy, PolicyEngine};

// Low-coupling host-call handlers split into focused submodules. Each adds
// `impl HostContext` methods so `HostContext`'s public surface is reachable at
// the same paths regardless of which file the handler body lives in:
//   * `policy` — the `check_or_record_denial` denial-recording chokepoint;
//   * `time`   — the `ctx.time.now` / `ctx.random.next` deterministic seams;
//   * `log`    — the `ctx.log` sink + its byte/call budgets;
//   * `ui`     — `ctx.ui.render` + the UI event-dispatch envelope.
mod log;
mod policy;
mod time;
mod ui;

/// The hub shared (via interior mutability in the engine) by all `ctx.*`
/// forwarders for the duration of a single run.
pub struct HostContext<'b> {
    policy: PolicyEngine,
    recorder: RunRecorder,
    bridge: &'b mut dyn HostBridge,
    limits: Limits,
    /// Bytes appended to the log so far (against `Limits::log_bytes`).
    log_bytes_used: u64,
    /// Bytes written to storage so far (against `Limits::storage_bytes`).
    storage_bytes_used: u64,
    /// `ctx.log` calls so far (against `Limits::max_host_calls`, review 009 P2):
    /// a flood of empty-string logs costs zero bytes, so the byte budget alone
    /// can't stop it — count the *calls* against the host-call cap too.
    log_calls_used: u64,
    /// The full network egress allowlist for `ctx.net.fetch` (prd-merged/07
    /// SC-5/SC-8), with **every** SC-5 constraint intact (request + response).
    /// Derived from the policy's permission snapshot at construction so it tracks
    /// the *recorded* grants on replay (review 009 P1 CR-9), not the live manifest.
    /// Empty ⇒ no network (the default for every applet). The **response-leg**
    /// check (`net_fetch` step 5) runs against this full allowlist.
    net_allowlist: NetGrant,
    /// The **request-phase** view of [`net_allowlist`](Self::net_allowlist): the
    /// same rules with their *response* constraints (`max_response_bytes`,
    /// `response_content_types`) cleared. The call gate (`net_fetch` step 2) must
    /// decide *before* a request is sent, when the response is unknown — so it
    /// evaluates only the request-side gates against this view. A rule that
    /// constrains the response would otherwise spuriously deny at the call gate
    /// (the policy denies an unknown response content-type); stripping the
    /// response constraints here defers them, intact, to the response leg where
    /// the real response is in hand. Built once at construction so each fetch is
    /// allocation-free on this path.
    net_allowlist_request_phase: NetGrant,
    /// `ctx.net.fetch` calls so far (against `Limits::max_host_calls`). `net` is
    /// gated by the [`NetPolicy`] decision rather than the [`PolicyEngine`]
    /// `HostCall` categories, so — like `ctx.log` — it counts its own calls
    /// against the host-call flood cap (SC-2) here.
    net_calls_used: u64,
    /// The full handle-scoped filesystem grant for `ctx.files` (prd-merged/01
    /// CR-3, `forge/spec/files.md`). Like [`net_allowlist`](Self::net_allowlist)
    /// it is derived from the policy's permission **snapshot** at construction, so
    /// on replay it is the *recorded* grant (built via `PolicyEngine::from_snapshot`),
    /// not whatever the live manifest grants now — keeping a files allow/deny
    /// decision deterministic across replay (review 009 P1 CR-9). Empty ⇒ no file
    /// access (the default for every applet).
    files_grant: FilesGrant,
    /// `ctx.files.read`/`ctx.files.write` calls so far (against
    /// `Limits::max_host_calls`). Like `net`, files is gated by its own grant
    /// (not the [`PolicyEngine`] `HostCall` categories), so it counts its own
    /// calls against the host-call flood cap (SC-2) here.
    files_calls_used: u64,
    /// Captured log lines (mirrored into the RunRecord).
    logs: Vec<String>,
}

impl<'b> HostContext<'b> {
    pub fn new(
        manifest: &Manifest,
        actor: &ActorContext,
        recorder: RunRecorder,
        bridge: &'b mut dyn HostBridge,
    ) -> Result<Self> {
        // `PolicyEngine::new` validates the manifest's storage glob grants
        // (forge-policy review 006 P2), so it can now fail closed; propagate that
        // instead of constructing a hub around invalid grants.
        Ok(Self::with_policy(
            PolicyEngine::new(manifest, actor)?,
            manifest.limits.clone(),
            recorder,
            bridge,
        ))
    }

    /// Build a hub around a pre-constructed [`PolicyEngine`]. Replay uses this
    /// with a policy built from the run's recorded [`PermissionSnapshot`]
    /// (review 009 P1 CR-9), so the replay re-derives the *recorded* permission
    /// decision rather than whatever the live manifest grants now.
    pub fn with_policy(
        policy: PolicyEngine,
        limits: Limits,
        recorder: RunRecorder,
        bridge: &'b mut dyn HostBridge,
    ) -> Self {
        // The net allowlist rides on the evaluated permission snapshot's
        // capabilities, so on replay it is the *recorded* grant (built via
        // `PolicyEngine::from_snapshot`), not whatever the live manifest grants
        // now — keeping a net allow/deny decision deterministic across replay
        // exactly like the storage/db scopes (review 009 P1 CR-9).
        let snapshot = policy.snapshot();
        let net_allowlist = snapshot.capabilities.net;
        let net_allowlist_request_phase = request_phase_allowlist(&net_allowlist);
        // The files grant likewise rides on the recorded snapshot's capabilities,
        // so a files allow/deny is deterministic across replay (review 009 P1 CR-9).
        let files_grant = snapshot.capabilities.files;
        HostContext {
            policy,
            recorder,
            bridge,
            limits,
            log_bytes_used: 0,
            storage_bytes_used: 0,
            log_calls_used: 0,
            net_allowlist,
            net_allowlist_request_phase,
            net_calls_used: 0,
            files_grant,
            files_calls_used: 0,
            logs: Vec::new(),
        }
    }

    /// The evaluated permission snapshot for this run (review 009 P1 CR-9), to
    /// be recorded on the [`RunRecord`] so a later replay is governed by the
    /// permissions in effect *now*, not the live manifest then.
    pub fn permission_snapshot(&self) -> PermissionSnapshot {
        self.policy.snapshot()
    }

    /// Consume the context after a run, yielding the recorder (for the trace)
    /// and the captured logs.
    pub fn finish(self) -> (RunRecorder, Vec<String>) {
        (self.recorder, self.logs)
    }

    /// In replay mode, fail the run if not every recorded call was consumed
    /// (review 009 P2). Delegates to the recorder; no-op in record mode.
    pub fn assert_replay_consumed(&self) -> Result<()> {
        self.recorder.assert_fully_consumed()
    }

    // --- Storage (capability-checked, recorded effects) ------------------

    pub fn storage_get(&mut self, key: &str) -> Result<serde_json::Value> {
        let args = serde_json::json!([key]);
        self.check_or_record_denial(
            &HostCall::Storage { op: Access::Read, key: key.to_string() },
            "storage.get",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let k = key.to_string();
        self.recorder
            .host_call("storage.get", args, || bridge.storage_get(&k))
    }

    pub fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        let args = serde_json::json!([key, value]);
        self.check_or_record_denial(
            &HostCall::Storage { op: Access::Write, key: key.to_string() },
            "storage.set",
            &args,
        )?;
        // Account the written bytes against the storage byte budget (CR-5).
        let value_bytes = serde_json::to_vec(&value).map(|v| v.len()).unwrap_or(0) as u64;
        self.storage_bytes_used = self.storage_bytes_used.saturating_add(value_bytes);
        if self.storage_bytes_used > self.limits.storage_bytes {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "storage byte budget exceeded: storage_bytes = {} reached",
                self.limits.storage_bytes
            )));
        }
        let bridge = &mut *self.bridge;
        let k = key.to_string();
        let v = value.clone();
        self.recorder.host_call("storage.set", args, || {
            bridge.storage_set(&k, v).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    pub fn storage_delete(&mut self, key: &str) -> Result<()> {
        let args = serde_json::json!([key]);
        self.check_or_record_denial(
            &HostCall::Storage { op: Access::Write, key: key.to_string() },
            "storage.delete",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let k = key.to_string();
        self.recorder.host_call("storage.delete", args, || {
            bridge.storage_delete(&k).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    pub fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>> {
        let args = serde_json::json!([prefix]);
        self.check_or_record_denial(
            &HostCall::Storage { op: Access::Read, key: prefix.to_string() },
            "storage.list",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let p = prefix.to_string();
        let resp = self.recorder.host_call("storage.list", args, || {
            Ok(serde_json::json!(bridge.storage_list(&p)?))
        })?;
        Ok(resp
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    // --- Db (capability-checked, recorded effects) ----------------------

    pub fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String> {
        let args = serde_json::json!([collection, record]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Write, collection: collection.to_string() },
            "db.insert",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let r = record.clone();
        let resp = self.recorder.host_call("db.insert", args, || {
            Ok(serde_json::json!(bridge.db_insert(&c, r)?))
        })?;
        Ok(resp.as_str().unwrap_or("").to_string())
    }

    pub fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        let args = serde_json::json!([collection, id]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.to_string() },
            "db.get",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let i = id.to_string();
        self.recorder
            .host_call("db.get", args, || bridge.db_get(&c, &i))
    }

    pub fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        let args = serde_json::json!([collection]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.to_string() },
            "db.list",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let resp = self.recorder.host_call("db.list", args, || {
            Ok(serde_json::json!(bridge.db_list(&c)?))
        })?;
        Ok(resp.as_array().cloned().unwrap_or_default())
    }

    /// `ctx.db.query(collection, query)` — run the structured query plan against
    /// the collection and return the matched rows (DL-15). Like the other `db.*`
    /// reads it is gated on `db.read` for `collection` and recorded: in record
    /// mode the call + the bridge's rows are appended as a `RecordedCall`; on
    /// replay the recorded rows are *served* (the live storage is never touched),
    /// so replay stays byte-identical. A denied query is recorded as the run's
    /// denial and no rows are returned.
    pub fn db_query(
        &mut self,
        collection: &str,
        mut query: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Pin the query's `from` to the capability-checked `collection` BEFORE it
        // reaches any bridge, so a caller cannot read an ungranted collection by
        // putting a different `from` in the query body — the host is the single
        // source of truth for which collection a db.read grant authorizes
        // (review 052 #2; the real StorageHostBridge also pins this, but
        // normalizing here means no bridge — incl. test doubles — can widen).
        if let Some(obj) = query.as_object_mut() {
            obj.insert("from".into(), serde_json::Value::String(collection.to_string()));
        }
        let args = serde_json::json!([collection, query]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.to_string() },
            "db.query",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let q = query.clone();
        self.recorder
            .host_call("db.query", args, || bridge.db_query(&c, q))
    }

    // --- Net (egress-policy-checked, recorded) --------------------------

    /// `ctx.net.fetch(request)` — perform a network request, gated by the SC-5
    /// network egress policy and recorded for deterministic replay.
    ///
    /// Order (prd-merged/07 SC-5, prd-merged/01 CR-3/CR-4/CR-8):
    ///   1. **Role gate** (SC-10): a non-runnable actor cannot fetch — denied
    ///      before any policy/bridge work, recorded as the run's denial.
    ///   2. **Egress policy** ([`NetPolicy`], capability check at call time, CR-4):
    ///      the request is matched against the manifest's `net` allowlist. An
    ///      empty allowlist ⇒ `CapabilityRequired`; a non-matching/forbidden
    ///      request (host/scheme/path/method mismatch, a private-IP/localhost
    ///      target denied by default, a size/timeout/content-type violation, a
    ///      secret-header violation) ⇒ `PermissionDenied`. A denied fetch is
    ///      recorded as the run's denial and **no request reaches the client**.
    ///   3. **Host-call budget** (SC-2): a permitted fetch counts against
    ///      `max_host_calls` (the `NetPolicy` decision is separate from the
    ///      `PolicyEngine` category counter, so net counts its own calls here,
    ///      like `ctx.log`).
    ///   4. **Record/replay** (CR-8): in record mode the response the bridge's
    ///      injected [`HttpClient`](crate::HttpClient) returned is captured; on
    ///      replay the recorded response is **served** and no live call is made —
    ///      live network is forbidden unless a recorded response is being served.
    pub fn net_fetch(&mut self, request: NetRequest) -> Result<NetResponse> {
        // The args RECORDED into the trace are trace-safe (SC-13): a `secret_ref`
        // header is kept verbatim (it carries only the non-sensitive name), but a
        // *literal* value on a secret-like header (Authorization/Cookie/…) is
        // redacted to a placeholder, so even a request the applet wrote with a
        // plaintext secret as a literal — which the policy denies — cannot persist
        // that plaintext in the RunRecord. The request handed to the bridge below
        // is the ORIGINAL (unredacted) one; only the recorded copy is redacted.
        let args = trace_safe_args(&request);

        // 1. Role gate (SC-10): record the denial so it is replayable, then fail.
        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for net.fetch call".to_string(),
            );
            self.recorder.record_denial("net.fetch", args, &err)?;
            return Err(err);
        }

        // 1b. Secret-exfil guard (SC-13 / spec/secrets.md): M0a permits a
        //     `secret_ref` ONLY in a header value. A secret_ref smuggled into the
        //     request BODY is a secret-exfil pattern — reject it as a
        //     ValidationError before any policy/budget/bridge work, and never send.
        //     This is recorded as the run's denial so it is replayable. (The body
        //     is opaque to the runtime, so this is a textual scan for the
        //     `secret_ref` marker; a literal body that happens to contain the
        //     marker is treated conservatively as exfil — fail-closed.)
        if let Some(body) = &request.body {
            if body_contains_secret_ref(body) {
                let err = CoreError::ValidationError(
                    "ctx.net.fetch denied: a secret_ref may only appear in a header value, not the request body (SC-13 secret-exfil guard)".to_string(),
                );
                self.recorder.record_denial("net.fetch", args, &err)?;
                return Err(err);
            }
        }

        // 2. Egress call gate (SC-5 / CR-4): request-side gates only, decided
        //    BEFORE the request is sent so no request reaches the client on a
        //    deny. Evaluated against the request-phase allowlist (response
        //    constraints stripped) so a rule that caps the *response* does not
        //    spuriously deny here, where the response is still unknown — those
        //    caps are enforced, intact, on the response leg (step 5). A denial is
        //    recorded then propagated.
        let policy_request = to_policy_request(&request);
        if let Err(net_err) =
            NetPolicy::new(&self.net_allowlist_request_phase).check(&policy_request)
        {
            self.recorder.record_denial("net.fetch", args, &net_err)?;
            return Err(net_err);
        }

        // 3. Host-call budget (SC-2): only a permitted fetch consumes a slot.
        self.net_calls_used = self.net_calls_used.saturating_add(1);
        if self.net_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.net.fetch flood)",
                self.limits.max_host_calls
            )));
        }

        // 4. Record/replay (CR-8) + secret injection (SC-13). The `args` recorded
        //    into the trace are the request AS THE APPLET BUILT IT — every
        //    secret-bearing header still carries only its `{ secret_ref }`, never a
        //    resolved value. INSIDE the closure (record mode only) the host
        //    resolves each secret_ref against the bridge's secret store and hands
        //    the RESOLVED, literal-only request to the client. So:
        //      * the client/bridge sees the real header value;
        //      * the RECORDING (`args`) keeps only the secret_ref;
        //      * replay serves the recorded response and never resolves a secret
        //        (the closure does not run), so replay needs no secret store.
        //    The secret-header/destination allowlist gate already ran at step 2
        //    (the policy permits a secret_ref header only where the matched rule
        //    lists it in `allow_secret_headers`); an unknown/revoked secret name is
        //    a fail-closed `RuntimeError` here and no request is sent.
        let bridge = &mut *self.bridge;
        let req = request.clone();
        let response_json = self.recorder.host_call("net.fetch", args, || {
            // Resolve secret_ref headers to plaintext only now, at the HTTP edge,
            // into a fresh literal-only request the client receives. `req` (the
            // recorded shape) is untouched, so the trace keeps the secret_ref.
            let injected = resolve_secret_headers(&req, bridge.secret_store())?;
            let resp = bridge.net_fetch(injected)?;
            serde_json::to_value(&resp).map_err(|e| {
                CoreError::RuntimeError(format!("net.fetch response serialize failed: {e}"))
            })
        })?;
        // On REPLAY the recorder serves the recorded response. A response-leg
        // denial was REDACTED into `{"denied": <CoreError>}` (step 5 below /
        // `redact_last_response`), so the recorded entry for a denied fetch is that
        // shape — NOT a `NetResponse`. Reconstruct the original denial here and
        // surface it, so replay reports the SAME error byte-identically instead of
        // failing to decode the redacted entry as a `NetResponse` (review 077). A
        // real recorded response is a full `NetResponse` (always carries `status`),
        // so a lone `"denied"` key is unambiguously the redaction shape.
        if let Some(denied) = response_json.get("denied") {
            if response_json.as_object().is_some_and(|o| o.len() == 1) {
                let err: CoreError = serde_json::from_value(denied.clone()).map_err(|e| {
                    CoreError::RuntimeError(format!(
                        "net.fetch recorded denial decode failed: {e}"
                    ))
                })?;
                return Err(err);
            }
        }
        let response = serde_json::from_value::<NetResponse>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("net.fetch response decode failed: {e}"))
        })?;

        // 5. Response-leg egress check (SC-5 response caps + redirect/DNS facts):
        //    the call-gate check above could only see the *request* — the response
        //    size/content-type, the redirect hops actually followed, and the
        //    resolved DNS answers do not exist until the fetch returns. Re-run the
        //    SAME `NetPolicy` against the populated response (size/content-type +
        //    `redirect_chain` + `dns_answers` reported by the client) before the
        //    body is served to the applet, so:
        //      * an over-cap / wrong-content-type response is denied;
        //      * a redirect to a private IP or an unallowlisted public host is
        //        denied (every hop is re-checked against the request-side gates);
        //      * a host that resolves to a private literal address (DNS rebinding)
        //        is denied.
        //    This re-check runs on **both** record and replay (the recorded
        //    response is policy-bound too: a tampered/oversized/rebinding recording
        //    is denied identically on replay), and is fail-closed — a violating
        //    response surfaces as the run's `CoreError` and never reaches the applet.
        //
        //    TRACE-SAFETY (review 074 #2 / SC-13): on a response-leg DENIAL the
        //    response captured by `host_call` is REDACTED into a denial-shaped
        //    trace entry, so a rejected response body — and any value that a
        //    secret-bearing request might otherwise expose downstream — never
        //    persists in the RunRecord. The call's recorded `args` still carry only
        //    the request's `secret_ref` (never a resolved value).
        let response_policy_request = to_response_policy_request(&request, &response);
        if let Err(net_err) = NetPolicy::new(&self.net_allowlist).check(&response_policy_request) {
            self.recorder.redact_last_response(&net_err);
            return Err(net_err);
        }

        Ok(response)
    }

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
        self.files_calls_used = self.files_calls_used.saturating_add(1);
        if self.files_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.files flood)",
                self.limits.max_host_calls
            )));
        }

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
        self.files_calls_used = self.files_calls_used.saturating_add(1);
        if self.files_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.files flood)",
                self.limits.max_host_calls
            )));
        }

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

/// Project the runtime's [`NetRequest`] onto the [`forge_policy::NetRequest`] the
/// egress [`NetPolicy`] evaluates **at the call gate**. The runtime carries the
/// *wire* request (method/url/headers/body/content-type/timeout); the policy
/// needs the match-relevant fields plus the declared body size for the SC-5 size
/// cap. The response-size/content-type caps cannot be evaluated here (the
/// response isn't known yet at the call gate); they are enforced on the response
/// leg by [`to_response_policy_request`] + a second [`NetPolicy`] check after the
/// bridge returns (`net_fetch` step 5). The literal URL/host/scheme/path/method/
/// body-size/timeout/secret-header gates are what this call-time check enforces.
fn to_policy_request(request: &NetRequest) -> forge_policy::NetRequest {
    use forge_policy::HeaderValue;
    forge_policy::NetRequest {
        method: request.method.clone(),
        url: request.url.clone(),
        body_bytes: request.body.as_ref().map(|b| b.len() as u64),
        timeout_ms: request.timeout_ms,
        request_content_type: request.content_type.clone(),
        // Request headers projected onto the policy's header model: a literal
        // string maps to `HeaderValue::Literal` (the policy denies a secret-like
        // header carrying a literal value); a `{ secret_ref }` maps to
        // `HeaderValue::Secret`, which the policy permits ONLY when the matched
        // rule lists that header name in `allow_secret_headers`. So both the
        // literal-secret deny and the secret_ref allowlist gate run at the call
        // gate, before any secret is resolved or any request is sent (SC-13).
        headers: request
            .headers
            .iter()
            .map(|(k, v)| {
                let pv = match v {
                    NetHeaderValue::Literal(s) => HeaderValue::Literal(s.clone()),
                    NetHeaderValue::Secret { secret_ref } => {
                        HeaderValue::Secret { secret_ref: secret_ref.clone() }
                    }
                };
                (k.clone(), pv)
            })
            .collect(),
        ..Default::default()
    }
}

/// Header names treated as secret-bearing for trace redaction. Mirrors the
/// policy's secret-header set (`policy/net.rs`); a *literal* value on one of
/// these must never be persisted into the trace, even on a denied request.
fn is_secret_header_name(name: &str) -> bool {
    const SECRET_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];
    let lower = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&lower.as_str())
}

/// Build the **trace-safe** recorded args for a `net.fetch` (SC-13). Starting
/// from the serialized request, every header whose value is a *literal* on a
/// secret-like header name is replaced with `"<redacted>"`, so a plaintext
/// secret the applet wrote as a literal never enters the RunRecord (even when the
/// request is denied and recorded as a denial). A `{ "secret_ref": "name" }`
/// header is left untouched — it carries only the non-sensitive ref, which the
/// trace is *required* to keep for replay/diagnostics. Non-header fields are
/// unchanged. This is purely the recorded shape; the bridge still receives the
/// original request and resolves/injects real values at the HTTP edge.
fn trace_safe_args(request: &NetRequest) -> serde_json::Value {
    let mut value = serde_json::to_value(request).unwrap_or(serde_json::Value::Null);
    if let Some(headers) = value
        .get_mut("headers")
        .and_then(|h| h.as_object_mut())
    {
        for (name, hv) in headers.iter_mut() {
            // Only redact a LITERAL (a JSON string) on a secret-like header; a
            // secret_ref object stays so the trace keeps the ref.
            if hv.is_string() && is_secret_header_name(name) {
                *hv = serde_json::Value::String("<redacted>".to_string());
            }
        }
    }
    value
}

/// Whether a request body smuggles a secret reference (SC-13 exfil guard). M0a
/// only permits a `secret_ref` in a header value; one in the body is a
/// secret-exfil pattern and the request must be rejected before it is sent.
///
/// The body is opaque to the runtime (an already-serialized string), so this is
/// a conservative textual check: if the body parses as JSON and contains a
/// `secret_ref` object key anywhere, it is exfil; if it does not parse as JSON
/// but still contains the literal `"secret_ref"` token, it is treated as exfil
/// too (fail-closed). A benign body without the marker passes.
fn body_contains_secret_ref(body: &str) -> bool {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if json_has_secret_ref_key(&value) {
            return true;
        }
    }
    body.contains("\"secret_ref\"")
}

/// Recursively scan a JSON value for an object that has a `secret_ref` key.
fn json_has_secret_ref_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key("secret_ref") || map.values().any(json_has_secret_ref_key)
        }
        serde_json::Value::Array(items) => items.iter().any(json_has_secret_ref_key),
        _ => false,
    }
}

/// Build the **request-phase** view of a net allowlist: the same rules in the
/// same order, but with each rule's *response* constraints (`max_response_bytes`,
/// `response_content_types`) cleared. The call gate checks against this view so a
/// rule that constrains the response cannot spuriously deny a request before its
/// response is known (the policy denies an unknown response content-type). The
/// response constraints are preserved in the full allowlist and enforced on the
/// response leg. All request-side fields (host/scheme/path/method/body/timeout/
/// request-content-type/secret-headers) are untouched, so the call gate's
/// request-side decision is identical to the full allowlist's.
fn request_phase_allowlist(full: &NetGrant) -> NetGrant {
    NetGrant(
        full
            .rules()
            .iter()
            .map(|rule| forge_domain::NetRule {
                max_response_bytes: None,
                response_content_types: Vec::new(),
                ..rule.clone()
            })
            .collect(),
    )
}

/// Project the request **plus the real response** onto a [`forge_policy::NetRequest`]
/// for the **response-leg** egress check (SC-5 response caps + redirect/DNS facts).
/// This is the call-gate projection ([`to_policy_request`]) with the now-known
/// facts populated from the [`NetResponse`], so a second [`NetPolicy`] check matches
/// the **same** allowlist rule (host/scheme/path/method must still match) and
/// additionally enforces:
///   * `max_response_bytes` / `response_content_types` against the real body size
///     and content-type;
///   * the SC-5 redirect re-check against the `redirect_chain` the client actually
///     followed — every hop must independently satisfy a rule's request-side gates,
///     so a redirect to a private IP or an unallowlisted public host is denied;
///   * the SC-5 private-network deny against the `dns_answers` the host resolved —
///     a host resolving to a private literal address (DNS rebinding) is denied.
///
/// These redirect/DNS facts exist only on the response (they are products of the
/// transport, not the request), so they can only be checked here on the response
/// leg, never at the call gate. Reusing the same projection keeps the rule
/// selection identical to the call gate; only the response-derived facts are added.
fn to_response_policy_request(
    request: &NetRequest,
    response: &NetResponse,
) -> forge_policy::NetRequest {
    // Fold `final_url` (the URL the response actually came from) into the
    // policy-checked redirect hops so it is **bound to the allowlist** even when a
    // client reports it with an empty or truncated `redirect_chain` (review 074 #1).
    // Without this, a client that follows redirects and returns
    // `final_url = https://evil.example.net/...` but no chain would pass the
    // response-leg redirect check. Appending it (when it isn't already the last
    // hop) makes the policy re-run the full request-side gates against the real
    // final destination — fail-closed.
    let mut redirect_chain = response.redirect_chain.clone();
    if let Some(final_url) = &response.final_url {
        if redirect_chain.last() != Some(final_url) {
            redirect_chain.push(final_url.clone());
        }
    }
    forge_policy::NetRequest {
        response_bytes: Some(response.body_bytes()),
        response_content_type: response.content_type.clone(),
        redirect_chain,
        dns_answers: response.dns_answers.clone(),
        ..to_policy_request(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::MemoryHostBridge;
    use crate::recorder::RunRecorder;
    use forge_domain::{Capabilities, Limits, NetGrant, NetRule};

    fn manifest_with_net(net: NetGrant, max_host_calls: u64) -> Manifest {
        Manifest {
            entrypoint: "main.ts".into(),
            min_api: "forge-api@0.1".into(),
            deterministic: true,
            capabilities: Capabilities { net, ..Capabilities::default() },
            limits: Limits { max_host_calls, ..Limits::default() },
        }
    }

    fn get_rule(url: &str) -> NetRule {
        NetRule { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    fn req(url: &str) -> NetRequest {
        NetRequest { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    /// An allowed fetch returns the bridge's (mock) response and records the call.
    #[test]
    fn net_fetch_allowed_records_and_serves_mock() {
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host.net_fetch(req("https://api.example.com/public/weather")).unwrap();
        assert_eq!(resp.status, 200);
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "net.fetch");
    }

    /// A non-allowlisted host is denied; the bridge is never touched and the
    /// denial is recorded as the run's `{"denied": …}` entry.
    #[test]
    fn net_fetch_denied_does_not_reach_bridge_and_is_recorded() {
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host.net_fetch(req("https://evil.example.com/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let calls = recorder.into_calls();
        // The denial is in the trace as a recorded `net.fetch` denial.
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "net.fetch");
        assert!(calls[0].response.get("denied").is_some());
        // And the bridge never saw a request.
        assert!(bridge.net_requests.is_empty());
    }

    /// An empty allowlist surfaces CapabilityRequired (category not declared).
    #[test]
    fn net_fetch_without_grant_is_capability_required() {
        let manifest = manifest_with_net(NetGrant::default(), 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host.net_fetch(req("https://api.example.com/x")).unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
        assert!(bridge.net_requests.is_empty());
    }

    // --- Response-leg SC-5 caps (max_response_bytes / response_content_types) --

    use crate::net::MockHttpClient;

    /// A rule with a response cap whose response, when it comes back, exceeds the
    /// cap is denied — the over-cap body is NOT served to the applet (SC-5
    /// max_response_bytes enforced on the response leg, not just at the call gate).
    #[test]
    fn net_fetch_oversized_response_is_denied_and_not_served() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(8);
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        // Inject a client whose response body is 16 bytes — over the 8-byte cap.
        let big = NetResponse {
            status: 200,
            body: Some("0123456789abcdef".into()), // 16 bytes > 8
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let mut bridge =
            MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(big)));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/weather"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("response"), "{err}");
        assert!(err.to_string().contains("max_response_bytes"), "{err}");
    }

    /// A rule constraining response content-types denies a response whose
    /// content-type is outside the set (SC-5 response_content_types on the
    /// response leg). The wrong-type body never reaches the applet.
    #[test]
    fn net_fetch_wrong_response_content_type_is_denied() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.response_content_types = vec!["application/json".into()];
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        let html = NetResponse {
            status: 200,
            body: Some("<html></html>".into()),
            content_type: Some("text/html".into()),
            ..Default::default()
        };
        let mut bridge =
            MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(html)));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/page"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("content-type"), "{err}");
    }

    /// A response within the rule's caps is served unchanged: the new response-leg
    /// check must not over-deny a compliant response (positive control).
    #[test]
    fn net_fetch_response_within_caps_is_served() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(64);
        rule.response_content_types = vec!["application/json".into()];
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        // Default canned mock: 11-byte `{"ok":true}` JSON body, application/json.
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .net_fetch(req("https://api.example.com/public/weather"))
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_deref(), Some(r#"{"ok":true}"#));
    }

    /// The response-leg cap is enforced on **replay** too: a recorded response
    /// that violates a response cap is denied identically when replayed (the
    /// recording is policy-bound; a tampered/oversized recording cannot smuggle a
    /// body past SC-5). We replay a recording whose `net.fetch` response is over
    /// the rule's cap and assert it surfaces the same PermissionDenied.
    #[test]
    fn net_fetch_response_cap_is_enforced_on_replay() {
        use crate::bridge::NullBridge;
        use forge_domain::RecordedCall;

        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(8);
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");

        // A recorded trace whose net.fetch response body is 16 bytes (> 8 cap).
        let recorded_resp = NetResponse {
            status: 200,
            body: Some("0123456789abcdef".into()),
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let request = req("https://api.example.com/public/weather");
        let recorded = vec![RecordedCall {
            seq: 0,
            method: "net.fetch".into(),
            args: serde_json::to_value(&request).unwrap(),
            response: serde_json::to_value(&recorded_resp).unwrap(),
        }];

        // Replay must NOT touch a live bridge; NullBridge proves the recorder
        // serves the response, and the response-leg policy check still denies it.
        let mut bridge = NullBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::replaying(1, 0, recorded),
            &mut bridge,
        )
        .unwrap();
        let err = host.net_fetch(request).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("max_response_bytes"), "{err}");
    }

    /// A rule that constrains the response content-type still denies a
    /// request-side violation (wrong host) AT THE CALL GATE — before any request
    /// reaches the client. Proves the request-phase allowlist (response
    /// constraints stripped) keeps every request-side gate intact: the response
    /// caps don't make the call gate either over-deny a good host or under-deny a
    /// bad one.
    #[test]
    fn net_fetch_response_constrained_rule_still_gates_request_side() {
        let mut rule = get_rule("https://api.example.com/public/*");
        rule.max_response_bytes = Some(1024);
        rule.response_content_types = vec!["application/json".into()];
        let manifest = manifest_with_net(NetGrant(vec![rule]), 100);
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        // Wrong host: denied at the call gate, bridge never touched.
        let err = host.net_fetch(req("https://evil.example.com/public/x")).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(bridge.net_requests.is_empty(), "denied request must not reach the client");
    }

    // --- Response-leg SC-5 redirect / DNS facts (review 070 P1 #2) -----------

    /// A redirect whose every hop (origin + final) is allowlisted is allowed: the
    /// client reports the chain on the response and the response-leg check
    /// re-validates each hop, all of which pass.
    #[test]
    fn net_fetch_redirect_through_allowlisted_hop_is_allowed() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![
                get_rule("https://api.example.com/public/*"),
                get_rule("https://cdn.example.com/public/*"),
            ]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // The mock simulates a 302 chain api -> cdn, both allowlisted.
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(MockHttpClient::with_redirects(
            vec![
                "https://api.example.com/public/asset".into(),
                "https://cdn.example.com/public/asset".into(),
            ],
        )));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .net_fetch(req("https://api.example.com/public/asset"))
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.redirect_chain.len(), 2);
    }

    /// A redirect whose final hop is a private IP is denied AFTER the fetch: the
    /// hop is on the response (not the request), so the call gate could not catch
    /// it; the response-leg check denies it and the body never reaches the applet.
    #[test]
    fn net_fetch_redirect_to_private_ip_is_denied_after_fetch() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(MockHttpClient::with_redirects(
            vec![
                "https://api.example.com/public/redirect".into(),
                "http://127.0.0.1/admin".into(),
            ],
        )));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/redirect"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("private"), "{err}");
    }

    /// A redirect to a public-but-unallowlisted host is denied after the fetch:
    /// the hop is not private, but its origin is not in the allowlist, so the
    /// per-hop re-check on the response leg denies it (SC-5 redirect re-check).
    #[test]
    fn net_fetch_redirect_to_unallowlisted_public_host_is_denied_after_fetch() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(MockHttpClient::with_redirects(
            vec![
                "https://api.example.com/public/asset".into(),
                "https://evil.example.net/public/asset".into(),
            ],
        )));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/asset"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("redirect hop"), "{err}");
    }

    /// Review 074 #1: a client that lands on an unallowlisted final URL but reports
    /// an EMPTY `redirect_chain` (only `final_url`) must still be denied — the
    /// response leg folds `final_url` into the policy-checked hops.
    #[test]
    fn net_fetch_final_url_to_unallowlisted_host_is_denied_without_a_chain() {
        use crate::net::{MockHttpClient, NetResponse};
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // Followed a redirect to evil but reports NO chain — only the final_url.
        let response = NetResponse {
            status: 200,
            body: Some(r#"{"ok":true}"#.into()),
            content_type: Some("application/json".into()),
            final_url: Some("https://evil.example.net/public/asset".into()),
            redirect_chain: vec![],
            ..Default::default()
        };
        let mut bridge =
            MemoryHostBridge::with_http_client(Box::new(MockHttpClient::new(response)));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/asset"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied", "final_url must be policy-bound: {err}");
    }

    /// A host that resolves (dns_answers) to a private IP is denied after the
    /// fetch (DNS rebinding): the request URL host is public, so the call gate
    /// allowed it, but the resolved literal address is private and the
    /// response-leg check denies it before the body reaches the applet.
    #[test]
    fn net_fetch_dns_rebinding_to_private_is_denied_after_fetch() {
        use crate::net::MockHttpClient;
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // Public request host, but it resolves to a loopback literal.
        let mut bridge = MemoryHostBridge::with_http_client(Box::new(
            MockHttpClient::with_dns_answers(vec!["127.0.0.1".into()]),
        ));
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(req("https://api.example.com/public/weather"))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("private"), "{err}");
        assert!(err.to_string().contains("DNS answer"), "{err}");
    }

    /// The allowed redirect case records byte-identically and replays unchanged:
    /// record the run, then replay the recorded trace through a NullBridge (no
    /// live network) and assert the served response is identical (redirect/DNS
    /// facts ride on the recording too).
    #[test]
    fn net_fetch_allowed_redirect_replays_byte_identical() {
        use crate::bridge::NullBridge;
        use crate::net::MockHttpClient;

        let manifest = manifest_with_net(
            NetGrant(vec![
                get_rule("https://api.example.com/public/*"),
                get_rule("https://cdn.example.com/public/*"),
            ]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let request = req("https://api.example.com/public/asset");

        // Record.
        let mut rec_bridge = MemoryHostBridge::with_http_client(Box::new(
            MockHttpClient::with_redirects(vec![
                "https://api.example.com/public/asset".into(),
                "https://cdn.example.com/public/asset".into(),
            ]),
        ));
        let mut rec_host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut rec_bridge,
        )
        .unwrap();
        let recorded_resp = rec_host.net_fetch(request.clone()).unwrap();
        let (recorder, _logs) = rec_host.finish();
        let calls = recorder.into_calls();
        assert_eq!(calls.len(), 1);

        // Replay the recorded trace; NullBridge proves no live network is touched
        // and the response-leg policy check still passes for the allowed chain.
        let mut replay_bridge = NullBridge::new();
        let mut replay_host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::replaying(1, 0, calls),
            &mut replay_bridge,
        )
        .unwrap();
        let replayed_resp = replay_host.net_fetch(request).unwrap();
        assert_eq!(recorded_resp, replayed_resp);
        assert_eq!(replayed_resp.redirect_chain.len(), 2);
    }

    /// `net.fetch` counts against the host-call flood cap (SC-2): the (n+1)th
    /// allowed fetch over `max_host_calls` trips ResourceLimitExceeded.
    #[test]
    fn net_fetch_counts_against_host_call_budget() {
        let manifest = manifest_with_net(
            NetGrant(vec![get_rule("https://api.example.com/public/*")]),
            1,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        assert!(host.net_fetch(req("https://api.example.com/public/a")).is_ok());
        let err = host
            .net_fetch(req("https://api.example.com/public/b"))
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
    }

    // --- SC-13: ctx.secrets header injection ---------------------------------

    use crate::net::{InMemorySecretStore, NetHeaderValue};

    /// A GET rule that allows a `secret_ref` into the named header.
    fn secret_rule(url: &str, header: &str) -> NetRule {
        NetRule {
            method: "GET".into(),
            url: url.into(),
            allow_secret_headers: vec![header.into()],
            ..Default::default()
        }
    }

    /// A GET request carrying a `secret_ref` in `header`.
    fn secret_req(url: &str, header: &str, secret_ref: &str) -> NetRequest {
        let mut r = req(url);
        r.headers.insert(
            header.into(),
            NetHeaderValue::Secret { secret_ref: secret_ref.into() },
        );
        r
    }

    /// An allowlisted secret header is injected: a capturing client sees the
    /// RESOLVED value, but the RunRecord trace + the applet's returned response
    /// carry only the secret_ref, never the plaintext (SC-13).
    #[test]
    fn secret_header_is_injected_into_client_but_not_trace_or_return() {
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host
            .net_fetch(secret_req(
                "https://api.example.com/private/me",
                "Authorization",
                "tok",
            ))
            .unwrap();

        // The applet's returned response carries no secret value.
        let resp_json = serde_json::to_string(&resp).unwrap();
        assert!(!resp_json.contains("SECRET-XYZ"), "applet return leaked the secret: {resp_json}");

        // The RECORDED trace keeps only the secret_ref — never the value.
        let (recorder, logs) = host.finish();
        let calls = recorder.into_calls();

        // The CLIENT received the resolved literal header value (injection happened
        // at the HTTP edge). Read after `finish()` releases the &mut bridge borrow.
        assert_eq!(bridge.net_requests.len(), 1);
        let sent = &bridge.net_requests[0];
        assert_eq!(
            sent.headers.get("Authorization").and_then(|h| h.as_literal()),
            Some("Bearer SECRET-XYZ"),
            "client must receive the RESOLVED secret value"
        );

        let trace = serde_json::to_string(&calls).unwrap();
        assert!(trace.contains("secret_ref"), "trace must keep the secret_ref: {trace}");
        assert!(trace.contains("\"tok\""), "trace must keep the ref name: {trace}");
        assert!(!trace.contains("SECRET-XYZ"), "trace leaked the secret value: {trace}");
        // And no log line carries it either.
        assert!(!logs.join("\n").contains("SECRET-XYZ"), "logs leaked the secret");
    }

    /// A secret_ref on a header the matched rule does NOT allowlist is denied at
    /// the call gate; nothing reaches the client and no value is resolved.
    #[test]
    fn secret_header_not_allowlisted_is_denied_before_send() {
        // Rule allowlists "Authorization" but the request uses "X-Api-Key".
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req(
                "https://api.example.com/private/me",
                "X-Api-Key",
                "tok",
            ))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let trace = serde_json::to_string(&recorder.into_calls()).unwrap();
        assert!(!trace.contains("SECRET-XYZ"), "trace leaked the secret value: {trace}");
        assert!(bridge.net_requests.is_empty(), "denied secret header must not send");
    }

    /// An allowlisted header whose secret NAME is unknown to the store is a
    /// fail-closed RuntimeError; nothing is sent and the value never exists.
    #[test]
    fn unknown_secret_name_is_runtime_error_and_sends_nothing() {
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        // The store is EMPTY: the named secret cannot be resolved.
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::canned()),
            InMemorySecretStore::new(),
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req(
                "https://api.example.com/private/me",
                "Authorization",
                "missing",
            ))
            .unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
        assert!(err.to_string().contains("missing"), "error names the ref: {err}");
        drop(host); // release the &mut bridge borrow before reading the bridge
        assert!(
            bridge.net_requests.is_empty(),
            "an unresolvable secret must not reach the client"
        );
    }

    /// A `secret_ref` smuggled into the request BODY is rejected as a
    /// ValidationError (secret-exfil guard) before any policy/bridge work.
    #[test]
    fn secret_ref_in_body_is_rejected() {
        let manifest = manifest_with_net(
            NetGrant(vec![NetRule {
                method: "POST".into(),
                url: "https://api.example.com/report".into(),
                ..Default::default()
            }]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let request = NetRequest {
            method: "POST".into(),
            url: "https://api.example.com/report".into(),
            body: Some(r#"{"token":{"secret_ref":"tok"}}"#.into()),
            ..Default::default()
        };
        let err = host.net_fetch(request).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        drop(host); // release the &mut bridge borrow before reading the bridge
        assert!(bridge.net_requests.is_empty(), "a body secret_ref must not send");
    }

    /// A *literal* value on a secret-like header is denied by policy AND redacted
    /// from the recorded trace, so the plaintext never persists even on a denial.
    #[test]
    fn literal_secret_header_is_redacted_from_the_trace() {
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let mut bridge = MemoryHostBridge::new();
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let mut request = req("https://api.example.com/private/me");
        request.headers.insert(
            "Authorization".into(),
            NetHeaderValue::Literal("Bearer LITERAL-LEAK".into()),
        );
        let err = host.net_fetch(request).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let trace = serde_json::to_string(&recorder.into_calls()).unwrap();
        assert!(!trace.contains("LITERAL-LEAK"), "literal secret leaked into trace: {trace}");
        assert!(trace.contains("<redacted>"), "secret-like literal must be redacted: {trace}");
    }

    /// A response-leg denial (e.g. a redirect to an unallowlisted host) after a
    /// secret was injected must NOT persist the rejected response body or the
    /// secret value; the recorded entry is redacted to a denial (review 074 #2).
    #[test]
    fn response_leg_denial_after_injection_is_trace_safe() {
        use crate::net::NetResponse;
        let manifest = manifest_with_net(
            NetGrant(vec![secret_rule("https://api.example.com/private/*", "Authorization")]),
            100,
        );
        let actor = ActorContext::owner("dev");
        let secrets = InMemorySecretStore::new().with_secret("tok", "Bearer SECRET-XYZ");
        // The transport followed a redirect to an UNALLOWLISTED host and returns a
        // body that must not be recorded once the response-leg policy denies.
        let transport = NetResponse {
            status: 200,
            body: Some("REJECTED-BODY".into()),
            content_type: Some("application/json".into()),
            final_url: Some("https://evil.example/collect".into()),
            redirect_chain: vec![
                "https://api.example.com/private/redirect".into(),
                "https://evil.example/collect".into(),
            ],
            ..Default::default()
        };
        let mut bridge = MemoryHostBridge::with_http_and_secrets(
            Box::new(MockHttpClient::new(transport)),
            secrets,
        );
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let err = host
            .net_fetch(secret_req(
                "https://api.example.com/private/redirect",
                "Authorization",
                "tok",
            ))
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        let (recorder, _logs) = host.finish();
        let trace = serde_json::to_string(&recorder.into_calls()).unwrap();
        // The secret_ref survives (the request was recorded) but neither the
        // resolved value nor the rejected response body persists.
        assert!(trace.contains("secret_ref"), "trace must keep the secret_ref: {trace}");
        assert!(!trace.contains("SECRET-XYZ"), "trace leaked the secret value: {trace}");
        assert!(!trace.contains("REJECTED-BODY"), "trace persisted the rejected body: {trace}");
    }

    // --- CR-3: ctx.files sandboxed file capability + record/replay -----------

    use crate::files::{
        FileReadRequest, FileWriteRequest, InMemoryFileSystem, SandboxFile,
    };
    use forge_domain::{FileRule, FilesGrant};

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
