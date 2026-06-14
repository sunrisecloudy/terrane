//! Trusted run-policy state: the workspace/run/platform inputs the live SC-10
//! gates read (`forge/spec/policy-gates.md`, gates 2/4/5).
//!
//! This is the **forge-core** trusted source for the three `DecisionContext`
//! gates that `forge_policy::ComposedDecisionContext` evaluates on every live
//! `ctx.*` host call (T037):
//!
//!   - **workspace-policy** (gate 2) → which capability categories the workspace
//!     admin policy permits / forbids;
//!   - **run-profile** (gate 4) → the run's declared profile bounds;
//!   - **platform-permission** (gate 5) → the OS-granted capability set.
//!
//! TRUST MODEL (review 048/050, identical to `db_read_grants` / `sync_membership`):
//! this state is set ONLY through a trusted [`WorkspaceCore`] seam
//! (`set_run_policy`) — workspace configuration / membership — and is persisted to
//! the workspace file. It is NEVER read from a command's request payload, so an
//! applet (or a shell) cannot widen its own grants by editing the command body.
//!
//! FAIL-CLOSED vs. UN-PROVISIONED (the two distinct defaults):
//!   - An **un-provisioned** workspace (no [`RunPolicy`] set) runs under the
//!     permissive [`forge_runtime::DecisionContext`] default (`AllowAll`) — the
//!     M0a spine baseline, so the demo and existing applets are unaffected. There
//!     is no SC-10 deny to impose because the admin never configured one.
//!   - A **provisioned** [`RunPolicy`] builds a real
//!     [`ComposedDecisionContext`](forge_runtime::ComposedDecisionContext). Each
//!     gate the admin **explicitly restricts** then denies fail-closed per the
//!     [`forge_policy`] contract (a category absent from an explicit allow list,
//!     outside the profile bounds, or not platform-granted is denied). A gate the
//!     admin leaves **unspecified** defaults to "all categories" so configuring
//!     only `workspace_policy.denied = [Db]` blocks `db` without forcing the admin
//!     to re-enumerate every other category — the policy only ever ADDS denials
//!     relative to the prior `AllowAll` baseline (shells tighten, never loosen).

use forge_runtime::{
    ComposedDecisionContext, DecisionContext, PlatformPermissions, PolicyCategory, RunProfile,
    WorkspacePolicy,
};
use serde::{Deserialize, Serialize};

/// The capability categories the live SC-10 gates decide over, mirrored from
/// [`forge_policy::Category`](forge_runtime::PolicyCategory) with serde so the
/// trusted [`RunPolicy`] can be persisted to the workspace file. Kept as a local
/// enum (rather than re-exporting the policy one) so the persisted JSON is owned by
/// this trusted-state module and not coupled to the engine's internal type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Storage,
    Db,
    Ui,
    Time,
    Random,
}

impl Capability {
    /// Every capability category — the implicit "allow all" a gate the admin left
    /// unspecified expands to.
    const ALL: [Capability; 5] = [
        Capability::Storage,
        Capability::Db,
        Capability::Ui,
        Capability::Time,
        Capability::Random,
    ];

    /// Map to the engine's gate category.
    fn to_policy(self) -> PolicyCategory {
        match self {
            Capability::Storage => PolicyCategory::Storage,
            Capability::Db => PolicyCategory::Db,
            Capability::Ui => PolicyCategory::Ui,
            Capability::Time => PolicyCategory::Time,
            Capability::Random => PolicyCategory::Random,
        }
    }
}

/// The trusted workspace/run/platform inputs for the three live SC-10 gates.
///
/// Set via [`WorkspaceCore::set_run_policy`](crate::WorkspaceCore::set_run_policy)
/// and persisted to the workspace file. Every field is optional: an unspecified
/// gate defaults to permitting all categories (see the module note), so a partial
/// policy only restricts the gates the admin actually configured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPolicy {
    /// Workspace-policy gate (gate 2). When set, only these categories are
    /// permitted (minus `workspace_denied`); when `None`, all are permitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_allowed: Option<Vec<Capability>>,
    /// Workspace-policy gate explicit denials — these override `workspace_allowed`
    /// (deny wins on conflict), matching [`forge_policy::WorkspacePolicy`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_denied: Vec<Capability>,
    /// Run-profile gate (gate 4) name, for diagnostics/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_profile_name: Option<String>,
    /// Run-profile gate bounds. When set, only these categories are within the
    /// profile; when `None`, all are within bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_profile_permitted: Option<Vec<Capability>>,
    /// Platform-permission gate (gate 5). When set, only these categories are
    /// OS-granted (others are `PlatformUnavailable`); when `None`, all are granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_granted: Option<Vec<Capability>>,
}

impl RunPolicy {
    /// Build the live [`ComposedDecisionContext`] this trusted policy materializes,
    /// boxed as a [`DecisionContext`] ready to install on a record entry point.
    ///
    /// A gate the admin left unspecified expands to "all categories", so the
    /// resulting context only ADDS denials relative to `AllowAll`. The boxed
    /// context reads ONLY these trusted inputs — never the request payload.
    pub fn to_decision_context(&self) -> Box<dyn DecisionContext> {
        let allowed = self
            .workspace_allowed
            .clone()
            .unwrap_or_else(|| Capability::ALL.to_vec());
        let workspace = WorkspacePolicy::new(
            allowed.iter().map(|c| c.to_policy()),
            self.workspace_denied.iter().map(|c| c.to_policy()),
        );

        let permitted = self
            .run_profile_permitted
            .clone()
            .unwrap_or_else(|| Capability::ALL.to_vec());
        let run_profile = RunProfile::new(
            self.run_profile_name.clone().unwrap_or_else(|| "workspace-default".to_string()),
            permitted.iter().map(|c| c.to_policy()),
        );

        let granted = self
            .platform_granted
            .clone()
            .unwrap_or_else(|| Capability::ALL.to_vec());
        let platform = PlatformPermissions::new(granted.iter().map(|c| c.to_policy()));

        Box::new(ComposedDecisionContext::new(workspace, run_profile, platform))
    }
}
