//! Static command catalog (cli-plan/04, Phase 1 keystone).
//!
//! Every outer command in [`super::commands::Registry`] has a matching
//! [`CommandDescriptor`]. [`system.describe`] projects this table for clients.

use forge_domain::{
    catalog::{CommandDescriptor, CommandStability, CommandSurface, CommandVisibility},
    content_hash, CommandDescriptorJson, Role,
};
use serde::Serialize;

const ALL_MEMBERS: &[Role] = &[
    Role::Owner,
    Role::Maintainer,
    Role::Editor,
    Role::Viewer,
    Role::Auditor,
];
const MAINTAINER_PLUS: &[Role] = &[Role::Owner, Role::Maintainer];
const RUN_CAPABLE: &[Role] = &[Role::Owner, Role::Maintainer, Role::Editor, Role::Runner];
const AUDIT_PLUS: &[Role] = &[Role::Owner, Role::Maintainer, Role::Auditor];
const DATA_WRITE: &[Role] = &[Role::Owner, Role::Maintainer, Role::Editor];
const ALL_ROLES: &[Role] = &[
    Role::Owner,
    Role::Maintainer,
    Role::Editor,
    Role::Runner,
    Role::Viewer,
    Role::Auditor,
    Role::Reviewer,
];

#[allow(clippy::too_many_arguments)]
const fn desc(
    name: &'static str,
    summary: &'static str,
    visibility: CommandVisibility,
    mutates: bool,
    effectful: bool,
    required_roles: &'static [Role],
    stability: CommandStability,
    payload_schema: Option<&'static str>,
) -> CommandDescriptor {
    CommandDescriptor {
        name,
        summary,
        surface: CommandSurface::Outer,
        mutates,
        effectful,
        visibility,
        required_roles,
        capabilities: &[],
        payload_schema,
        response_schema: None,
        events: &[],
        stability,
        since: "m0a",
    }
}

/// The compiled-in outer command catalog.
pub(crate) static CATALOG: &[CommandDescriptor] = &[
    desc(
        "system.describe",
        "Return the role- and tier-scoped command catalog.",
        CommandVisibility::Public,
        false,
        false,
        ALL_ROLES,
        CommandStability::Stable,
        Some("schemas/commands/system.describe.request.schema.json"),
    ),
    desc(
        "system.trace",
        "Return recorded host-call effects for a run.",
        CommandVisibility::Operator,
        false,
        false,
        AUDIT_PLUS,
        CommandStability::Stable,
        Some("schemas/commands/system.trace.request.schema.json"),
    ),
    desc(
        "workspace.create",
        "Create a new workspace.",
        CommandVisibility::Operator,
        true,
        false,
        &[Role::Owner],
        CommandStability::Preview,
        None,
    ),
    desc(
        "workspace.open",
        "Open or inspect the current workspace.",
        CommandVisibility::Operator,
        false,
        false,
        ALL_MEMBERS,
        CommandStability::Stable,
        Some("schemas/commands/workspace.open.request.schema.json"),
    ),
    desc(
        "workspace.export",
        "Export a portable workspace bundle.",
        CommandVisibility::Operator,
        false,
        false,
        AUDIT_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "workspace.import",
        "Import a workspace bundle into the target workspace.",
        CommandVisibility::Admin,
        true,
        false,
        &[Role::Owner],
        CommandStability::Preview,
        None,
    ),
    desc(
        "applet.install",
        "Install an applet from a manifest and sources.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Stable,
        None,
    ),
    desc(
        "applet.enable",
        "Enable a suspended applet.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "applet.suspend",
        "Suspend an active applet.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "applet.upgrade",
        "Upgrade an applet to a new version.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "applet.uninstall",
        "Uninstall an applet.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "runtime.run",
        "Run an applet entrypoint and record a deterministic run.",
        CommandVisibility::Public,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Stable,
        None,
    ),
    desc(
        "legacy.core_step",
        "Legacy host compatibility step for mounted webapps.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "bridge.validate_network_request",
        "Validate a bridge network request against policy.",
        CommandVisibility::Debug,
        false,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "bridge.validate_envelope",
        "Validate a bridge envelope against policy.",
        CommandVisibility::Debug,
        false,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "bridge.prepare_session",
        "Prepare a bridge runtime session.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "bridge.record_call",
        "Record a bridge call in durable logs.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "bridge.record_core_event",
        "Record a core event from the host bridge.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "bridge.record_crash_recovery",
        "Record crash recovery metadata for a bridge session.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "package.get_manifest",
        "Read the trusted manifest for a mounted webapp.",
        CommandVisibility::Debug,
        false,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "package.get_permissions",
        "Read manifest permissions for a mounted webapp.",
        CommandVisibility::Debug,
        false,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "package.provision_registry",
        "Provision the package registry for a mounted webapp.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "package.list_versions",
        "List installed versions for a webapp package.",
        CommandVisibility::Debug,
        false,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "package.activate_version",
        "Activate a specific webapp package version.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "package.rollback_version",
        "Rollback a webapp package to a prior version.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "package.set_status",
        "Set the lifecycle status for a webapp package.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Legacy,
        None,
    ),
    desc(
        "runtime.replay",
        "Replay a recorded run deterministically.",
        CommandVisibility::Operator,
        false,
        false,
        AUDIT_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "runtime.replay_session",
        "Replay an ordered UI dispatch session.",
        CommandVisibility::Operator,
        false,
        false,
        AUDIT_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "ui.dispatch_event",
        "Dispatch a UI event into an applet handler.",
        CommandVisibility::Public,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Stable,
        None,
    ),
    desc(
        "query.execute",
        "Read records from a collection projection.",
        CommandVisibility::Public,
        false,
        false,
        ALL_MEMBERS,
        CommandStability::Stable,
        Some("schemas/commands/query.execute.request.schema.json"),
    ),
    desc(
        "audit.query",
        "Query the durable security audit log.",
        CommandVisibility::Admin,
        false,
        false,
        AUDIT_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "db.watch",
        "Register a live query subscription.",
        CommandVisibility::Public,
        false,
        false,
        ALL_MEMBERS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "db.unwatch",
        "Cancel a live query subscription.",
        CommandVisibility::Public,
        true,
        false,
        ALL_MEMBERS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "db.history",
        "Read the change history for a record.",
        CommandVisibility::Public,
        false,
        false,
        ALL_MEMBERS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "db.restore",
        "Restore a record to a prior version.",
        CommandVisibility::Operator,
        true,
        false,
        DATA_WRITE,
        CommandStability::Preview,
        None,
    ),
    desc(
        "schema.apply_change",
        "Apply a schema change to the workspace.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "schema.validate_compatibility",
        "Validate schema compatibility for a proposed change.",
        CommandVisibility::Operator,
        false,
        false,
        &[Role::Owner, Role::Maintainer, Role::Editor, Role::Auditor],
        CommandStability::Preview,
        None,
    ),
    desc(
        "schema.rebuild_indexes",
        "Rebuild expression indexes after schema changes.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "sync.trust_peer",
        "Trust a sync peer for CRDT exchange.",
        CommandVisibility::Admin,
        true,
        false,
        &[Role::Owner],
        CommandStability::Preview,
        None,
    ),
    desc(
        "sync.export",
        "Export CRDT chunks for sync.",
        CommandVisibility::Operator,
        false,
        false,
        AUDIT_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "sync.import",
        "Import CRDT chunks from a sync peer.",
        CommandVisibility::Operator,
        true,
        false,
        MAINTAINER_PLUS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "quota.status",
        "Report workspace quota usage and limits.",
        CommandVisibility::Operator,
        false,
        false,
        ALL_MEMBERS,
        CommandStability::Preview,
        None,
    ),
    desc(
        "quota.set",
        "Configure workspace quota policy.",
        CommandVisibility::Admin,
        true,
        false,
        &[Role::Owner],
        CommandStability::Preview,
        None,
    ),
    desc(
        "quota.auto_quarantine",
        "Auto-quarantine a workspace that exceeded quotas.",
        CommandVisibility::Debug,
        true,
        false,
        RUN_CAPABLE,
        CommandStability::Preview,
        None,
    ),
];

