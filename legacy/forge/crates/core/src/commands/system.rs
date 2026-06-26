//! `system.describe` / `system.trace` — catalog and run observability through the facade.

use forge_domain::{
    catalog::CommandVisibility, command_namespace, CoreCommand, CoreError, RecordedCall, Result,
    Role,
};
use serde_json::{json, Value};

use crate::catalog::{
    catalog_entries, catalog_version_hash, inner_catalog_entries, parse_visibility_tier,
};
use super::super::WorkspaceCore;

impl WorkspaceCore {
    /// `system.describe` — pure read of the compiled catalog with role/tier filters.
    pub(in crate::workspace) fn cmd_system_describe(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let _ = self;
        let payload = &cmd.payload;
        let max_tier = parse_visibility_tier(payload.get("tier").and_then(|v| v.as_str()))
            .map_err(CoreError::ValidationError)?;
        let include_inner = payload
            .get("include_inner")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let namespace_filter = payload.get("namespace").and_then(|v| v.as_str());
        let names_filter: Option<Vec<&str>> = payload.get("names").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
        });
        let effective_role = match payload.get("for_role").and_then(|v| v.as_str()) {
            Some(role_name) => parse_role(role_name)?,
            None => cmd.actor.role,
        };

        let mut sources: Vec<&forge_domain::CommandDescriptor> = catalog_entries();
        if include_inner {
            sources.extend(inner_catalog_entries().iter());
        }
        let mut commands: Vec<_> = sources
            .iter()
            .copied()
            .filter(|entry| entry.visible_to(effective_role, max_tier))
            .filter(|entry| {
                namespace_filter
                    .map(|ns| command_namespace(entry.name) == ns)
                    .unwrap_or(true)
            })
            .filter(|entry| {
                names_filter
                    .as_ref()
                    .map(|names| names.contains(&entry.name))
                    .unwrap_or(true)
            })
            .map(|entry| entry.to_json())
            .collect();
        commands.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(json!({
            "catalogVersion": catalog_version_hash(),
            "runtimeVersion": env!("CARGO_PKG_VERSION"),
            "commands": commands,
            "roles": [
                "owner",
                "maintainer",
                "editor",
                "runner",
                "viewer",
                "auditor",
                "reviewer",
            ],
            "tiers": ["public", "operator", "admin", "debug"],
            "effectiveRole": role_label(effective_role),
            "maxTier": tier_label(max_tier),
        }))
    }
}

fn parse_role(value: &str) -> Result<Role> {
    match value {
        "owner" => Ok(Role::Owner),
        "maintainer" => Ok(Role::Maintainer),
        "editor" => Ok(Role::Editor),
        "runner" => Ok(Role::Runner),
        "viewer" => Ok(Role::Viewer),
        "auditor" => Ok(Role::Auditor),
        "reviewer" => Ok(Role::Reviewer),
        other => Err(CoreError::ValidationError(format!(
            "system.describe for_role {other:?} is not a known role"
        ))),
    }
}

fn role_label(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Maintainer => "maintainer",
        Role::Editor => "editor",
        Role::Runner => "runner",
        Role::Viewer => "viewer",
        Role::Auditor => "auditor",
        Role::Reviewer => "reviewer",
    }
}

fn tier_label(tier: CommandVisibility) -> &'static str {
    match tier {
        CommandVisibility::Public => "public",
        CommandVisibility::Operator => "operator",
        CommandVisibility::Admin => "admin",
        CommandVisibility::Debug => "debug",
    }
}

impl WorkspaceCore {
    /// `system.trace` — pure read of a stored [`RunRecord`] host-call journal.
    pub(in crate::workspace) fn cmd_system_trace(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let run_id = cmd
            .payload
            .get("run_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::ValidationError("system.trace requires `run_id`".into()))?;
        let since_seq = cmd
            .payload
            .get("since_seq")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let methods_filter: Option<Vec<&str>> = cmd.payload.get("methods").and_then(|v| v.as_array()).map(
            |arr| {
                arr.iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()
            },
        );

        let run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| CoreError::ValidationError(format!("run {run_id} not found")))?;

        let mut calls: Vec<Value> = Vec::new();
        let mut truncated = false;
        for call in &run.calls {
            if call.seq < since_seq {
                continue;
            }
            if let Some(methods) = methods_filter.as_ref() {
                if !methods.iter().any(|method| call.method == *method) {
                    continue;
                }
            }
            calls.push(redacted_call_json(call));
        }

        // Bound response size for operator tooling (replay records can be large).
        const MAX_CALLS: usize = 10_000;
        if calls.len() > MAX_CALLS {
            calls.truncate(MAX_CALLS);
            truncated = true;
        }

        Ok(json!({
            "run_id": run_id,
            "applet_id": run.applet_id,
            "calls": calls,
            "truncated": truncated,
        }))
    }
}

fn redacted_call_json(call: &RecordedCall) -> Value {
    let method = call.method.as_str();
    if method.contains("secret") {
        return json!({
            "seq": call.seq,
            "method": call.method,
            "args": { "redacted": true },
            "response": { "redacted": true },
        });
    }
    if method == "net.fetch" {
        return json!({
            "seq": call.seq,
            "method": call.method,
            "args": redact_net_payload(&call.args),
            "response": redact_net_payload(&call.response),
        });
    }
    json!({
        "seq": call.seq,
        "method": call.method,
        "args": call.args,
        "response": call.response,
    })
}

fn redact_net_payload(value: &Value) -> Value {
    let Some(obj) = value.as_object() else {
        return value.clone();
    };
    let mut out = obj.clone();
    for key in ["body", "request_body", "response_body"] {
        if out.contains_key(key) {
            out.insert(key.to_string(), json!({ "redacted": true }));
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_secret_call_strips_payload() {
        let call = RecordedCall {
            seq: 0,
            method: "secret.store".into(),
            args: json!({ "value": "top-secret" }),
            response: json!({ "ref": "s1" }),
        };
        let out = redacted_call_json(&call);
        assert_eq!(out["args"]["redacted"], true);
        assert_eq!(out["response"]["redacted"], true);
    }

    #[test]
    fn net_fetch_bodies_are_redacted() {
        let call = RecordedCall {
            seq: 1,
            method: "net.fetch".into(),
            args: json!({ "body": "req" }),
            response: json!({ "body": "resp" }),
        };
        let out = redacted_call_json(&call);
        assert_eq!(out["args"]["body"]["redacted"], true);
        assert_eq!(out["response"]["body"]["redacted"], true);
    }
}