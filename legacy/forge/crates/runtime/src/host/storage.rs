//! `ctx.storage.*` host calls for [`HostContext`]: the capability-checked,
//! recorded key/value storage effects (`storage.get`/`set`/`delete`/`list`).
//!
//! Each call funnels through the shared
//! [`HostContext::check_or_record_denial`](super::HostContext::check_or_record_denial)
//! policy/denial chokepoint, then performs its single effect inside
//! `recorder.host_call(method, args, || bridge_call)` so record/replay stays
//! byte-identical. `storage.set` additionally accounts the written bytes against
//! the shared `storage_bytes` budget (CR-5).

use super::HostContext;
use forge_domain::Result;
use forge_policy::{Access, HostCall};

impl HostContext<'_> {
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
        self.budgets.check_storage_bytes(value_bytes)?;
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
}
