//! Legacy webapp package registry authority (Q8 `package.*`, Phase D12).
//!
//! The core owns version history, the active-version pointer, and status
//! transitions. Shells transport the resulting [`SqlOp`] rows to `platform.sqlite`
//! but must not mutate `app_versions.status` ad hoc.

use forge_domain::{CoreError, PackageAppStatus, PackageVersionStatus, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// KV namespace for the authoritative platform registry snapshot.
pub const PLATFORM_REGISTRY_NS: &str = "__forge/meta";
/// KV key within [`PLATFORM_REGISTRY_NS`].
pub const PLATFORM_REGISTRY_KEY: &str = "platform_registry";

fn deserialize_flexible_i32<'de, D>(deserializer: D) -> std::result::Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Number(number) => number
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| serde::de::Error::custom(format!("invalid i32: {number}"))),
        Value::String(text) => text
            .parse::<i32>()
            .map_err(|e| serde::de::Error::custom(format!("invalid i32 string {text:?}: {e}"))),
        other => Err(serde::de::Error::custom(format!("invalid i32 value: {other}"))),
    }
}

/// One `apps` row in the legacy webapp registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageAppRecord {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_install_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_i32")]
    pub data_version: i32,
}

/// One `app_versions` row in the legacy webapp registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersionRecord {
    pub install_id: String,
    pub app_id: String,
    pub version: String,
    #[serde(default)]
    pub runtime_version: String,
    #[serde(default, deserialize_with = "deserialize_flexible_i32")]
    pub data_version: i32,
    pub status: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
}

/// One `app_installations` audit row emitted by a lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInstallationEvent {
    pub installation_event_id: String,
    pub app_id: String,
    pub install_id: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_install_id: Option<String>,
    pub actor: String,
    pub created_at: String,
    #[serde(default)]
    pub details_json: Value,
}

/// A parameterized SQL statement for the shell to execute on `platform.sqlite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlOp {
    pub sql: String,
    pub args: Vec<Value>,
}

/// Authoritative in-core snapshot of the shell registry tables the `package.*`
/// commands mutate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlatformRegistry {
    #[serde(default)]
    pub apps: BTreeMap<String, PackageAppRecord>,
    #[serde(default)]
    pub versions: BTreeMap<String, PackageVersionRecord>,
    #[serde(default)]
    pub next_event_seq: u64,
}

/// Result of a mutating `package.*` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTransitionResult {
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_install_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back_install_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    pub sql_ops: Vec<SqlOp>,
    pub installation_events: Vec<PackageInstallationEvent>,
    pub audit_actions: Vec<String>,
}

impl PlatformRegistry {
    pub fn list_versions(&self, app_id: &str) -> Result<Value> {
        let app = self.apps.get(app_id);
        let mut versions: Vec<&PackageVersionRecord> =
            self.versions.values().filter(|v| v.app_id == app_id).collect();
        versions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(serde_json::json!({
            "app_id": app_id,
            "active_install_id": app.and_then(|a| a.active_install_id.clone()),
            "active_version": app.and_then(|a| a.active_version.clone()),
            "versions": versions,
        }))
    }

