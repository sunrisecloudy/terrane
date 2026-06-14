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
use crate::net::{resolve_secret_headers, NetHeaderValue, NetRequest, NetResponse};
use crate::recorder::RunRecorder;
use forge_domain::{ActorContext, CoreError, Limits, Manifest, NetGrant, PermissionSnapshot, Result};
use forge_policy::{Access, HostCall, NetPolicy, PolicyEngine};

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
        let net_allowlist = policy.snapshot().capabilities.net;
        let net_allowlist_request_phase = request_phase_allowlist(&net_allowlist);
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

    /// Run the policy check for `call`; on a denial, record the denied attempt
    /// into the trace (so it survives into the [`RunRecord`], review 009 P1 CR-9)
    /// and then propagate the error. `method`/`args` describe the call as the
    /// recorder logs it.
    ///
    /// Recording the denial can itself fail in replay mode (a method/args
    /// mismatch against the recorded denial) — that divergence takes precedence
    /// and is surfaced instead of the original policy error.
    fn check_or_record_denial(
        &mut self,
        call: &HostCall,
        method: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        match self.policy.check(call) {
            Ok(()) => Ok(()),
            Err(policy_err) => {
                self.recorder
                    .record_denial(method, args.clone(), &policy_err)?;
                Err(policy_err)
            }
        }
    }

    // --- Deterministic seams (policy-checked, recorded) ------------------

    /// `ctx.time.now()` — checked against the (always-granted) Time category,
    /// counted against `max_host_calls`, served by the seeded logical clock.
    pub fn now(&mut self) -> Result<i64> {
        self.check_or_record_denial(&HostCall::Time, "time.now", &serde_json::Value::Null)?;
        self.recorder.now()
    }

    /// `ctx.random.next()` — checked against Random, counted, served by the
    /// seeded RNG.
    pub fn random_next(&mut self) -> Result<f64> {
        self.check_or_record_denial(&HostCall::Random, "random.next", &serde_json::Value::Null)?;
        self.recorder.random_next()
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

    // --- UI event dispatch (recorded, replay-bound) ---------------------

    /// Record (or replay) a **dispatched UI event** (prd-merged/05 UI-4,
    /// prd-merged/01 CR-6): the `(action_ref, payload)` that addressed a handler,
    /// plus the `result` the dispatch produced (the handler's final UI tree /
    /// returned value). The individual `ctx.ui.render` calls a handler makes are
    /// already captured as `ui.render` effects; this records the *dispatch
    /// envelope* so a session replays the same event sequence byte-identically.
    ///
    /// On replay the recorder serves the recorded result and asserts the
    /// `action_ref`+`payload` match the recording (a diverging event/payload/order
    /// is a determinism `RuntimeError`). This is *not* a policy-gated host call —
    /// the dispatch itself touches no user data; the effects inside the handler
    /// are gated as usual. It is, however, counted toward the trace order so the
    /// `replay_fingerprint` covers every dispatched event.
    pub fn dispatch_event(
        &mut self,
        action_ref: &str,
        payload: serde_json::Value,
        result: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.recorder.dispatch_event(action_ref, payload, result)
    }

    // --- UI (capability-checked, recorded) ------------------------------

    pub fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        let args = serde_json::json!([tree]);
        self.check_or_record_denial(&HostCall::Ui, "ui.render", &args)?;
        let bridge = &mut *self.bridge;
        let t = tree.clone();
        self.recorder.host_call("ui.render", args, || {
            bridge.ui_render(t).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    // --- Log (budget-checked, recorded) ---------------------------------

    /// `ctx.log(line)` — there is no capability gate for logging (it is an
    /// observability sink, not an effect on user data). It is recorded so replay
    /// stays in parity, and bounded by **two** budgets (review 009 P2):
    ///   * the `log_bytes` budget (CR-5) caps total log *volume*; and
    ///   * the `max_host_calls` budget caps the *number* of log calls — an
    ///     empty-string log flood costs zero bytes, so the byte budget alone can
    ///     never stop it, and ctx.log is otherwise outside the policy host-call
    ///     counter. Counting log calls here closes that flood hole.
    pub fn log(&mut self, line: &str) -> Result<()> {
        // Call-count budget first: a flood of empty logs must trip even though it
        // adds no bytes (review 009 P2).
        self.log_calls_used = self.log_calls_used.saturating_add(1);
        if self.log_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.log flood)",
                self.limits.max_host_calls
            )));
        }
        self.log_bytes_used = self.log_bytes_used.saturating_add(line.len() as u64);
        if self.log_bytes_used > self.limits.log_bytes {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "log byte budget exceeded: log_bytes = {} reached",
                self.limits.log_bytes
            )));
        }
        let args = serde_json::json!([line]);
        let bridge = &mut *self.bridge;
        let l = line.to_string();
        self.recorder.host_call("log", args, || {
            bridge.log(&l).map(|()| serde_json::Value::Null)
        })?;
        self.logs.push(line.to_string());
        Ok(())
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
}
