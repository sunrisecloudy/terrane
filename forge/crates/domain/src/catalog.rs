//! Machine-readable command catalog types (Forge Unified CLI, cli-plan/04).
//!
//! Pure data — no I/O. Every consumer (CLI, console, agent, public contract)
//! projects from [`CommandDescriptor`].

use serde::{Deserialize, Serialize};

use crate::Role;

/// Operator/agent surface vs applet sandbox reference entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSurface {
    Outer,
    Inner,
}

/// Front-end exposure tier (on top of RBAC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandVisibility {
    Public,
    Operator,
    Admin,
    Debug,
}

/// Schema lifecycle for a command descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStability {
    Stable,
    Preview,
    Legacy,
    Deprecated,
}

/// Static metadata for one command in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDescriptor {
    pub name: &'static str,
    pub summary: &'static str,
    pub surface: CommandSurface,
    pub mutates: bool,
    pub effectful: bool,
    pub visibility: CommandVisibility,
    pub required_roles: &'static [Role],
    /// Secondary capability gates (e.g. `db.read:<collection>`).
    pub capabilities: &'static [&'static str],
    pub payload_schema: Option<&'static str>,
    pub response_schema: Option<&'static str>,
    pub events: &'static [&'static str],
    pub stability: CommandStability,
    pub since: &'static str,
}

/// JSON-serializable view of a descriptor (owned strings for clients).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDescriptorJson {
    pub name: String,
    pub namespace: String,
    pub summary: String,
    pub surface: CommandSurface,
    pub mutates: bool,
    pub effectful: bool,
    pub visibility: CommandVisibility,
    pub required_roles: Vec<Role>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    pub stability: CommandStability,
    pub since: String,
}

impl CommandDescriptor {
    pub fn namespace(&self) -> &'static str {
        self.name.split('.').next().unwrap_or(self.name)
    }

    pub fn to_json(&self) -> CommandDescriptorJson {
        CommandDescriptorJson {
            name: self.name.to_string(),
            namespace: self.namespace().to_string(),
            summary: self.summary.to_string(),
            surface: self.surface,
            mutates: self.mutates,
            effectful: self.effectful,
            visibility: self.visibility,
            required_roles: self.required_roles.to_vec(),
            capabilities: self.capabilities.iter().map(|s| (*s).to_string()).collect(),
            payload_schema: self.payload_schema.map(str::to_string),
            response_schema: self.response_schema.map(str::to_string),
            events: self.events.iter().map(|s| (*s).to_string()).collect(),
            stability: self.stability,
            since: self.since.to_string(),
        }
    }

    /// True if `role` may issue this command per the descriptor role set.
    pub fn role_permitted(&self, role: Role) -> bool {
        self.required_roles.contains(&role)
    }

    /// True if `role` may see this command at `max_tier` visibility.
    pub fn visible_to(&self, role: Role, max_tier: CommandVisibility) -> bool {
        if self.visibility > max_tier {
            return false;
        }
        self.role_permitted(role)
    }
}

/// Derive namespace from a command name.
pub fn command_namespace(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}