    pub fn activate_version(
        &mut self,
        app_id: &str,
        install_id: &str,
        actor: &str,
        created_at: &str,
        event_id: Option<&str>,
    ) -> Result<PackageTransitionResult> {
        let (target_version, target_data_version, target_status) = {
            let target = self
                .versions
                .get(install_id)
                .ok_or_else(|| CoreError::ValidationError(format!("install not found: {install_id}")))?;
            if target.app_id != app_id {
                return Err(CoreError::ValidationError(format!(
                    "install {install_id} does not belong to app {app_id}"
                )));
            }
            if matches_status(&target.status, PackageVersionStatus::Quarantined)
                || matches_status(&target.status, PackageVersionStatus::Uninstalled)
            {
                return Err(CoreError::ValidationError(format!(
                    "install cannot be activated from status: {}",
                    target.status
                )));
            }
            (
                target.version.clone(),
                target.data_version,
                target.status.clone(),
            )
        };
        let _ = target_status;

        let previous = self.active_version(app_id)?;
        let mut sql_ops = Vec::new();
        let mut events = Vec::new();
        let audit_actions = vec!["package.activate".to_string()];

        if let Some(prev) = &previous {
            if prev.install_id != install_id {
                self.set_version_status(
                    &prev.install_id,
                    PackageVersionStatus::Installed.as_str(),
                )?;
                sql_ops.push(sql_op(
                    "UPDATE app_versions SET status = ? WHERE install_id = ?",
                    [PackageVersionStatus::Installed.as_str(), &prev.install_id],
                ));
            }
        }

        self.set_version_status(install_id, PackageVersionStatus::Enabled.as_str())?;
        self.set_version_activated_at(install_id, created_at)?;
        self.update_app_active(
            app_id,
            install_id,
            &target_version,
            target_data_version,
            created_at,
        )?;

        sql_ops.push(sql_op(
            "UPDATE app_versions SET status = ?, activated_at = ? WHERE install_id = ?",
            [
                PackageVersionStatus::Enabled.as_str(),
                created_at,
                install_id,
            ],
        ));
        sql_ops.push(sql_op(
            "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = ?, updated_at = ? WHERE id = ?",
            [
                install_id,
                &target_version,
                &target_data_version.to_string(),
                PackageAppStatus::Enabled.as_str(),
                created_at,
                app_id,
            ],
        ));

        let event = self.make_event(
            event_id,
            app_id,
            install_id,
            "activate",
            previous.as_ref().map(|p| p.install_id.as_str()),
            actor,
            created_at,
            serde_json::json!({
                "previousInstallId": previous.as_ref().map(|p| &p.install_id),
            }),
        );
        events.push(event.clone());
        sql_ops.push(installation_event_sql(&event));

        Ok(PackageTransitionResult {
            app_id: app_id.to_string(),
            active_install_id: Some(install_id.to_string()),
            rolled_back_install_id: None,
            active_version: Some(target_version),
            sql_ops,
            installation_events: events,
            audit_actions,
        })
    }

