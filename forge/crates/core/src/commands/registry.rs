//! Folded command registry (cli-plan Phase 1): descriptor + handler per row.

use forge_domain::{
    catalog::{CommandStability, CommandVisibility},
    CommandDescriptor, CoreCommand, Result, Role,
};

use crate::catalog::{
    desc, ALL_MEMBERS, ALL_ROLES, AUDIT_PLUS, DATA_WRITE, MAINTAINER_PLUS, RUN_CAPABLE,
};
use crate::cmd_desc;
use super::super::WorkspaceCore;

pub(crate) type Handler = fn(&mut WorkspaceCore, &CoreCommand) -> Result<serde_json::Value>;

pub(crate) struct CommandRegistration {
    pub descriptor: CommandDescriptor,
    pub handler: Handler,
}

pub(crate) static OUTER_COMMANDS: &[CommandRegistration] = &[
    CommandRegistration {
        descriptor: cmd_desc!("system.describe", "Return the role- and tier-scoped command catalog.", CommandVisibility::Public, false, false, ALL_ROLES, CommandStability::Stable),
        handler: WorkspaceCore::cmd_system_describe,
    },
    CommandRegistration {
        descriptor: cmd_desc!("system.trace", "Return recorded host-call effects for a run.", CommandVisibility::Operator, false, false, AUDIT_PLUS, CommandStability::Stable),
        handler: WorkspaceCore::cmd_system_trace,
    },
    CommandRegistration {
        descriptor: cmd_desc!("workspace.create", "Create a new workspace.", CommandVisibility::Operator, true, false, &[Role::Owner], CommandStability::Preview),
        handler: WorkspaceCore::cmd_workspace_create,
    },
    CommandRegistration {
        descriptor: cmd_desc!("workspace.open", "Open or inspect the current workspace.", CommandVisibility::Operator, false, false, ALL_MEMBERS, CommandStability::Stable),
        handler: WorkspaceCore::cmd_workspace_open,
    },
    CommandRegistration {
        descriptor: cmd_desc!("applet.install", "Install an applet from a manifest and sources.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Stable),
        handler: WorkspaceCore::cmd_applet_install,
    },
    CommandRegistration {
        descriptor: cmd_desc!("applet.enable", "Enable a suspended applet.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_applet_enable,
    },
    CommandRegistration {
        descriptor: cmd_desc!("applet.suspend", "Suspend an active applet.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_applet_suspend,
    },
    CommandRegistration {
        descriptor: cmd_desc!("applet.upgrade", "Upgrade an applet to a new version.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_applet_upgrade,
    },
    CommandRegistration {
        descriptor: cmd_desc!("applet.uninstall", "Uninstall an applet.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_applet_uninstall,
    },
    CommandRegistration {
        descriptor: cmd_desc!("runtime.run", "Run an applet entrypoint and record a deterministic run.", CommandVisibility::Public, true, false, RUN_CAPABLE, CommandStability::Stable),
        handler: WorkspaceCore::cmd_runtime_run,
    },
    CommandRegistration {
        descriptor: cmd_desc!("legacy.core_step", "Legacy host compatibility step for mounted webapps.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_legacy_core_step,
    },
    CommandRegistration {
        descriptor: cmd_desc!("bridge.validate_network_request", "Validate a bridge network request against policy.", CommandVisibility::Debug, false, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_bridge_validate_network_request,
    },
    CommandRegistration {
        descriptor: cmd_desc!("bridge.validate_envelope", "Validate a bridge envelope against policy.", CommandVisibility::Debug, false, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_bridge_validate_envelope,
    },
    CommandRegistration {
        descriptor: cmd_desc!("bridge.prepare_session", "Prepare a bridge runtime session.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_bridge_prepare_session,
    },
    CommandRegistration {
        descriptor: cmd_desc!("bridge.record_call", "Record a bridge call in durable logs.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_bridge_record_call,
    },
    CommandRegistration {
        descriptor: cmd_desc!("bridge.record_core_event", "Record a core event from the host bridge.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_bridge_record_core_event,
    },
    CommandRegistration {
        descriptor: cmd_desc!("bridge.record_crash_recovery", "Record crash recovery metadata for a bridge session.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_bridge_record_crash_recovery,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.get_manifest", "Read the trusted manifest for a mounted webapp.", CommandVisibility::Debug, false, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_get_manifest,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.get_permissions", "Read manifest permissions for a mounted webapp.", CommandVisibility::Debug, false, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_get_permissions,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.provision_registry", "Provision the package registry for a mounted webapp.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_provision_registry,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.list_versions", "List installed versions for a webapp package.", CommandVisibility::Debug, false, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_list_versions,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.activate_version", "Activate a specific webapp package version.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_activate_version,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.rollback_version", "Rollback a webapp package to a prior version.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_rollback_version,
    },
    CommandRegistration {
        descriptor: cmd_desc!("package.set_status", "Set the lifecycle status for a webapp package.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Legacy),
        handler: WorkspaceCore::cmd_package_set_status,
    },
    CommandRegistration {
        descriptor: cmd_desc!("runtime.replay", "Replay a recorded run deterministically.", CommandVisibility::Operator, false, false, AUDIT_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_runtime_replay,
    },
    CommandRegistration {
        descriptor: cmd_desc!("runtime.replay_session", "Replay an ordered UI dispatch session.", CommandVisibility::Operator, false, false, AUDIT_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_runtime_replay_session,
    },
    CommandRegistration {
        descriptor: cmd_desc!("ui.dispatch_event", "Dispatch a UI event into an applet handler.", CommandVisibility::Public, true, false, RUN_CAPABLE, CommandStability::Stable),
        handler: WorkspaceCore::cmd_ui_dispatch_event,
    },
    CommandRegistration {
        descriptor: cmd_desc!("query.execute", "Read records from a collection projection.", CommandVisibility::Public, false, false, ALL_MEMBERS, CommandStability::Stable),
        handler: WorkspaceCore::cmd_query_execute,
    },
    CommandRegistration {
        descriptor: cmd_desc!("audit.query", "Query the durable security audit log.", CommandVisibility::Admin, false, false, AUDIT_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_audit_query,
    },
    CommandRegistration {
        descriptor: cmd_desc!("db.watch", "Register a live query subscription.", CommandVisibility::Public, false, false, ALL_MEMBERS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_db_watch,
    },
    CommandRegistration {
        descriptor: cmd_desc!("db.unwatch", "Cancel a live query subscription.", CommandVisibility::Public, true, false, ALL_MEMBERS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_db_unwatch,
    },
    CommandRegistration {
        descriptor: cmd_desc!("db.history", "Read the change history for a record.", CommandVisibility::Public, false, false, ALL_MEMBERS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_db_history,
    },
    CommandRegistration {
        descriptor: cmd_desc!("db.restore", "Restore a record to a prior version.", CommandVisibility::Operator, true, false, DATA_WRITE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_db_restore,
    },
    CommandRegistration {
        descriptor: cmd_desc!("schema.apply_change", "Apply a schema change to the workspace.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_schema_apply_change,
    },
    CommandRegistration {
        descriptor: cmd_desc!("schema.validate_compatibility", "Validate schema compatibility for a proposed change.", CommandVisibility::Operator, false, false, &[Role::Owner, Role::Maintainer, Role::Editor, Role::Auditor], CommandStability::Preview),
        handler: WorkspaceCore::cmd_schema_validate_compatibility,
    },
    CommandRegistration {
        descriptor: cmd_desc!("schema.rebuild_indexes", "Rebuild expression indexes after schema changes.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_schema_rebuild_indexes,
    },
    CommandRegistration {
        descriptor: cmd_desc!("sync.trust_peer", "Trust a sync peer for CRDT exchange.", CommandVisibility::Admin, true, false, &[Role::Owner], CommandStability::Preview),
        handler: WorkspaceCore::cmd_sync_trust_peer,
    },
    CommandRegistration {
        descriptor: cmd_desc!("sync.export", "Export CRDT chunks for sync.", CommandVisibility::Operator, false, false, AUDIT_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_sync_export,
    },
    CommandRegistration {
        descriptor: cmd_desc!("sync.import", "Import CRDT chunks from a sync peer.", CommandVisibility::Operator, true, false, MAINTAINER_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_sync_import,
    },
    CommandRegistration {
        descriptor: cmd_desc!("quota.status", "Report workspace quota usage and limits.", CommandVisibility::Operator, false, false, ALL_MEMBERS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_quota_status,
    },
    CommandRegistration {
        descriptor: cmd_desc!("quota.set", "Configure workspace quota policy.", CommandVisibility::Admin, true, false, &[Role::Owner], CommandStability::Preview),
        handler: WorkspaceCore::cmd_quota_set,
    },
    CommandRegistration {
        descriptor: cmd_desc!("quota.auto_quarantine", "Auto-quarantine a workspace that exceeded quotas.", CommandVisibility::Debug, true, false, RUN_CAPABLE, CommandStability::Preview),
        handler: WorkspaceCore::cmd_quota_auto_quarantine,
    },
    CommandRegistration {
        descriptor: cmd_desc!("workspace.export", "Export a portable workspace bundle.", CommandVisibility::Operator, false, false, AUDIT_PLUS, CommandStability::Preview),
        handler: WorkspaceCore::cmd_workspace_export,
    },
    CommandRegistration {
        descriptor: cmd_desc!("workspace.import", "Import a workspace bundle into the target workspace.", CommandVisibility::Admin, true, false, &[Role::Owner], CommandStability::Preview),
        handler: WorkspaceCore::cmd_workspace_import,
    },
];

#[cfg(feature = "control")]
pub(crate) static CONTROL_COMMANDS: &[CommandRegistration] = &[
    CommandRegistration {
        descriptor: desc("control.compare_snapshot", "Compare two control snapshots.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_compare_snapshot,
    },
    CommandRegistration {
        descriptor: desc("control.json_matches_subset", "Check JSON subset containment.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_json_matches_subset,
    },
    CommandRegistration {
        descriptor: desc("control.package_validate", "Validate a control-plane package.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_package_validate,
    },
    CommandRegistration {
        descriptor: desc("control.package_hashes", "Compute package content hashes.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_package_hashes,
    },
    CommandRegistration {
        descriptor: desc("control.backup_validate", "Validate a backup export.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_backup_validate,
    },
    CommandRegistration {
        descriptor: desc("control.backup_content_hash", "Compute a backup content hash.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_backup_content_hash,
    },
    CommandRegistration {
        descriptor: desc("control.generate_token", "Generate a control-plane token.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_generate_token,
    },
    CommandRegistration {
        descriptor: desc("control.sign_payload", "Sign a control-plane payload.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_sign_payload,
    },
    CommandRegistration {
        descriptor: desc("control.verify_signature", "Verify a control-plane signature.", CommandVisibility::Debug, false, false, ALL_ROLES, CommandStability::Preview, None, None),
        handler: WorkspaceCore::cmd_control_verify_signature,
    },
];
