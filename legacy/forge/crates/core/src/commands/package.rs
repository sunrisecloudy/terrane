//! Legacy webapp `package.*` commands (C9/D12, Q8 namespace).

use forge_domain::{CoreCommand, CoreError, Result, WebappManifest};
use forge_storage::{
    PackageTransitionResult, PlatformRegistry, PLATFORM_REGISTRY_KEY, PLATFORM_REGISTRY_NS,
};
use serde_json::Value;

use super::super::WorkspaceCore;
use super::{bool_field, take_field};

impl WorkspaceCore {
    pub(in crate::workspace) fn cmd_package_get_manifest(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let manifest = trusted_manifest(cmd)?;
        Ok(serde_json::to_value(&manifest).expect("WebappManifest serializes"))
    }

    pub(in crate::workspace) fn cmd_package_get_permissions(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let manifest = trusted_manifest(cmd)?;
        let ctx = manifest.sandbox_context();
        Ok(serde_json::json!({
            "app_id": manifest.id,
            "permissions": manifest.permissions,
            "storage_prefix": ctx.storage_prefix,
            "network_policy": ctx.network_policy,
            "deny_private_network": ctx.deny_private_network,
            "resource_budget": ctx.resource_budget,
        }))
    }

    pub(in crate::workspace) fn cmd_package_provision_registry(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let snapshot: PlatformRegistry = take_field(cmd, "snapshot")?;
        self.provision_platform_registry(snapshot)?;
        Ok(serde_json::json!({ "ok": true }))
    }

    pub(in crate::workspace) fn cmd_package_list_versions(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let app_id: String = take_field(cmd, "app_id")?;
        self.platform_registry().list_versions(&app_id)
    }

    pub(in crate::workspace) fn cmd_package_activate_version(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let app_id: String = take_field(cmd, "app_id")?;
        let install_id: String = take_field(cmd, "install_id")?;
        let created_at = required_created_at(cmd)?;
        let transition = self.package_activate_version(
            &app_id,
            &install_id,
            command_actor(cmd),
            &created_at,
            cmd.payload
                .get("installation_event_id")
                .and_then(|v| v.as_str()),
        )?;
        Ok(serde_json::to_value(transition).expect("PackageTransitionResult serializes"))
    }

    pub(in crate::workspace) fn cmd_package_rollback_version(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let app_id: String = take_field(cmd, "app_id")?;
        let target_install_id = cmd
            .payload
            .get("target_install_id")
            .and_then(|v| v.as_str());
        let created_at = required_created_at(cmd)?;
        let transition = self.package_rollback_version(
            &app_id,
            target_install_id,
            command_actor(cmd),
            &created_at,
            cmd.payload
                .get("installation_event_id")
                .and_then(|v| v.as_str()),
        )?;
        Ok(serde_json::to_value(transition).expect("PackageTransitionResult serializes"))
    }

