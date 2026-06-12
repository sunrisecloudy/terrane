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
use forge_domain::{ActorContext, CoreError, Limits, Manifest, Result};
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
    /// Captured log lines (mirrored into the RunRecord).
    logs: Vec<String>,
}

impl<'b> HostContext<'b> {
    pub fn new(
        manifest: &Manifest,
        actor: &ActorContext,
        recorder: RunRecorder,
        bridge: &'b mut dyn HostBridge,
    ) -> Self {
        HostContext {
            policy: PolicyEngine::new(manifest, actor),
            recorder,
            bridge,
            limits: manifest.limits.clone(),
            log_bytes_used: 0,
            storage_bytes_used: 0,
            logs: Vec::new(),
        }
    }

    /// Consume the context after a run, yielding the recorder (for the trace)
    /// and the captured logs.
    pub fn finish(self) -> (RunRecorder, Vec<String>) {
        (self.recorder, self.logs)
    }

    // --- Deterministic seams (policy-checked, recorded) ------------------

    /// `ctx.time.now()` — checked against the (always-granted) Time category,
    /// counted against `max_host_calls`, served by the seeded logical clock.
    pub fn now(&mut self) -> Result<i64> {
        self.policy.check(&HostCall::Time)?;
        self.recorder.now()
    }

    /// `ctx.random.next()` — checked against Random, counted, served by the
    /// seeded RNG.
    pub fn random_next(&mut self) -> Result<f64> {
        self.policy.check(&HostCall::Random)?;
        self.recorder.random_next()
    }

    // --- Storage (capability-checked, recorded effects) ------------------

    pub fn storage_get(&mut self, key: &str) -> Result<serde_json::Value> {
        self.policy.check(&HostCall::Storage {
            op: Access::Read,
            key: key.to_string(),
        })?;
        let args = serde_json::json!([key]);
        let bridge = &mut *self.bridge;
        let k = key.to_string();
        self.recorder
            .host_call("storage.get", args, || bridge.storage_get(&k))
    }

    pub fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        self.policy.check(&HostCall::Storage {
            op: Access::Write,
            key: key.to_string(),
        })?;
        // Account the written bytes against the storage byte budget (CR-5).
        let value_bytes = serde_json::to_vec(&value).map(|v| v.len()).unwrap_or(0) as u64;
        self.storage_bytes_used = self.storage_bytes_used.saturating_add(value_bytes);
        if self.storage_bytes_used > self.limits.storage_bytes {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "storage byte budget exceeded: storage_bytes = {} reached",
                self.limits.storage_bytes
            )));
        }
        let args = serde_json::json!([key, value]);
        let bridge = &mut *self.bridge;
        let k = key.to_string();
        let v = value.clone();
        self.recorder.host_call("storage.set", args, || {
            bridge.storage_set(&k, v).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    pub fn storage_delete(&mut self, key: &str) -> Result<()> {
        self.policy.check(&HostCall::Storage {
            op: Access::Write,
            key: key.to_string(),
        })?;
        let args = serde_json::json!([key]);
        let bridge = &mut *self.bridge;
        let k = key.to_string();
        self.recorder.host_call("storage.delete", args, || {
            bridge.storage_delete(&k).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    pub fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>> {
        self.policy.check(&HostCall::Storage {
            op: Access::Read,
            key: prefix.to_string(),
        })?;
        let args = serde_json::json!([prefix]);
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
        self.policy.check(&HostCall::Db {
            op: Access::Write,
            collection: collection.to_string(),
        })?;
        let args = serde_json::json!([collection, record]);
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let r = record.clone();
        let resp = self.recorder.host_call("db.insert", args, || {
            Ok(serde_json::json!(bridge.db_insert(&c, r)?))
        })?;
        Ok(resp.as_str().unwrap_or("").to_string())
    }

    pub fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        self.policy.check(&HostCall::Db {
            op: Access::Read,
            collection: collection.to_string(),
        })?;
        let args = serde_json::json!([collection, id]);
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let i = id.to_string();
        self.recorder
            .host_call("db.get", args, || bridge.db_get(&c, &i))
    }

    pub fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        self.policy.check(&HostCall::Db {
            op: Access::Read,
            collection: collection.to_string(),
        })?;
        let args = serde_json::json!([collection]);
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let resp = self.recorder.host_call("db.list", args, || {
            Ok(serde_json::json!(bridge.db_list(&c)?))
        })?;
        Ok(resp.as_array().cloned().unwrap_or_default())
    }

    // --- UI (capability-checked, recorded) ------------------------------

    pub fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        self.policy.check(&HostCall::Ui)?;
        let args = serde_json::json!([tree]);
        let bridge = &mut *self.bridge;
        let t = tree.clone();
        self.recorder.host_call("ui.render", args, || {
            bridge.ui_render(t).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    // --- Log (budget-checked, recorded) ---------------------------------

    /// `ctx.log(line)` — there is no capability gate for logging (it is an
    /// observability sink, not an effect on user data) and it does not consume a
    /// capability category or the `max_host_calls` budget. It is bounded only by
    /// the `log_bytes` budget (CR-5) and is recorded so replay stays in parity.
    pub fn log(&mut self, line: &str) -> Result<()> {
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