    pub fn rollback_version(
        &mut self,
        app_id: &str,
        target_install_id: Option<&str>,
        actor: &str,
        created_at: &str,
        event_id: Option<&str>,
    ) -> Result<PackageTransitionResult> {
        let current = self
            .active_version(app_id)?
            .ok_or(PackageRegistryError::AppNotInstalled)?;
        let target = if let Some(id) = target_install_id {
            self.version_by_install(id)?
                .filter(|v| v.app_id == app_id)
                .ok_or_else(|| CoreError::ValidationError(format!("rollback target not found: {id}")))?
        } else {
            self.rollback_target(app_id, &current.install_id)?
                .ok_or(PackageRegistryError::NoRollbackTarget)?
        };
        if target.install_id == current.install_id {
            return Err(PackageRegistryError::NoRollbackTarget.into());
        }
        if target.data_version != current.data_version {
            return Err(PackageRegistryError::RollbackDataVersionIncompatible.into());
        }

        let mut sql_ops = Vec::new();
        self.set_version_status(
            &current.install_id,
            PackageVersionStatus::RolledBack.as_str(),
        )?;
        self.set_version_status(&target.install_id, PackageVersionStatus::Enabled.as_str())?;
        self.set_version_activated_at(&target.install_id, created_at)?;
        self.update_app_active(
            app_id,
            &target.install_id,
            &target.version,
            target.data_version,
            created_at,
        )?;

        sql_ops.push(sql_op(
            "UPDATE app_versions SET status = ? WHERE install_id = ?",
            [PackageVersionStatus::RolledBack.as_str(), &current.install_id],
        ));
        sql_ops.push(sql_op(
            "UPDATE app_versions SET status = ?, activated_at = ? WHERE install_id = ?",
            [
                PackageVersionStatus::Enabled.as_str(),
                created_at,
                &target.install_id,
            ],
        ));
        sql_ops.push(sql_op(
            "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = ?, updated_at = ? WHERE id = ?",
            [
                &target.install_id,
                &target.version,
                &target.data_version.to_string(),
                PackageAppStatus::Enabled.as_str(),
                created_at,
                app_id,
            ],
        ));

        let event = self.make_event(
            event_id,
            app_id,
            &target.install_id,
            "rollback",
            Some(&current.install_id),
            actor,
            created_at,
            serde_json::json!({
                "targetInstallId": target.install_id,
                "rolledBackInstallId": current.install_id,
            }),
        );
        sql_ops.push(installation_event_sql(&event));

        Ok(PackageTransitionResult {
            app_id: app_id.to_string(),
            active_install_id: Some(target.install_id.clone()),
            rolled_back_install_id: Some(current.install_id.clone()),
            active_version: Some(target.version.clone()),
            sql_ops,
            installation_events: vec![event],
            audit_actions: vec!["package.rollback".to_string()],
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_status(
        &mut self,
        app_id: &str,
        install_id: &str,
        status: &str,
        actor: &str,
        created_at: &str,
        reason: Option<&str>,
        restore_previous: bool,
        event_id: Option<&str>,
    ) -> Result<PackageTransitionResult> {
        {
            let version = self
                .versions
                .get(install_id)
                .ok_or_else(|| CoreError::ValidationError(format!("install not found: {install_id}")))?;
            if version.app_id != app_id {
                return Err(CoreError::ValidationError(format!(
                    "install {install_id} does not belong to app {app_id}"
                )));
            }
        }
        let is_active = self
            .apps
            .get(app_id)
            .and_then(|a| a.active_install_id.as_deref())
            .map(|id| id == install_id)
            .unwrap_or(false);
        let mut active_install_id = self
            .apps
            .get(app_id)
            .and_then(|a| a.active_install_id.clone());
        let mut active_version = self.apps.get(app_id).and_then(|a| a.active_version.clone());

        let mut sql_ops = Vec::new();
        let mut events = Vec::new();
        let mut audit_actions = vec!["package.set_status".to_string()];
        let mut rolled_back_install_id = None;

        self.set_version_status(install_id, status)?;
        sql_ops.push(sql_op(
            "UPDATE app_versions SET status = ? WHERE app_id = ? AND install_id = ?",
            [status, app_id, install_id],
        ));

        if status == PackageVersionStatus::Quarantined.as_str() && restore_previous && is_active {
            if let Some(restore) = self.rollback_target(app_id, install_id)? {
                self.set_version_status(
                    &restore.install_id,
                    PackageVersionStatus::Enabled.as_str(),
                )?;
                self.set_version_activated_at(&restore.install_id, created_at)?;
                self.update_app_active(
                    app_id,
                    &restore.install_id,
                    &restore.version,
                    restore.data_version,
                    created_at,
                )?;
                sql_ops.push(sql_op(
                    "UPDATE app_versions SET status = ?, activated_at = ? WHERE install_id = ?",
                    [
                        PackageVersionStatus::Enabled.as_str(),
                        created_at,
                        &restore.install_id,
                    ],
                ));
                sql_ops.push(sql_op(
                    "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = ?, updated_at = ? WHERE id = ?",
                    [
                        &restore.install_id,
                        &restore.version,
                        &restore.data_version.to_string(),
                        PackageAppStatus::Enabled.as_str(),
                        created_at,
                        app_id,
                    ],
                ));
                let rollback_event = self.make_event(
                    None,
                    app_id,
                    &restore.install_id,
                    "rollback",
                    Some(install_id),
                    actor,
                    created_at,
                    serde_json::json!({
                        "reason": "automatic rollback after quarantine",
                        "quarantinedInstallId": install_id,
                    }),
                );
                events.push(rollback_event.clone());
                sql_ops.push(installation_event_sql(&rollback_event));
                audit_actions.push("package.rollback".to_string());
                active_install_id = Some(restore.install_id.clone());
                active_version = Some(restore.version.clone());
                rolled_back_install_id = Some(install_id.to_string());
            }
        } else if status == PackageVersionStatus::Quarantined.as_str() && is_active {
            if let Some(app_row) = self.apps.get_mut(app_id) {
                app_row.status = PackageAppStatus::Quarantined.as_str().to_string();
                app_row.active_install_id = None;
                app_row.active_version = None;
            }
            sql_ops.push(sql_op(
                "UPDATE apps SET status = ?, updated_at = ? WHERE id = ?",
                [PackageAppStatus::Quarantined.as_str(), created_at, app_id],
            ));
            active_install_id = None;
            active_version = None;
        }

        let quarantine_event = self.make_event(
            event_id,
            app_id,
            install_id,
            if status == PackageVersionStatus::Quarantined.as_str() {
                "quarantine"
            } else {
                "set_status"
            },
            active_install_id
                .as_deref()
                .filter(|id| *id != install_id)
                .or(rolled_back_install_id.as_deref()),
            actor,
            created_at,
            serde_json::json!({
                "reason": reason,
                "status": status,
                "restoredInstallId": active_install_id,
            }),
        );
        events.push(quarantine_event.clone());
        sql_ops.push(installation_event_sql(&quarantine_event));
        if status == PackageVersionStatus::Quarantined.as_str() {
            audit_actions.push("package.quarantine".to_string());
        }

        Ok(PackageTransitionResult {
            app_id: app_id.to_string(),
            active_install_id,
            rolled_back_install_id,
            active_version,
            sql_ops,
            installation_events: events,
            audit_actions,
        })
    }

    fn active_version(&self, app_id: &str) -> Result<Option<PackageVersionRecord>> {
        let Some(app) = self.apps.get(app_id) else {
            return Ok(None);
        };
        if app.status != PackageAppStatus::Enabled.as_str() {
            return Ok(None);
        }
        let Some(active_id) = &app.active_install_id else {
            return Ok(None);
        };
        Ok(self.versions.get(active_id).cloned())
    }

    fn rollback_target(&self, app_id: &str, excluding: &str) -> Result<Option<PackageVersionRecord>> {
        let mut candidates: Vec<&PackageVersionRecord> = self
            .versions
            .values()
            .filter(|v| {
                v.app_id == app_id
                    && v.install_id != excluding
                    && v.status != PackageVersionStatus::Quarantined.as_str()
                    && v.status != PackageVersionStatus::Uninstalled.as_str()
            })
            .collect();
        candidates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(candidates.first().map(|v| (*v).clone()))
    }

    fn version_by_install(&self, install_id: &str) -> Result<Option<PackageVersionRecord>> {
        if install_id.is_empty() {
            return Ok(None);
        }
        if matches_status(
            self.versions
                .get(install_id)
                .map(|v| v.status.as_str())
                .unwrap_or(""),
            PackageVersionStatus::Quarantined,
        ) || matches_status(
            self.versions
                .get(install_id)
                .map(|v| v.status.as_str())
                .unwrap_or(""),
            PackageVersionStatus::Uninstalled,
        ) {
            return Ok(None);
        }
        Ok(self.versions.get(install_id).cloned())
    }

    fn set_version_status(&mut self, install_id: &str, status: impl Into<String>) -> Result<()> {
        let version = self
            .versions
            .get_mut(install_id)
            .ok_or_else(|| CoreError::ValidationError(format!("install not found: {install_id}")))?;
        version.status = status.into();
        Ok(())
    }

    fn set_version_activated_at(&mut self, install_id: &str, activated_at: &str) -> Result<()> {
        let version = self
            .versions
            .get_mut(install_id)
            .ok_or_else(|| CoreError::ValidationError(format!("install not found: {install_id}")))?;
        version.activated_at = Some(activated_at.to_string());
        Ok(())
    }

    fn update_app_active(
        &mut self,
        app_id: &str,
        install_id: &str,
        version: &str,
        data_version: i32,
        updated_at: &str,
    ) -> Result<()> {
        let app = self
            .apps
            .entry(app_id.to_string())
            .or_insert_with(|| PackageAppRecord {
                id: app_id.to_string(),
                name: app_id.to_string(),
                status: PackageAppStatus::Enabled.as_str().to_string(),
                active_install_id: None,
                active_version: None,
                data_version,
            });
        app.status = PackageAppStatus::Enabled.as_str().to_string();
        app.active_install_id = Some(install_id.to_string());
        app.active_version = Some(version.to_string());
        app.data_version = data_version;
        let _ = updated_at;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn make_event(
        &mut self,
        event_id: Option<&str>,
        app_id: &str,
        install_id: &str,
        action: &str,
        previous_install_id: Option<&str>,
        actor: &str,
        created_at: &str,
        details_json: Value,
    ) -> PackageInstallationEvent {
        let id = event_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("event-{}-{}", app_id, self.next_event_seq));
        self.next_event_seq += 1;
        PackageInstallationEvent {
            installation_event_id: id,
            app_id: app_id.to_string(),
            install_id: install_id.to_string(),
            action: action.to_string(),
            previous_install_id: previous_install_id.map(str::to_string),
            actor: actor.to_string(),
            created_at: created_at.to_string(),
            details_json,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageRegistryError {
    AppNotInstalled,
    NoRollbackTarget,
    RollbackDataVersionIncompatible,
}

impl std::fmt::Display for PackageRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageRegistryError::AppNotInstalled => f.write_str("app is not installed"),
            PackageRegistryError::NoRollbackTarget => f.write_str("no rollback target exists"),
            PackageRegistryError::RollbackDataVersionIncompatible => {
                f.write_str("rollback data version incompatible")
            }
        }
    }
}

impl From<PackageRegistryError> for CoreError {
    fn from(value: PackageRegistryError) -> Self {
        CoreError::ValidationError(value.to_string())
    }
}

fn sql_op(sql: impl Into<String>, args: impl IntoIterator<Item = impl Into<Value>>) -> SqlOp {
    SqlOp {
        sql: sql.into(),
        args: args.into_iter().map(Into::into).collect(),
    }
}

fn installation_event_sql(event: &PackageInstallationEvent) -> SqlOp {
    sql_op(
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        [
            Value::String(event.installation_event_id.clone()),
            Value::String(event.app_id.clone()),
            Value::String(event.install_id.clone()),
            Value::String(event.action.clone()),
            event
                .previous_install_id
                .as_ref()
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null),
            Value::String(event.actor.clone()),
            Value::String(event.created_at.clone()),
            event.details_json.clone(),
        ],
    )
}

fn matches_status(token: &str, status: PackageVersionStatus) -> bool {
    token == status.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_registry() -> PlatformRegistry {
        let mut registry = PlatformRegistry::default();
        registry.apps.insert(
            "notes-lite".into(),
            PackageAppRecord {
                id: "notes-lite".into(),
                name: "Notes Lite".into(),
                status: PackageAppStatus::Enabled.as_str().into(),
                active_install_id: Some("install-v2".into()),
                active_version: Some("0.2.0".into()),
                data_version: 1,
            },
        );
        for (install_id, version, status, created_at) in [
            ("install-v1", "0.1.0", "installed", "2024-01-01T00:00:00Z"),
            ("install-v2", "0.2.0", "enabled", "2024-01-02T00:00:00Z"),
        ] {
            registry.versions.insert(
                install_id.into(),
                PackageVersionRecord {
                    install_id: install_id.into(),
                    app_id: "notes-lite".into(),
                    version: version.into(),
                    runtime_version: "0.4.0".into(),
                    data_version: 1,
                    status: status.into(),
                    created_at: created_at.into(),
                    activated_at: Some(created_at.into()),
                },
            );
        }
        registry
    }

    #[test]
    fn rollback_swaps_active_pointer() {
        let mut registry = seed_registry();
        let result = registry
            .rollback_version("notes-lite", None, "test", "2024-01-03T00:00:00Z", None)
            .unwrap();
        assert_eq!(result.active_install_id.as_deref(), Some("install-v1"));
        assert_eq!(result.rolled_back_install_id.as_deref(), Some("install-v2"));
        assert_eq!(
            registry
                .versions
                .get("install-v2")
                .map(|v| v.status.as_str()),
            Some("rolled-back")
        );
    }

    #[test]
    fn quarantine_restores_previous_active_version() {
        let mut registry = seed_registry();
        let result = registry
            .set_status(
                "notes-lite",
                "install-v2",
                PackageVersionStatus::Quarantined.as_str(),
                "test",
                "2024-01-03T00:00:00Z",
                Some("resource_budget_exceeded"),
                true,
                None,
            )
            .unwrap();
        assert_eq!(result.active_install_id.as_deref(), Some("install-v1"));
        assert!(result
            .installation_events
            .iter()
            .any(|e| e.action == "rollback"));
    }
}