#[cfg(feature = "control")]
pub(crate) static CONTROL_CATALOG: &[CommandDescriptor] = &[
    desc(
        "control.compare_snapshot",
        "Compare two control snapshots.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.json_matches_subset",
        "Check JSON subset containment.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.package_validate",
        "Validate a control-plane package.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.package_hashes",
        "Compute package content hashes.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.backup_validate",
        "Validate a backup export.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.backup_content_hash",
        "Compute a backup content hash.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.generate_token",
        "Generate a control-plane token.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.sign_payload",
        "Sign a control-plane payload.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
    desc(
        "control.verify_signature",
        "Verify a control-plane signature.",
        CommandVisibility::Debug,
        false,
        false,
        ALL_ROLES,
        CommandStability::Preview,
        None,
    ),
];

/// Lookup a command descriptor by name (outer catalog + optional debug control).
pub(crate) fn descriptor_for(name: &str) -> Option<&'static CommandDescriptor> {
    if let Some(entry) = CATALOG.iter().find(|entry| entry.name == name) {
        return Some(entry);
    }
    #[cfg(feature = "control")]
    {
        CONTROL_CATALOG.iter().find(|entry| entry.name == name)
    }
    #[cfg(not(feature = "control"))]
    {
        None
    }
}

/// All catalog entries visible to this build (outer + optional debug control).
pub(crate) fn catalog_entries() -> Vec<&'static CommandDescriptor> {
    #[allow(unused_mut)]
    let mut entries: Vec<&'static CommandDescriptor> = CATALOG.iter().collect();
    #[cfg(feature = "control")]
    entries.extend(CONTROL_CATALOG.iter());
    entries
}

/// Stable hash over the sorted, serialized catalog for this build.
pub(crate) fn catalog_version_hash() -> String {
    #[derive(Serialize)]
    struct CatalogBody<'a> {
        commands: &'a [CommandDescriptorJson],
    }

    let mut commands: Vec<CommandDescriptorJson> =
        catalog_entries().iter().map(|d| d.to_json()).collect();
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    let body = serde_json::to_vec(&CatalogBody { commands: &commands })
        .expect("catalog serialization must be infallible");
    content_hash(&body)
}

pub(crate) fn parse_visibility_tier(value: Option<&str>) -> Result<CommandVisibility, String> {
    let tier = value.unwrap_or("public");
    match tier {
        "public" => Ok(CommandVisibility::Public),
        "operator" => Ok(CommandVisibility::Operator),
        "admin" => Ok(CommandVisibility::Admin),
        "debug" => Ok(CommandVisibility::Debug),
        other => Err(format!("unknown visibility tier {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_names_are_unique() {
        let mut seen = BTreeSet::new();
        for entry in catalog_entries() {
            assert!(seen.insert(entry.name), "duplicate catalog name {}", entry.name);
        }
    }

    #[test]
    fn catalog_version_is_stable() {
        let first = catalog_version_hash();
        let second = catalog_version_hash();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
    }
}