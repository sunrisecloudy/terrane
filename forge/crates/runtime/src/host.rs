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
use crate::recorder::RunRecorder;
use forge_domain::{ActorContext, CoreError, Limits, Manifest, PermissionSnapshot, Result};
use forge_policy::{Access, HostCall, PolicyEngine};

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
        HostContext {
            policy,
            recorder,
            bridge,
            limits,
            log_bytes_used: 0,
            storage_bytes_used: 0,
            log_calls_used: 0,
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
        query: serde_json::Value,
    ) -> Result<serde_json::Value> {
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
