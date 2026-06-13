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
use crate::net::{NetRequest, NetResponse};
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
    /// The network egress allowlist for `ctx.net.fetch` (prd-merged/07 SC-5/SC-8).
    /// Derived from the policy's permission snapshot at construction so it tracks
    /// the *recorded* grants on replay (review 009 P1 CR-9), not the live manifest.
    /// Empty ⇒ no network (the default for every applet).
    net_allowlist: NetGrant,
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
        HostContext {
            policy,
            recorder,
            bridge,
            limits,
            log_bytes_used: 0,
            storage_bytes_used: 0,
            log_calls_used: 0,
            net_allowlist,
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
        let args = serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);

        // 1. Role gate (SC-10): record the denial so it is replayable, then fail.
        if !self.policy.snapshot().can_run {
            let err = CoreError::PermissionDenied(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for net.fetch call".to_string(),
            );
            self.recorder.record_denial("net.fetch", args, &err)?;
            return Err(err);
        }

        // 2. Egress policy (SC-5 / CR-4): a denial is recorded then propagated;
        //    no request reaches the client on a deny.
        let policy_request = to_policy_request(&request);
        if let Err(net_err) = NetPolicy::new(&self.net_allowlist).check(&policy_request) {
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

        // 4. Record/replay (CR-8): record mode captures the bridge's response;
        //    replay serves the recorded JSON and never touches the live bridge.
        let bridge = &mut *self.bridge;
        let req = request.clone();
        let response_json = self.recorder.host_call("net.fetch", args, || {
            let resp = bridge.net_fetch(req)?;
            serde_json::to_value(&resp).map_err(|e| {
                CoreError::RuntimeError(format!("net.fetch response serialize failed: {e}"))
            })
        })?;
        serde_json::from_value::<NetResponse>(response_json).map_err(|e| {
            CoreError::RuntimeError(format!("net.fetch response decode failed: {e}"))
        })
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
/// egress [`NetPolicy`] evaluates. The runtime carries the *wire* request
/// (method/url/headers/body/content-type/timeout); the policy needs the
/// match-relevant fields plus the declared body size for the SC-5 size cap. The
/// response-size/content-type and redirect/DNS checks are evaluated host-side at
/// fetch time (the response isn't known yet at the call gate), so they are not
/// populated here; the literal URL/host/scheme/path/method/body-size/timeout/
/// secret-header gates are what this call-time check enforces.
fn to_policy_request(request: &NetRequest) -> forge_policy::NetRequest {
    use forge_policy::HeaderValue;
    forge_policy::NetRequest {
        method: request.method.clone(),
        url: request.url.clone(),
        body_bytes: request.body.as_ref().map(|b| b.len() as u64),
        timeout_ms: request.timeout_ms,
        request_content_type: request.content_type.clone(),
        // Literal request headers; the policy denies a secret-like header carrying
        // a literal value, so passing them through enforces SC-5 at the call gate.
        headers: request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), HeaderValue::Literal(v.clone())))
            .collect(),
        ..Default::default()
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
}