    pub(in crate::workspace) fn cmd_package_set_status(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<Value> {
        let app_id: String = take_field(cmd, "app_id")?;
        let install_id: String = take_field(cmd, "install_id")?;
        let status: String = take_field(cmd, "status")?;
        let created_at = required_created_at(cmd)?;
        let reason = cmd.payload.get("reason").and_then(|v| v.as_str());
        let restore_previous = bool_field(cmd, "restore_previous")?;
        let transition = self.package_set_status(
            &app_id,
            &install_id,
            &status,
            command_actor(cmd),
            &created_at,
            reason,
            restore_previous,
            cmd.payload
                .get("installation_event_id")
                .and_then(|v| v.as_str()),
        )?;
        Ok(serde_json::to_value(transition).expect("PackageTransitionResult serializes"))
    }

    pub(in crate::workspace) fn package_activate_version(
        &mut self,
        app_id: &str,
        install_id: &str,
        actor: &str,
        created_at: &str,
        event_id: Option<&str>,
    ) -> Result<PackageTransitionResult> {
        let transition = self
            .platform_registry_mut()
            .activate_version(app_id, install_id, actor, created_at, event_id)?;
        self.persist_platform_registry()?;
        self.audit_package_transition(actor, &transition)?;
        Ok(transition)
    }

    pub(in crate::workspace) fn package_rollback_version(
        &mut self,
        app_id: &str,
        target_install_id: Option<&str>,
        actor: &str,
        created_at: &str,
        event_id: Option<&str>,
    ) -> Result<PackageTransitionResult> {
        let transition = self.platform_registry_mut().rollback_version(
            app_id,
            target_install_id,
            actor,
            created_at,
            event_id,
        )?;
        self.persist_platform_registry()?;
        self.audit_package_transition(actor, &transition)?;
        Ok(transition)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn package_set_status(
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
        let transition = self.platform_registry_mut().set_status(
            app_id,
            install_id,
            status,
            actor,
            created_at,
            reason,
            restore_previous,
            event_id,
        )?;
        self.persist_platform_registry()?;
        self.audit_package_transition(actor, &transition)?;
        Ok(transition)
    }

    pub(in crate::workspace) fn platform_registry(&self) -> &PlatformRegistry {
        &self.platform_registry
    }

    pub(in crate::workspace) fn platform_registry_mut(&mut self) -> &mut PlatformRegistry {
        &mut self.platform_registry
    }

    /// Hydrate the authoritative registry snapshot from a shell-read
    /// `platform.sqlite` export (Q9 dual-file transport).
    pub fn provision_platform_registry(&mut self, snapshot: PlatformRegistry) -> Result<()> {
        self.platform_registry = snapshot;
        self.persist_platform_registry()
    }

    pub(in crate::workspace) fn persist_platform_registry(&mut self) -> Result<()> {
        let bytes = serde_json::to_vec(&self.platform_registry).map_err(|e| {
            CoreError::StorageError(format!("serialize platform registry: {e}"))
        })?;
        self.store.kv_set(
            PLATFORM_REGISTRY_NS,
            PLATFORM_REGISTRY_KEY,
            &bytes,
            "application/json",
        )
    }

    fn audit_package_transition(
        &mut self,
        actor: &str,
        transition: &PackageTransitionResult,
    ) -> Result<()> {
        for action in &transition.audit_actions {
            let _ = self.persist_producer_audit(
                action,
                serde_json::json!({
                    "app_id": transition.app_id,
                    "active_install_id": transition.active_install_id,
                    "rolled_back_install_id": transition.rolled_back_install_id,
                    "installation_events": transition.installation_events,
                }),
                "lifecycle",
                action,
                "allow",
                actor,
                "package",
                Some(transition.app_id.clone()),
                None,
                "package lifecycle transition",
                serde_json::json!({
                    "app_id": transition.app_id,
                    "active_install_id": transition.active_install_id,
                    "rolled_back_install_id": transition.rolled_back_install_id,
                }),
            )?;
        }
        Ok(())
    }
}

fn trusted_manifest(cmd: &CoreCommand) -> Result<WebappManifest> {
    let app_id: String = take_field(cmd, "app_id")?;
    let manifest_json: Value = take_field(cmd, "manifest_json")?;
    let manifest = WebappManifest::from_json_value(&manifest_json)?;
    if manifest.id != app_id {
        return Err(CoreError::ValidationError(format!(
            "package manifest app_id mismatch: payload {app_id:?} != manifest.id {:?}",
            manifest.id
        )));
    }
    Ok(manifest)
}

fn required_created_at(cmd: &CoreCommand) -> Result<String> {
    take_field(cmd, "created_at")
}

fn command_actor(cmd: &CoreCommand) -> &str {
    cmd.payload
        .get("actor")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| cmd.actor.actor.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{ActorContext, CoreCommand, RequestId, WorkspaceId};
    use forge_storage::{PackageAppRecord, PackageVersionRecord};

    fn cmd(payload: serde_json::Value) -> CoreCommand {
        CoreCommand {
            request_id: RequestId::new("t1"),
            actor: ActorContext::owner("host"),
            workspace_id: WorkspaceId::new("ws"),
            applet_id: None,
            name: "package.get_manifest".into(),
            payload,
        }
    }

    fn seed(core: &mut WorkspaceCore) {
        let mut registry = PlatformRegistry::default();
        registry.apps.insert(
            "notes-lite".into(),
            PackageAppRecord {
                id: "notes-lite".into(),
                name: "Notes Lite".into(),
                status: "enabled".into(),
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
        core.provision_platform_registry(registry).unwrap();
    }

    #[test]
    fn get_manifest_rejects_app_id_mismatch() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../webapps/examples/api-dashboard/manifest.json"
        ))
        .unwrap();
        let mut core = WorkspaceCore::in_memory("ws").unwrap();
        let err = core
            .cmd_package_get_manifest(&cmd(serde_json::json!({
                "app_id": "other-app",
                "manifest_json": manifest,
            })))
            .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn rollback_emits_audit_rows() {
        let mut core = WorkspaceCore::in_memory("ws").unwrap();
        seed(&mut core);
        let transition = core
            .package_rollback_version(
                "notes-lite",
                None,
                "host",
                "2024-01-03T00:00:00Z",
                None,
            )
            .unwrap();
        assert_eq!(transition.active_install_id.as_deref(), Some("install-v1"));
        let rows = core
            .store()
            .query_audit(&forge_storage::AuditQuery::by_action("package.rollback"))
            .unwrap();
        assert!(!rows.is_empty());
    }
}