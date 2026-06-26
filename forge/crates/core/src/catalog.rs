//! Static command catalog (cli-plan/04, Phase 1 keystone).
//!
//! Every outer command in [`super::commands::Registry`] has a matching
//! [`CommandDescriptor`]. [`system.describe`] projects this table for clients.

use forge_domain::{
    catalog::{CommandDescriptor, CommandStability, CommandSurface, CommandVisibility},
    content_hash, CommandDescriptorJson, Role,
};
use serde::Serialize;

pub(crate) const ALL_MEMBERS: &[Role] = &[
    Role::Owner,
    Role::Maintainer,
    Role::Editor,
    Role::Viewer,
    Role::Auditor,
];
pub(crate) const MAINTAINER_PLUS: &[Role] = &[Role::Owner, Role::Maintainer];
pub(crate) const RUN_CAPABLE: &[Role] = &[Role::Owner, Role::Maintainer, Role::Editor, Role::Runner];
pub(crate) const AUDIT_PLUS: &[Role] = &[Role::Owner, Role::Maintainer, Role::Auditor];
pub(crate) const DATA_WRITE: &[Role] = &[Role::Owner, Role::Maintainer, Role::Editor];
pub(crate) const ALL_ROLES: &[Role] = &[
    Role::Owner,
    Role::Maintainer,
    Role::Editor,
    Role::Runner,
    Role::Viewer,
    Role::Auditor,
    Role::Reviewer,
];

#[allow(clippy::too_many_arguments)]
pub(crate) const fn desc(
    name: &'static str,
    summary: &'static str,
    visibility: CommandVisibility,
    mutates: bool,
    effectful: bool,
    required_roles: &'static [Role],
    stability: CommandStability,
    payload_schema: Option<&'static str>,
    response_schema: Option<&'static str>,
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
        response_schema,
        events: &[],
        stability,
        since: "m0a",
    }
}

/// Build a descriptor with the standard per-command JSON schema paths.
#[macro_export]
macro_rules! cmd_desc {
    (
        $name:literal,
        $summary:literal,
        $visibility:expr,
        $mutates:expr,
        $effectful:expr,
        $roles:expr,
        $stability:expr
    ) => {
        $crate::catalog::desc(
            $name,
            $summary,
            $visibility,
            $mutates,
            $effectful,
            $roles,
            $stability,
            Some(concat!("schemas/commands/", $name, ".request.schema.json")),
            Some(concat!("schemas/commands/", $name, ".response.schema.json")),
        )
    };
}

/// Outer + control descriptors live in [`crate::workspace::commands::registry`] (folded with handlers).
const fn inner_desc(name: &'static str, summary: &'static str) -> CommandDescriptor {
    CommandDescriptor {
        name,
        summary,
        surface: CommandSurface::Inner,
        mutates: false,
        effectful: false,
        visibility: CommandVisibility::Public,
        required_roles: &[],
        capabilities: &[],
        payload_schema: None,
        response_schema: None,
        events: &[],
        stability: CommandStability::Stable,
        since: "m0a",
    }
}

/// Reference-only inner (`ctx.*` / HostBridge) entries for describe/documentation.
pub(crate) static INNER_CATALOG: &[CommandDescriptor] = &[
    inner_desc("storage.get", "Read a key from applet storage."),
    inner_desc("storage.set", "Write a key in applet storage."),
    inner_desc("storage.delete", "Delete a key from applet storage."),
    inner_desc("storage.list", "List storage keys by prefix."),
    inner_desc("db.insert", "Insert a record into a collection."),
    inner_desc("db.update", "Replace a record in a collection."),
    inner_desc("db.patch", "Patch fields on a record."),
    inner_desc("db.delete", "Tombstone a record."),
    inner_desc("db.transact", "Apply mutations atomically."),
    inner_desc("db.get", "Read one record by id."),
    inner_desc("db.list", "List all records in a collection."),
    inner_desc("db.query", "Run a structured query against a collection."),
    inner_desc("db.watch", "Register a live query subscription."),
    inner_desc("db.unwatch", "Cancel a live query subscription."),
    inner_desc("ui.render", "Render a declarative UI tree."),
    inner_desc("log", "Append a bounded log line."),
    inner_desc("net.fetch", "Perform a policy-gated network fetch."),
    inner_desc("files.write", "Write a sandbox file."),
];

pub(crate) fn inner_catalog_entries() -> &'static [CommandDescriptor] {
    INNER_CATALOG
}

/// Lookup a command descriptor by name (outer catalog + optional debug control).
pub(crate) fn descriptor_for(name: &str) -> Option<&'static CommandDescriptor> {
    crate::workspace::commands::registry::OUTER_COMMANDS
        .iter()
        .find(|entry| entry.descriptor.name == name)
        .map(|entry| &entry.descriptor)
        .or_else(|| {
            #[cfg(feature = "control")]
            {
                crate::workspace::commands::registry::CONTROL_COMMANDS
                    .iter()
                    .find(|entry| entry.descriptor.name == name)
                    .map(|entry| &entry.descriptor)
            }
            #[cfg(not(feature = "control"))]
            {
                None
            }
        })
}

/// All catalog entries visible to this build (outer + optional debug control).
pub(crate) fn catalog_entries() -> Vec<&'static CommandDescriptor> {
    let mut entries: Vec<&'static CommandDescriptor> =
        crate::workspace::commands::registry::OUTER_COMMANDS
        .iter()
        .map(|entry| &entry.descriptor)
        .collect();
    #[cfg(feature = "control")]
    {
        entries.extend(
            crate::workspace::commands::registry::CONTROL_COMMANDS
                .iter()
                .map(|entry| &entry.descriptor),
        );
    }
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