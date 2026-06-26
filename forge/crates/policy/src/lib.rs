//! forge-policy: the capability + minimal-RBAC engine that gates every
//! `ctx.*` host call an applet attempts at runtime.
//!
//! Normative spec: prd-merged/07-security-prd.md.
//!   - **SC-8** capability = action + resource + constraints. A [`HostCall`]
//!     models the concrete call the runtime is about to make; the manifest
//!     [`Capabilities`] grants are the action+resource scopes.
//!   - **SC-9** grants are per-workspace-member: the [`ActorContext`] role is
//!     evaluated together with the manifest grants on every call.
//!   - **SC-10** a run is allowed only if **all** of seven gates pass:
//!     `actor role permits operation ∧ workspace policy permits capability ∧
//!     manifest requests it ∧ run profile permits it ∧ platform permission
//!     granted ∧ resource matches allowlist ∧ rate/resource limit available`.
//!   - **SC-2** the host-call counter is a flood guard mapped to
//!     `manifest.limits.max_host_calls` → [`CoreError::ResourceLimitExceeded`].
//!   - **CR-4** revocation takes effect immediately: [`PolicyEngine::revoke`]
//!     denies a category on the very next [`PolicyEngine::check`].
//!
//! ## Honest scoping of the SC-10 decision (review 006 P1)
//!
//! SC-10 enumerates **seven** independent gates. This crate's
//! [`CapabilityCheck`] implements only **three** of them — the
//! *manifest-requests-it* gate, the *resource-matches-allowlist* gate, and the
//! immediate-revocation hook (CR-4). It is **not** the whole SC-10 decision and
//! must never be mistaken for it. The remaining gates
//! (`workspace policy permits capability`, `run profile permits it`,
//! `platform permission granted`) live behind the [`DecisionContext`] seam,
//! which has an explicit, fail-closed place for each. The actor-role and
//! rate-limit gates are enforced directly by [`PolicyEngine::check`].
//!
//! [`PolicyEngine`] composes all of these: it runs the [`DecisionContext`]
//! gates, the actor-role gate, the budget gate, and finally the
//! [`CapabilityCheck`] subcheck. **In M0a the three [`DecisionContext`] gates
//! are permissive stubs** (`AllowAll`), documented per-method below; they exist
//! so that wiring a real workspace-policy / run-profile / platform-permission
//! source in M0b is a drop-in, and so that the missing gates have a visible,
//! fail-closed seam today rather than being silently absent.
//!
//! This crate is pure logic with no I/O; it stays `wasm32-unknown-unknown`
//! clean (no `std::time`/`std::fs`). The runtime crate owns real CPU/memory
//! limits; policy owns role/capability decisions and the call-count limit.

use forge_domain::{
    ActorContext, Capabilities, CoreError, Manifest, PermissionSnapshot, Result, Role,
};
use serde::{Deserialize, Serialize};

mod auto_quarantine;
mod bridge_gate;
mod bridge_record;
mod net;
mod net_url;
mod webapp_net;

pub use auto_quarantine::{
    evaluate_auto_quarantine, AutoQuarantineDecision, AutoQuarantinePolicy, AutoQuarantineRequest,
};

pub use bridge_gate::{
    bridge_context_from_manifest, permission_for_bridge_method, validate_bridge_envelope,
    BridgeCallCounts, BridgeEnvelopeDecision, BridgeEnvelopeRequest,
};
pub use bridge_record::{
    bridge_call_id, core_action_id, core_event_id, runtime_session_id, state_version_before,
    BridgeCallRecord, BridgePlatformIds, CoreActionRecord, CoreEventRecord, RuntimeSessionMetadata,
};
pub use net::{check_net, HeaderValue, NetPolicy, NetRequest};
pub use net_url::{host_is_private_literal, ParsedUrl};
pub use webapp_net::{check_webapp_network, WebappNetDecision, WebappNetRequest};

/// A concrete `ctx.*` host call the runtime is about to perform.
///
/// Each variant carries exactly the resource the policy engine needs to match
/// against the manifest allowlist (prd-merged/07 SC-8 action+resource).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum HostCall {
    /// `ctx.storage.get` / `ctx.storage.set` against a KV key.
    Storage { op: Access, key: String },
    /// `ctx.db.*` read/write against a logical collection.
    Db { op: Access, collection: String },
    /// `ctx.ui.render` — emit a UI tree.
    Ui,
    /// `ctx.time.now` — deterministic clock seam.
    Time,
    /// `ctx.random.next` — deterministic RNG seam.
    Random,
    /// `ctx.resource.invoke(kind, args)` — platform resource capture.
    Resource {
        kind: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    /// `ctx.resource.read(asset_id)` — lazy byte retrieval for a run asset.
    ResourceRead { asset_id: String },
    /// `ctx.resource.materialize(asset_id, handle, path)` — copy asset into files sandbox.
    ResourceMaterialize {
        asset_id: String,
        handle: String,
        path: String,
    },
}

/// Read vs write intent for a [`HostCall`] resource access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}

impl Access {
    fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

/// The capability categories a [`HostCall`] can belong to. Used both for
/// targeted revocation (prd-merged/07 CR-4) and to name a category in errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Storage,
    Db,
    Ui,
    Time,
    Random,
    Resource,
}

const CATEGORY_COUNT: usize = 6;

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Category::Storage => "storage",
            Category::Db => "db",
            Category::Ui => "ui",
            Category::Time => "time",
            Category::Random => "random",
            Category::Resource => "resource",
        }
    }

    /// Dense index of a category, used for the per-category allowance bitset.
    fn index(self) -> usize {
        match self {
            Category::Storage => 0,
            Category::Db => 1,
            Category::Ui => 2,
            Category::Time => 3,
            Category::Random => 4,
            Category::Resource => 5,
        }
    }
}

/// Whether a role is permitted to *run* applet code at all (prd-merged/07
/// SC-10 "actor role permits operation", SC-11 default roles).
///
/// Owner/Maintainer/Editor/Runner may run. Viewer/Auditor/Reviewer are
/// read-only/oversight roles and cannot trigger execution.
fn role_can_run(role: Role) -> bool {
    matches!(role, Role::Owner | Role::Maintainer | Role::Editor | Role::Runner)
}

// ---------------------------------------------------------------------------
// SC-10 gates that are NOT the manifest+resource subcheck (review 006 P1).
// ---------------------------------------------------------------------------

/// Outcome of a single SC-10 gate hook. A gate either permits the capability,
/// denies it with a reason that is surfaced as `PermissionDenied`, or reports
/// the capability is *unavailable on this platform* (`PlatformUnavailable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    /// Deny, carrying a human-readable reason naming the failing gate. Surfaced
    /// as [`CoreError::PermissionDenied`].
    Deny(String),
    /// The host platform does not grant this capability (no camera, no
    /// clipboard, OS denied the prompt). Surfaced as
    /// [`CoreError::PlatformUnavailable`] (prd-merged/01 CR-3), distinct from a
    /// policy denial: the capability is not *refused*, it is *absent*.
    Unavailable(String),
}

/// The SC-10 gates that this crate does **not** implement as the
/// manifest+resource [`CapabilityCheck`]: the workspace-policy, run-profile,
/// and platform-permission gates (prd-merged/07 SC-10).
///
/// Implementors decide each gate *fail-closed*: a real implementation returns
/// [`GateDecision::Deny`] whenever it cannot positively confirm the gate is
/// satisfied. M0a ships [`AllowAll`], a permissive stub that is the *only*
/// place these gates are short-circuited, so the absence of the real gates is
/// explicit and auditable rather than invisible.
///
/// Each hook receives the [`Category`] being exercised so a future
/// implementation can scope decisions per capability category (e.g. "this run
/// profile forbids `db` writes" or "platform has no clipboard").
pub trait DecisionContext: std::fmt::Debug {
    /// Clone this context behind the trait object so a [`PolicyEngine`] clone
    /// preserves the **exact** gate behavior (review 023 P2). `PolicyEngine`
    /// holds the context as `Box<dyn DecisionContext>`, which is not `Clone`;
    /// without this hook a clone would have to substitute a different context
    /// and could become *more permissive* (a fail-open bug). Implementors that
    /// derive `Clone` get this for free via [`clone_decision_context`].
    fn clone_box(&self) -> Box<dyn DecisionContext>;

    /// **Gate: workspace policy permits capability** (SC-10).
    ///
    /// The workspace admin policy can forbid a capability category outright,
    /// independent of any single applet's manifest. The **trusted source** is
    /// the workspace's own admin policy ([`WorkspacePolicy`]), never the request
    /// payload (review 048/050). [`ComposedDecisionContext`] implements the real
    /// evaluation; the permissive default here serves [`AllowAll`] and the
    /// replay context only.
    fn workspace_policy(&self, _category: Category) -> GateDecision {
        GateDecision::Allow
    }

    /// **Gate: run profile permits it** (SC-10).
    ///
    /// The run profile (e.g. a locked-down "review-safety" profile, prd-merged/07
    /// SC-21) can narrow what a run may do. The **trusted source** is the run's
    /// declared profile bounds ([`RunProfile`]), resolved from trusted run state,
    /// never the request payload. [`ComposedDecisionContext`] implements the real
    /// evaluation; the permissive default here serves [`AllowAll`] and the
    /// replay context only.
    fn run_profile(&self, _category: Category) -> GateDecision {
        GateDecision::Allow
    }

    /// **Gate: platform permission granted** (SC-10, prd-merged/01 CR-3
    /// `PlatformUnavailable`).
    ///
    /// The host platform may not grant a capability (no camera, no clipboard,
    /// OS denied the prompt). The **trusted source** is the OS-granted capability
    /// set ([`PlatformPermissions`]) the host reports, never the request payload.
    /// A missing platform grant yields [`GateDecision::Unavailable`] →
    /// [`CoreError::PlatformUnavailable`]. [`ComposedDecisionContext`] implements
    /// the real evaluation; the permissive default here serves [`AllowAll`] and
    /// the replay context only.
    fn platform_permission(&self, _category: Category) -> GateDecision {
        GateDecision::Allow
    }
}

/// Box-clone a [`DecisionContext`] implementor that is `Clone`. Implementors
/// can satisfy [`DecisionContext::clone_box`] with a one-liner:
/// `fn clone_box(&self) -> Box<dyn DecisionContext> { clone_decision_context(self) }`.
pub fn clone_decision_context<C: DecisionContext + Clone + 'static>(
    ctx: &C,
) -> Box<dyn DecisionContext> {
    Box::new(ctx.clone())
}

/// The M0a permissive [`DecisionContext`]: every non-capability SC-10 gate
/// allows. This is the **single, explicit** place the workspace-policy,
/// run-profile, and platform-permission gates are short-circuited until M0b
/// wires real sources (review 006 P1).
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

impl DecisionContext for AllowAll {
    fn clone_box(&self) -> Box<dyn DecisionContext> {
        clone_decision_context(self)
    }
}

// ---------------------------------------------------------------------------
// Real trusted-source SC-10 gates (workspace-policy / run-profile /
// platform-permission). These replace the AllowAll stubs on the *live* decision
// path; AllowAll stays the default + the replay context (review 006 P1, T037).
// ---------------------------------------------------------------------------
//
// FAIL-CLOSED & TRUSTED-SOURCE (review 048/050): every type below is read from
// TRUSTED workspace / run / platform state, NEVER the request payload. A gate
// input that is missing or ambiguous DENIES — it never silently allows.

/// **Trusted source for the workspace-policy gate** (SC-10).
///
/// The workspace admin policy is an explicit allow/deny over capability
/// *categories*, decided by the workspace, not the applet. A category is
/// permitted only when it is in `allowed` and not in `denied`; `denied` wins on
/// conflict, and a category in neither set is **denied fail-closed** (the
/// workspace never positively granted it).
///
/// This is trusted workspace state. It is resolved from the workspace's own
/// policy table at the command boundary, never from the request payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspacePolicy {
    /// Categories the workspace admin policy explicitly permits.
    allowed: Vec<Category>,
    /// Categories the workspace admin policy explicitly forbids. Overrides
    /// `allowed` on conflict (deny wins).
    denied: Vec<Category>,
}

impl WorkspacePolicy {
    /// Build a workspace policy from explicit allow / deny category lists.
    pub fn new(allowed: impl IntoIterator<Item = Category>, denied: impl IntoIterator<Item = Category>) -> Self {
        WorkspacePolicy { allowed: allowed.into_iter().collect(), denied: denied.into_iter().collect() }
    }

    /// Evaluate the workspace-policy gate fail-closed: an explicit deny denies;
    /// an absent grant (not in `allowed`) denies; only an explicit allow with no
    /// overriding deny permits.
    fn decide(&self, category: Category) -> GateDecision {
        if self.denied.contains(&category) {
            return GateDecision::Deny(format!(
                "workspace policy explicitly forbids the {} capability",
                category.as_str()
            ));
        }
        if self.allowed.contains(&category) {
            GateDecision::Allow
        } else {
            // FAIL-CLOSED: the workspace never positively granted this category.
            GateDecision::Deny(format!(
                "workspace policy does not grant the {} capability (fail-closed: absent from the allow list)",
                category.as_str()
            ))
        }
    }
}

/// **Trusted source for the run-profile gate** (SC-10, prd-merged/07 SC-21).
///
/// A run executes under a declared profile (e.g. a locked-down "review-safety"
/// profile) whose bounds *narrow* what the run may do. A capability category is
/// permitted only when it is within the profile's `permitted` bounds; a category
/// outside the bounds is **denied fail-closed**.
///
/// This is trusted run state resolved from the run's declared profile, never the
/// request payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunProfile {
    /// Stable name of the profile (for diagnostics/audit).
    name: String,
    /// Capability categories the profile permits this run to exercise.
    permitted: Vec<Category>,
}

impl RunProfile {
    /// Build a run profile from a name and the capability bounds it permits.
    pub fn new(name: impl Into<String>, permitted: impl IntoIterator<Item = Category>) -> Self {
        RunProfile { name: name.into(), permitted: permitted.into_iter().collect() }
    }

    /// Evaluate the run-profile gate fail-closed: a category outside the
    /// profile's permitted bounds is denied.
    fn decide(&self, category: Category) -> GateDecision {
        if self.permitted.contains(&category) {
            GateDecision::Allow
        } else {
            GateDecision::Deny(format!(
                "run profile {:?} does not permit the {} capability (fail-closed: outside the profile bounds)",
                self.name,
                category.as_str()
            ))
        }
    }
}

/// **Trusted source for the platform-permission gate** (SC-10, prd-merged/01
/// CR-3 `PlatformUnavailable`).
///
/// The host platform grants a set of OS-level capabilities (clipboard, camera,
/// notifications, …). A capability category the platform has not granted is
/// **unavailable** — reported as [`GateDecision::Unavailable`] →
/// [`CoreError::PlatformUnavailable`], distinct from a policy denial. A missing
/// grant fails closed: the capability is treated as absent, not present.
///
/// This is trusted platform state the host reports, never the request payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformPermissions {
    /// Capability categories the host OS has granted to this process.
    granted: Vec<Category>,
}

impl PlatformPermissions {
    /// Build a platform-permission set from the OS-granted capability categories.
    pub fn new(granted: impl IntoIterator<Item = Category>) -> Self {
        PlatformPermissions { granted: granted.into_iter().collect() }
    }

    /// Evaluate the platform-permission gate fail-closed: a category the platform
    /// has not granted is reported *unavailable* (`PlatformUnavailable`), not
    /// merely denied.
    fn decide(&self, category: Category) -> GateDecision {
        if self.granted.contains(&category) {
            GateDecision::Allow
        } else {
            GateDecision::Unavailable(format!(
                "the host platform has not granted the {} capability",
                category.as_str()
            ))
        }
    }
}

/// The real composed [`DecisionContext`]: evaluates the workspace-policy,
/// run-profile, and platform-permission gates against **trusted** workspace /
/// run / platform state (T037, SC-10).
///
/// This is what a live command (and every remote sync op, SS-7) installs via
/// [`PolicyEngine::with_context`] instead of [`AllowAll`]. Each gate is
/// fail-closed and reads only its trusted source; none reads the request
/// payload (review 048/050). Replay does **not** use this context — it
/// re-installs [`AllowAll`] and replays the recorded decisions
/// ([`PolicyEngine::from_snapshot`]), so the live gate sources are consulted
/// only during the original run.
#[derive(Debug, Clone, Default)]
pub struct ComposedDecisionContext {
    workspace_policy: WorkspacePolicy,
    run_profile: RunProfile,
    platform: PlatformPermissions,
}

impl ComposedDecisionContext {
    /// Compose the three trusted gate sources into a live decision context.
    pub fn new(
        workspace_policy: WorkspacePolicy,
        run_profile: RunProfile,
        platform: PlatformPermissions,
    ) -> Self {
        ComposedDecisionContext { workspace_policy, run_profile, platform }
    }
}

impl DecisionContext for ComposedDecisionContext {
    fn clone_box(&self) -> Box<dyn DecisionContext> {
        clone_decision_context(self)
    }

    fn workspace_policy(&self, category: Category) -> GateDecision {
        self.workspace_policy.decide(category)
    }

    fn run_profile(&self, category: Category) -> GateDecision {
        self.run_profile.decide(category)
    }

    fn platform_permission(&self, category: Category) -> GateDecision {
        self.platform.decide(category)
    }
}

/// The **manifest + resource capability subcheck** of SC-10 (review 006 P1).
///
/// This is deliberately *not* "the SC-10 gate". It enforces exactly three of
/// the seven SC-10 conjuncts:
///   - *manifest requests it* — the capability category is declared, and
///   - *resource matches allowlist* — the specific key/collection is in scope;
///   - plus the CR-4 immediate-revocation hook (a runtime revocation denies a
///     category before any manifest grant is consulted).
///
/// It owns the per-category allowance state so that the *non-ambient seams*
/// (time/random/ui) are routed through the **same** decision path as
/// storage/db: each must be requested by the manifest, then remains revocable
/// and counted like any other host call (prd-merged/07 zero-ambient; review
/// 006 P1).
#[derive(Debug, Clone)]
struct CapabilityCheck {
    /// Requested grants, cloned so revocation can mutate without touching the
    /// caller's manifest (revocation is per-engine/per-member, prd-merged/07
    /// SC-9/CR-4).
    capabilities: Capabilities,
    /// Per-category runtime allowance. `true` = currently granted, `false` =
    /// revoked (CR-4). A true bit is not itself a grant; the manifest request
    /// below must still pass for the call to proceed.
    allowed: [bool; CATEGORY_COUNT],
}

impl CapabilityCheck {
    /// Build the subcheck from a manifest's capabilities, after validating the
    /// storage glob grants (review 006 P2). All categories start granted; the
    /// manifest-declared / resource-allowlist gates then decide each call.
    fn from_capabilities(capabilities: &Capabilities) -> Result<Self> {
        validate_storage_grants(capabilities)?;
        Ok(CapabilityCheck {
            capabilities: capabilities.clone(),
            allowed: [true; CATEGORY_COUNT],
        })
    }

    fn is_allowed(&self, cat: Category) -> bool {
        self.allowed[cat.index()]
    }

    fn revoke(&mut self, cat: Category) {
        self.allowed[cat.index()] = false;
    }

    /// The capability/allowlist portion of a decision. Does not touch any
    /// counter and assumes the role/budget/`DecisionContext` gates already ran.
    fn check(&self, call: &HostCall) -> Result<()> {
        let cat = PolicyEngine::category_of(call);
        // CR-4: a runtime revocation denies the whole category before the
        // manifest grant is even consulted — including the time/random/ui
        // seams, which are revocable allowances, not ambient capabilities.
        if !self.is_allowed(cat) {
            return Err(revoked_error_for(call));
        }
        match call {
            HostCall::Storage { op, key } => {
                let scope = match op {
                    Access::Read => &self.capabilities.storage.read,
                    Access::Write => &self.capabilities.storage.write,
                };
                // Distinguish "no grant of this category at all" (the applet
                // forgot to declare it → CapabilityRequired) from "declared
                // some scopes but not this key" (→ PermissionDenied).
                if !category_declared_storage(&self.capabilities) {
                    return Err(capability_required(Category::Storage, *op, key));
                }
                if scope.iter().any(|prefix| prefix_matches(prefix, key)) {
                    Ok(())
                } else {
                    Err(permission_denied(Category::Storage, *op, key))
                }
            }
            HostCall::Db { op, collection } => {
                let scope = match op {
                    Access::Read => &self.capabilities.db.read,
                    Access::Write => &self.capabilities.db.write,
                };
                if !category_declared_db(&self.capabilities) {
                    return Err(capability_required(Category::Db, *op, collection));
                }
                if scope.iter().any(|c| c == collection) {
                    Ok(())
                } else {
                    Err(permission_denied(Category::Db, *op, collection))
                }
            }
            HostCall::Ui => {
                if self.capabilities.ui {
                    Ok(())
                } else {
                    // `ui` is a single boolean grant, not a list of scopes, so
                    // an absent grant is a flat-out denial rather than a
                    // "you forgot to declare a scope" CapabilityRequired.
                    Err(CoreError::PermissionDenied(
                        "ui capability not granted in manifest (capabilities.ui = false)"
                            .to_string(),
                    ))
                }
            }
            HostCall::Time => {
                if self.capabilities.time {
                    Ok(())
                } else {
                    Err(CoreError::PermissionDenied(
                        "time capability not granted in manifest (capabilities.time = false)"
                            .to_string(),
                    ))
                }
            }
            HostCall::Random => {
                if self.capabilities.random {
                    Ok(())
                } else {
                    Err(CoreError::PermissionDenied(
                        "random capability not granted in manifest (capabilities.random = false)"
                            .to_string(),
                    ))
                }
            }
            HostCall::Resource { kind, .. } => {
                if self.capabilities.resources.is_empty() {
                    return Err(resource_capability_required(kind));
                }
                if self.capabilities.resources.iter().any(|k| k == kind) {
                    Ok(())
                } else {
                    Err(resource_permission_denied(kind))
                }
            }
            HostCall::ResourceRead { asset_id } => {
                if self.capabilities.resources.is_empty() {
                    return Err(resource_capability_required(asset_id));
                }
                Ok(())
            }
            HostCall::ResourceMaterialize { asset_id, .. } => {
                if self.capabilities.resources.is_empty() {
                    return Err(resource_capability_required(asset_id));
                }
                Ok(())
            }
        }
    }
}

/// The capability + RBAC decision engine for a single run.
///
/// Built from a `&Manifest` (the requested grants) plus an [`ActorContext`]
/// (who is running, with what role) and a [`DecisionContext`] (the
/// workspace-policy / run-profile / platform-permission gates).
/// [`PolicyEngine::check`] is called once per host call the runtime makes, in
/// order, and composes **all** of SC-10: the [`DecisionContext`] gates, the
/// actor-role gate, the SC-2 host-call flood guard, and the manifest+resource
/// [`CapabilityCheck`] subcheck.
#[derive(Debug)]
pub struct PolicyEngine {
    /// The manifest+resource capability subcheck (review 006 P1): NOT the whole
    /// decision — see the crate docs and [`CapabilityCheck`].
    capability: CapabilityCheck,
    /// The non-capability SC-10 gates (workspace policy / run profile /
    /// platform permission). M0a defaults to the permissive [`AllowAll`] stub.
    context: Box<dyn DecisionContext>,
    /// Whether the actor's role may run code at all (SC-10).
    can_run: bool,
    /// Max host calls permitted this run (`manifest.limits.max_host_calls`).
    max_host_calls: u64,
    /// Host calls counted so far this run.
    host_calls: u64,
}

impl Clone for PolicyEngine {
    fn clone(&self) -> Self {
        // `DecisionContext` is a trait object so the engine can't derive Clone.
        // Cloning MUST preserve the exact decision context (review 023 P2):
        // re-installing a bare `AllowAll` here would make a clone of a
        // context-scoped engine *more permissive* than the original — a
        // fail-OPEN permission bypass once M0b passes real workspace/run-profile/
        // platform gates through `with_context`. `clone_box` carries the real
        // context across, so the clone makes identical decisions to the original.
        PolicyEngine {
            capability: self.capability.clone(),
            context: self.context.clone_box(),
            can_run: self.can_run,
            max_host_calls: self.max_host_calls,
            host_calls: self.host_calls,
        }
    }
}

impl PolicyEngine {
    /// Build an engine for `actor` running under `manifest`, with the M0a
    /// permissive [`DecisionContext`] ([`AllowAll`]).
    ///
    /// Returns `Err` if the manifest's storage glob grants are overly broad or
    /// malformed (review 006 P2) — a bare `*`, an unscoped/empty grant, or a
    /// glob with `*` anywhere but the end is rejected fail-closed at build time
    /// rather than silently granting more than intended.
    pub fn new(manifest: &Manifest, actor: &ActorContext) -> Result<Self> {
        Self::with_context(manifest, actor, Box::new(AllowAll))
    }

    /// Build an engine with an explicit [`DecisionContext`] (the
    /// workspace-policy / run-profile / platform-permission gates). M0b wires a
    /// real context here; M0a callers use [`new`](Self::new).
    pub fn with_context(
        manifest: &Manifest,
        actor: &ActorContext,
        context: Box<dyn DecisionContext>,
    ) -> Result<Self> {
        Ok(PolicyEngine {
            capability: CapabilityCheck::from_capabilities(&manifest.capabilities)?,
            context,
            can_run: role_can_run(actor.role),
            max_host_calls: manifest.limits.max_host_calls,
            host_calls: 0,
        })
    }

    /// Build an engine from a recorded [`PermissionSnapshot`] (review 009 P1
    /// CR-9). Replay must re-derive the permission decision the run was recorded
    /// under — *not* whatever the live manifest grants now — so a denied call
    /// stays denied (and an allowed call stays allowed) even if grants/role/budget
    /// have since changed. The runtime builds its replay-mode engine from the
    /// record's snapshot, making the recorded decision authoritative.
    ///
    /// Replay re-installs the permissive [`AllowAll`] context: the recorded
    /// snapshot already captured the *outcome* of the non-capability gates at
    /// record time (a call that those gates denied was never recorded as
    /// allowed), so replay must not re-impose today's workspace/run-profile
    /// policy on a historical run.
    ///
    /// Returns `Err` if the snapshot's stored grants fail glob validation —
    /// which should never happen for a snapshot this crate produced, but is
    /// enforced fail-closed in case a record was tampered with (review 006 P2).
    pub fn from_snapshot(snapshot: &PermissionSnapshot) -> Result<Self> {
        Ok(PolicyEngine {
            capability: CapabilityCheck::from_capabilities(&snapshot.capabilities)?,
            context: Box::new(AllowAll),
            can_run: snapshot.can_run,
            max_host_calls: snapshot.max_host_calls,
            host_calls: 0,
        })
    }

    /// Capture the engine's evaluated permission state as a [`PermissionSnapshot`]
    /// for the run record (review 009 P1 CR-9). Reflects the *current* grants,
    /// role-can-run, and host-call budget (revocations applied so far are not part
    /// of the static snapshot; M0a has no mid-run revocation on the spine path).
    pub fn snapshot(&self) -> PermissionSnapshot {
        PermissionSnapshot {
            capabilities: self.capability.capabilities.clone(),
            can_run: self.can_run,
            max_host_calls: self.max_host_calls,
        }
    }

    /// Revoke a capability category for the rest of this run.
    ///
    /// prd-merged/07 CR-4: revocation takes effect immediately — the very next
    /// [`check`](Self::check) of a call in `cat` is denied with
    /// `PermissionDenied`, even though the manifest still nominally grants it.
    /// This applies uniformly to the time/random/ui seams: revoking `Time`
    /// denies the next manifest-granted `ctx.time.now()`, proving the seam is
    /// revocable after grant and not ambient (review 006 P1).
    pub fn revoke(&mut self, cat: Category) {
        self.capability.revoke(cat);
    }

    /// Number of host calls counted so far this run.
    pub fn host_calls(&self) -> u64 {
        self.host_calls
    }

    /// Evaluate **only** the non-capability SC-10 [`DecisionContext`] gates
    /// (workspace policy / run profile / platform permission) for `call`,
    /// without touching the role gate, the host-call budget, or the
    /// capability/resource subcheck.
    ///
    /// ## Why this is public: the replayable-denial seam (review 023 P1)
    ///
    /// A [`PermissionSnapshot`] captures only `capabilities` / `can_run` /
    /// `max_host_calls` — it does **not** capture the context-gate outcome.
    /// Replay rebuilds policy with the permissive [`AllowAll`] context
    /// ([`from_snapshot`](Self::from_snapshot)), so a call that a *real* context
    /// denied at record time would be **allowed** on replay. That is a fail-open
    /// hole: the recorder writes a `{"denied": ...}` entry for the original
    /// denial, but a replay that re-allows the call would then consume that
    /// denied entry as if it were a normal response (corrupting the trace).
    ///
    /// To keep the denial deterministic without bloating the snapshot, the
    /// **runtime** must, in record mode and *before* the live-allowed path runs,
    /// call this method and — if it returns `Err` — record that denial through
    /// the same `record_denial` channel as a manifest-scope denial, then
    /// propagate the error (exactly the shape of `HostContext::check_or_record_denial`
    /// in `forge/crates/runtime/src/host.rs`). On replay the recorded denial is
    /// consumed and its error reconstructed at the cursor, so a context-only
    /// denial replays identically to a manifest-scope denial. This method is the
    /// policy-side surface the runtime needs so the seam is **not** silently
    /// fail-open; the recording/replay wiring itself is a runtime-crate concern.
    ///
    /// Note: [`check`](Self::check) already runs these same gates inline, so a
    /// caller that records via `check_or_record_denial` around `check` is
    /// already covered — this method exists for callers that need the context
    /// decision in isolation (e.g. to snapshot it explicitly).
    pub fn check_context_gates(&self, call: &HostCall) -> Result<()> {
        let cat = Self::category_of(call);
        gate(self.context.workspace_policy(cat), "workspace policy", cat)?;
        gate(self.context.run_profile(cat), "run profile", cat)?;
        gate(self.context.platform_permission(cat), "platform permission", cat)?;
        Ok(())
    }

    /// The capability category a `call` belongs to.
    pub fn category_of(call: &HostCall) -> Category {
        match call {
            HostCall::Storage { .. } => Category::Storage,
            HostCall::Db { .. } => Category::Db,
            HostCall::Ui => Category::Ui,
            HostCall::Time => Category::Time,
            HostCall::Random => Category::Random,
            HostCall::Resource { .. }
            | HostCall::ResourceRead { .. }
            | HostCall::ResourceMaterialize { .. } => Category::Resource,
        }
    }

    /// Decide whether `call` is permitted, and on success count it against the
    /// host-call budget.
    ///
    /// Order of checks (prd-merged/07 SC-10 — *all seven conjuncts* must pass):
    /// 1. the actor's role permits running at all;
    /// 2. the budget (`max_host_calls`) has not been exhausted (SC-2);
    /// 3. the workspace-policy / run-profile / platform-permission gates
    ///    ([`DecisionContext`]; M0a-permissive but fail-closed-capable);
    /// 4. the category has not been revoked (CR-4), the manifest grants the
    ///    capability category, and the specific resource matches the allowlist
    ///    (SC-8) — the [`CapabilityCheck`] subcheck.
    ///
    /// On any failure no call is counted; the budget is only consumed by calls
    /// that are actually allowed to proceed.
    pub fn check(&mut self, call: &HostCall) -> Result<()> {
        let cat = Self::category_of(call);

        // 1. Role gate (SC-10): read-only roles cannot run code, so no host
        //    call they would issue is ever permitted.
        if !self.can_run {
            return Err(CoreError::PermissionDenied(format!(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for {} call",
                cat.as_str()
            )));
        }

        // 2. Budget gate (SC-2): the (n+1)th call once n == max is refused.
        if self.host_calls >= self.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached",
                self.max_host_calls
            )));
        }

        // 3. Non-capability SC-10 gates (review 006 P1): workspace policy, run
        //    profile, platform permission. M0a stubs allow, but a real context
        //    can deny any of them fail-closed here. Same evaluation the runtime
        //    can call in isolation via `check_context_gates` to record a
        //    context-only denial deterministically (review 023 P1).
        self.check_context_gates(call)?;

        // 4. Capability subcheck (SC-8/CR-4): manifest grant + resource match,
        //    including the revocable seam allowances.
        self.capability.check(call)?;

        // Only count calls that pass every gate.
        self.host_calls += 1;
        Ok(())
    }
}

/// Map a [`GateDecision`] from a [`DecisionContext`] hook to a `Result`, naming
/// the gate and category in the denial so the seam is auditable.
///
/// A [`GateDecision::Deny`] is a policy refusal → [`CoreError::PermissionDenied`].
/// A [`GateDecision::Unavailable`] is a missing platform capability →
/// [`CoreError::PlatformUnavailable`] (prd-merged/01 CR-3): the run is not
/// *refused* by policy, the capability simply does not exist on this host.
fn gate(decision: GateDecision, gate_name: &str, cat: Category) -> Result<()> {
    match decision {
        GateDecision::Allow => Ok(()),
        GateDecision::Deny(reason) => Err(CoreError::PermissionDenied(format!(
            "{gate_name} gate denied {cat} capability: {reason}",
            cat = cat.as_str(),
        ))),
        GateDecision::Unavailable(reason) => Err(CoreError::PlatformUnavailable(format!(
            "{gate_name} gate: {cat} capability unavailable on this platform: {reason}",
            cat = cat.as_str(),
        ))),
    }
}

/// A storage capability is "declared" iff the applet listed at least one
/// read or write scope. An empty `StorageGrant` means the applet never
/// requested storage → `CapabilityRequired`.
fn category_declared_storage(caps: &Capabilities) -> bool {
    !caps.storage.read.is_empty() || !caps.storage.write.is_empty()
}

fn category_declared_db(caps: &Capabilities) -> bool {
    !caps.db.read.is_empty() || !caps.db.write.is_empty()
}

/// Validate every storage glob grant (review 006 P2).
///
/// A grant is rejected fail-closed if it is:
///   - empty / whitespace-only (an unscoped grant);
///   - a bare `*` (or `**`, etc.) — a lone wildcard must **not** silently mean
///     "full storage access"; applets must scope grants to a prefix;
///   - a prefix glob that would still match everything (`*` with nothing before
///     it, e.g. `*notes`), since the prefix before `*` is empty;
///   - malformed — contains a `*` anywhere except as the final character (M0a
///     only supports a trailing-`*` prefix glob, prd-merged/07 SC-8 `path/*`).
///
/// Exact (non-glob) grants like `config` are always fine; trailing-`*` grants
/// like `app/*` are fine because they carry a non-empty `app/` prefix.
fn validate_storage_grants(caps: &Capabilities) -> Result<()> {
    for grant in caps.storage.read.iter().chain(caps.storage.write.iter()) {
        validate_storage_grant(grant)?;
    }
    Ok(())
}

fn validate_storage_grant(grant: &str) -> Result<()> {
    if grant.trim().is_empty() {
        return Err(CoreError::ValidationError(
            "storage grant is empty; grants must be applet-scoped (e.g. \"app/*\")".to_string(),
        ));
    }
    let star_count = grant.matches('*').count();
    if star_count == 0 {
        // Exact-key grant: always well-scoped.
        return Ok(());
    }
    // A glob: the single `*` must be the final character (trailing-prefix glob).
    if star_count > 1 || !grant.ends_with('*') {
        return Err(CoreError::ValidationError(format!(
            "malformed storage grant {grant:?}: the only supported glob is a single trailing \"*\" (e.g. \"app/*\")"
        )));
    }
    // Trailing-`*` glob: the prefix before `*` must be non-empty, otherwise the
    // grant matches every key — a bare `*` (or `*`-prefixed) "full access" grant
    // is exactly what we reject (review 006 P2).
    let prefix = &grant[..grant.len() - 1];
    if prefix.is_empty() {
        return Err(CoreError::ValidationError(
            "overly broad storage grant \"*\": a bare wildcard would grant full storage access; \
             scope it to an applet prefix (e.g. \"app/*\")"
                .to_string(),
        ));
    }
    Ok(())
}

/// Prefix/glob match for storage keys. A grant of `app/*` matches any key
/// under `app/`; a bare grant (`config`) matches exactly that key. The `*`
/// suffix is the only glob form M0a supports (prd-merged/07 SC-8 `path/*`).
///
/// Callers must have validated grants with [`validate_storage_grant`] first, so
/// a bare `*` never reaches here as an allowed grant.
fn prefix_matches(grant: &str, key: &str) -> bool {
    if let Some(prefix) = grant.strip_suffix('*') {
        key.starts_with(prefix)
    } else {
        grant == key
    }
}

fn capability_required(cat: Category, op: Access, resource: &str) -> CoreError {
    CoreError::CapabilityRequired(format!(
        "manifest declares no {cat} capability; cannot {op} {resource:?} (add a capabilities.{cat}.{op} grant)",
        cat = cat.as_str(),
        op = op.as_str(),
    ))
}

fn permission_denied(cat: Category, op: Access, resource: &str) -> CoreError {
    CoreError::PermissionDenied(format!(
        "{op} on {cat} {resource:?} is not within any granted {cat}.{op} scope",
        cat = cat.as_str(),
        op = op.as_str(),
    ))
}

/// Build the "revoked" denial for any host call, naming the resource where one
/// exists (storage/db) and the category otherwise (ui/time/random).
fn revoked_error_for(call: &HostCall) -> CoreError {
    match call {
        HostCall::Storage { op, key } => revoked_error(Category::Storage, *op, key),
        HostCall::Db { op, collection } => revoked_error(Category::Db, *op, collection),
        HostCall::Ui => CoreError::PermissionDenied("ui capability has been revoked".to_string()),
        HostCall::Time => {
            CoreError::PermissionDenied("time capability has been revoked".to_string())
        }
        HostCall::Random => {
            CoreError::PermissionDenied("random capability has been revoked".to_string())
        }
        HostCall::Resource { kind, .. } => CoreError::PermissionDenied(format!(
            "resource capability has been revoked; cannot invoke {kind:?}"
        )),
        HostCall::ResourceRead { asset_id } => CoreError::PermissionDenied(format!(
            "resource capability has been revoked; cannot read {asset_id:?}"
        )),
        HostCall::ResourceMaterialize { asset_id, .. } => CoreError::PermissionDenied(format!(
            "resource capability has been revoked; cannot materialize {asset_id:?}"
        )),
    }
}

fn resource_capability_required(kind: &str) -> CoreError {
    CoreError::CapabilityRequired(format!(
        "manifest declares no resource capability; cannot access {kind:?} (add capabilities.resources)"
    ))
}

fn resource_permission_denied(kind: &str) -> CoreError {
    CoreError::PermissionDenied(format!(
        "resource kind {kind:?} is not listed in capabilities.resources"
    ))
}

fn revoked_error(cat: Category, op: Access, resource: &str) -> CoreError {
    CoreError::PermissionDenied(format!(
        "{cat} capability has been revoked; cannot {op} {resource:?}",
        cat = cat.as_str(),
        op = op.as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{DbGrant, Limits, StorageGrant};

    /// Manifest with the given capabilities and a `max_host_calls` budget.
    fn manifest_with(caps: Capabilities, max_host_calls: u64) -> Manifest {
        Manifest {
            entrypoint: "src/main.ts".into(),
            min_api: "forge-api@0.1".into(),
            deterministic: true,
            capabilities: caps,
            limits: Limits { max_host_calls, ..Limits::default() },
            compatibility: Default::default(),
        }
    }

    fn caps(
        storage_read: &[&str],
        storage_write: &[&str],
        db_read: &[&str],
        db_write: &[&str],
        ui: bool,
    ) -> Capabilities {
        Capabilities {
            storage: StorageGrant {
                read: storage_read.iter().map(|s| s.to_string()).collect(),
                write: storage_write.iter().map(|s| s.to_string()).collect(),
            },
            db: DbGrant {
                read: db_read.iter().map(|s| s.to_string()).collect(),
                write: db_write.iter().map(|s| s.to_string()).collect(),
            },
            ui,
            time: true,
            random: true,
            // The capability-engine tests don't exercise net; an empty net grant
            // keeps these manifests "no network" (net is gated by NetPolicy, a
            // separate decision path from this CapabilityCheck).
            net: forge_domain::NetGrant::default(),
            // Files are likewise gated by the runtime's ctx.files host call, not
            // this CapabilityCheck; an empty grant keeps these manifests "no files".
            ..Capabilities::default()
        }
    }

    fn owner() -> ActorContext {
        ActorContext::owner("dev")
    }

    fn engine(caps: Capabilities, actor: ActorContext) -> PolicyEngine {
        PolicyEngine::new(&manifest_with(caps, 10_000), &actor).expect("valid grants")
    }

    // --- Storage grants -----------------------------------------------------

    #[test]
    fn granted_storage_prefix_is_allowed() {
        let mut e = engine(caps(&["app/*"], &["app/*"], &[], &[], true), owner());
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/notes/1".into() }).is_ok());
        assert!(e.check(&HostCall::Storage { op: Access::Write, key: "app/notes/2".into() }).is_ok());
    }

    #[test]
    fn exact_storage_grant_matches_only_exact_key() {
        let mut e = engine(caps(&["config"], &[], &[], &[], true), owner());
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "config".into() }).is_ok());
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "config/extra".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn ungranted_storage_key_is_denied() {
        // Storage IS declared (some scopes), but this key is outside them →
        // PermissionDenied, not CapabilityRequired.
        let mut e = engine(caps(&["app/*"], &["app/*"], &[], &[], true), owner());
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "secret/keys".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("secret/keys"), "message names the key: {err}");
        assert!(err.to_string().contains("storage"), "message names the scope: {err}");
    }

    #[test]
    fn write_grant_does_not_imply_read() {
        // Only a write scope is granted; a read of the same key is denied.
        let mut e = engine(caps(&[], &["app/*"], &[], &[], true), owner());
        assert!(e.check(&HostCall::Storage { op: Access::Write, key: "app/x".into() }).is_ok());
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn storage_not_declared_is_capability_required() {
        // No storage scopes at all → the applet forgot to request the
        // category → CapabilityRequired (distinct from PermissionDenied).
        let mut e = engine(caps(&[], &[], &["tasks"], &[], true), owner());
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
        assert!(err.to_string().contains("storage"), "{err}");
    }

    // --- Glob grant validation (review 006 P2) ------------------------------

    #[test]
    fn bare_star_storage_grant_is_rejected() {
        // A lone "*" must not silently mean full storage access.
        let err = PolicyEngine::new(
            &manifest_with(caps(&["*"], &[], &[], &[], true), 10_000),
            &owner(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("full storage access"), "{err}");
    }

    #[test]
    fn bare_star_write_grant_is_rejected() {
        let err = PolicyEngine::new(
            &manifest_with(caps(&[], &["*"], &[], &[], true), 10_000),
            &owner(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn empty_storage_grant_is_rejected() {
        let err = PolicyEngine::new(
            &manifest_with(caps(&["   "], &[], &[], &[], true), 10_000),
            &owner(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("applet-scoped"), "{err}");
    }

    #[test]
    fn malformed_glob_with_inner_star_is_rejected() {
        // `*` is only supported as a trailing char; an inner `*` is malformed.
        let err = PolicyEngine::new(
            &manifest_with(caps(&["app/*/secret"], &[], &[], &[], true), 10_000),
            &owner(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("malformed"), "{err}");
    }

    #[test]
    fn malformed_glob_with_leading_star_is_rejected() {
        // `*notes` would match everything (empty prefix) and isn't a trailing
        // glob → rejected as malformed.
        let err = PolicyEngine::new(
            &manifest_with(caps(&["*notes"], &[], &[], &[], true), 10_000),
            &owner(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn malformed_glob_with_double_star_is_rejected() {
        let err = PolicyEngine::new(
            &manifest_with(caps(&["app/**"], &[], &[], &[], true), 10_000),
            &owner(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn scoped_prefix_and_exact_grants_are_accepted() {
        // A trailing-`*` prefix glob and an exact key are both well-formed.
        assert!(PolicyEngine::new(
            &manifest_with(caps(&["app/*", "config"], &["app/*"], &[], &[], true), 10_000),
            &owner(),
        )
        .is_ok());
    }

    #[test]
    fn from_snapshot_rejects_tampered_broad_grant() {
        // A snapshot whose stored grant was tampered to a bare "*" is rejected
        // fail-closed on replay (review 006 P2).
        let snap = PermissionSnapshot {
            capabilities: caps(&["*"], &[], &[], &[], true),
            can_run: true,
            max_host_calls: 10,
        };
        assert_eq!(PolicyEngine::from_snapshot(&snap).unwrap_err().code(), "ValidationError");
    }

    // --- Db grants ----------------------------------------------------------

    #[test]
    fn granted_db_collection_is_allowed() {
        let mut e = engine(caps(&[], &[], &["tasks"], &["tasks"], true), owner());
        assert!(e.check(&HostCall::Db { op: Access::Read, collection: "tasks".into() }).is_ok());
        assert!(e.check(&HostCall::Db { op: Access::Write, collection: "tasks".into() }).is_ok());
    }

    #[test]
    fn ungranted_db_collection_is_denied() {
        let mut e = engine(caps(&[], &[], &["tasks"], &["tasks"], true), owner());
        let err = e
            .check(&HostCall::Db { op: Access::Write, collection: "users".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("users"), "message names the collection: {err}");
    }

    #[test]
    fn db_collections_are_exact_not_prefix() {
        // Db grants are named collections, not globs: "tasks" does not cover
        // "tasks_archive".
        let mut e = engine(caps(&[], &[], &["tasks"], &[], true), owner());
        let err = e
            .check(&HostCall::Db { op: Access::Read, collection: "tasks_archive".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
    }

    #[test]
    fn db_not_declared_is_capability_required() {
        let mut e = engine(caps(&["app/*"], &[], &[], &[], true), owner());
        let err = e
            .check(&HostCall::Db { op: Access::Read, collection: "tasks".into() })
            .unwrap_err();
        assert_eq!(err.code(), "CapabilityRequired");
        assert!(err.to_string().contains("db"), "{err}");
    }

    // --- UI -----------------------------------------------------------------

    #[test]
    fn ui_allowed_when_granted() {
        let mut e = engine(caps(&[], &[], &[], &[], true), owner());
        assert!(e.check(&HostCall::Ui).is_ok());
    }

    #[test]
    fn ui_denied_when_not_granted() {
        let mut e = engine(caps(&[], &[], &[], &[], false), owner());
        let err = e.check(&HostCall::Ui).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("ui"), "{err}");
    }

    fn caps_without_seams(ui: bool) -> Capabilities {
        Capabilities {
            ui,
            ..Capabilities::default()
        }
    }

    // --- Non-ambient seams: explicit grants, revocable (review 006 P1) -------

    #[test]
    fn seams_allowed_when_granted() {
        // Time/Random are routed through the capability decision path and are
        // allowed only because this test manifest explicitly grants them.
        let mut e = engine(caps(&[], &[], &[], &[], false), owner());
        assert!(e.check(&HostCall::Time).is_ok());
        assert!(e.check(&HostCall::Random).is_ok());
    }

    #[test]
    fn seams_denied_when_not_granted() {
        let mut e = engine(caps_without_seams(false), owner());

        let err = e.check(&HostCall::Time).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("time"), "{err}");

        let err = e.check(&HostCall::Random).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("random"), "{err}");
    }

    #[test]
    fn time_seam_denied_when_revoked() {
        // The seam is a REVOCABLE explicit grant, not an ambient capability:
        // revoking it denies the next call (zero-ambient, prd-merged/07).
        let mut e = engine(caps(&[], &[], &[], &[], false), owner());
        assert!(e.check(&HostCall::Time).is_ok());
        e.revoke(Category::Time);
        let err = e.check(&HostCall::Time).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("revoked"), "{err}");
    }

    #[test]
    fn random_seam_denied_when_revoked() {
        let mut e = engine(caps(&[], &[], &[], &[], false), owner());
        assert!(e.check(&HostCall::Random).is_ok());
        e.revoke(Category::Random);
        let err = e.check(&HostCall::Random).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("revoked"), "{err}");
    }

    // --- DecisionContext seam (review 006 P1) -------------------------------

    /// A context that denies one named gate for a given category, allowing the
    /// rest — used to prove each missing SC-10 gate has a fail-closed seam.
    #[derive(Debug, Clone)]
    struct DenyGate {
        which: &'static str,
        category: Category,
    }

    impl DecisionContext for DenyGate {
        fn clone_box(&self) -> Box<dyn DecisionContext> {
            clone_decision_context(self)
        }
        fn workspace_policy(&self, category: Category) -> GateDecision {
            self.maybe_deny("workspace_policy", category)
        }
        fn run_profile(&self, category: Category) -> GateDecision {
            self.maybe_deny("run_profile", category)
        }
        fn platform_permission(&self, category: Category) -> GateDecision {
            self.maybe_deny("platform_permission", category)
        }
    }

    impl DenyGate {
        fn maybe_deny(&self, gate: &str, category: Category) -> GateDecision {
            if gate == self.which && category == self.category {
                GateDecision::Deny("test-denied".into())
            } else {
                GateDecision::Allow
            }
        }
    }

    fn engine_with(ctx: Box<dyn DecisionContext>, c: Capabilities) -> PolicyEngine {
        PolicyEngine::with_context(&manifest_with(c, 10_000), &owner(), ctx).expect("valid grants")
    }

    #[test]
    fn allow_all_context_permits_all_gates() {
        // The M0a default stub allows the otherwise-granted call.
        let mut e = engine_with(Box::new(AllowAll), caps(&["app/*"], &[], &[], &[], true));
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_ok());
    }

    #[test]
    fn workspace_policy_gate_can_deny_fail_closed() {
        let ctx = Box::new(DenyGate { which: "workspace_policy", category: Category::Storage });
        let mut e = engine_with(ctx, caps(&["app/*"], &[], &[], &[], true));
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("workspace policy"), "{err}");
    }

    #[test]
    fn run_profile_gate_can_deny_fail_closed() {
        let ctx = Box::new(DenyGate { which: "run_profile", category: Category::Db });
        let mut e = engine_with(ctx, caps(&[], &[], &["tasks"], &["tasks"], true));
        let err = e
            .check(&HostCall::Db { op: Access::Write, collection: "tasks".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("run profile"), "{err}");
    }

    #[test]
    fn platform_permission_gate_can_deny_fail_closed() {
        let ctx = Box::new(DenyGate { which: "platform_permission", category: Category::Ui });
        let mut e = engine_with(ctx, caps(&[], &[], &[], &[], true));
        let err = e.check(&HostCall::Ui).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("platform permission"), "{err}");
    }

    #[test]
    fn decision_context_gate_does_not_consume_budget() {
        let ctx = Box::new(DenyGate { which: "workspace_policy", category: Category::Storage });
        let mut e = PolicyEngine::with_context(
            &manifest_with(caps(&["app/*"], &[], &[], &[], true), 5),
            &owner(),
            ctx,
        )
        .unwrap();
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_err());
        assert_eq!(e.host_calls(), 0, "a gate denial must not be counted");
    }

    // --- Real trusted-source SC-10 gates (T037) -----------------------------
    //
    // ComposedDecisionContext reads TRUSTED workspace/run/platform state, never
    // the request payload (review 048/050). Each gate is proven deny + allow +
    // fail-closed-on-missing-input.

    /// A `ComposedDecisionContext` whose trusted state grants everything the M0a
    /// host-call categories need, so a single gate can be narrowed per test.
    fn composed_all_granted() -> ComposedDecisionContext {
        let all = [
            Category::Storage,
            Category::Db,
            Category::Ui,
            Category::Time,
            Category::Random,
            Category::Resource,
        ];
        ComposedDecisionContext::new(
            WorkspacePolicy::new(all, []),
            RunProfile::new("default", all),
            PlatformPermissions::new(all),
        )
    }

    // workspace-policy gate -------------------------------------------------

    #[test]
    fn workspace_policy_allows_explicitly_granted_category() {
        let wp = WorkspacePolicy::new([Category::Storage], []);
        assert_eq!(wp.decide(Category::Storage), GateDecision::Allow);
    }

    #[test]
    fn workspace_policy_denies_explicitly_forbidden_category() {
        // Deny wins even when also present in the allow list.
        let wp = WorkspacePolicy::new([Category::Storage], [Category::Storage]);
        match wp.decide(Category::Storage) {
            GateDecision::Deny(r) => assert!(r.contains("forbids"), "{r}"),
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[test]
    fn workspace_policy_fail_closed_on_missing_input() {
        // A category absent from BOTH lists is denied fail-closed: the workspace
        // never positively granted it.
        let wp = WorkspacePolicy::new([Category::Db], []);
        match wp.decide(Category::Storage) {
            GateDecision::Deny(r) => assert!(r.contains("fail-closed"), "{r}"),
            other => panic!("expected fail-closed deny, got {other:?}"),
        }
        // The default (empty) policy denies everything.
        assert!(matches!(
            WorkspacePolicy::default().decide(Category::Ui),
            GateDecision::Deny(_)
        ));
    }

    // run-profile gate ------------------------------------------------------

    #[test]
    fn run_profile_allows_in_bounds_and_denies_out_of_bounds() {
        let rp = RunProfile::new("review-safety", [Category::Ui, Category::Time]);
        assert_eq!(rp.decide(Category::Ui), GateDecision::Allow);
        match rp.decide(Category::Db) {
            GateDecision::Deny(r) => {
                assert!(r.contains("review-safety"), "names the profile: {r}");
                assert!(r.contains("fail-closed"), "{r}");
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[test]
    fn run_profile_fail_closed_on_missing_input() {
        // A profile with empty bounds permits nothing.
        assert!(matches!(
            RunProfile::default().decide(Category::Storage),
            GateDecision::Deny(_)
        ));
    }

    // platform-permission gate ----------------------------------------------

    #[test]
    fn platform_permission_allows_granted_and_reports_unavailable_otherwise() {
        let pp = PlatformPermissions::new([Category::Ui]);
        assert_eq!(pp.decide(Category::Ui), GateDecision::Allow);
        // A capability the OS has not granted is UNAVAILABLE, not merely denied.
        match pp.decide(Category::Storage) {
            GateDecision::Unavailable(r) => assert!(r.contains("not granted"), "{r}"),
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn platform_permission_fail_closed_maps_to_platform_unavailable() {
        // Through the engine, a missing platform grant surfaces PlatformUnavailable
        // (not PermissionDenied) — the capability is absent, not refused.
        let mut granted = composed_all_granted();
        granted.platform = PlatformPermissions::new([]); // OS grants nothing
        let mut e = engine_with(Box::new(granted), caps(&[], &[], &[], &[], true));
        let err = e.check(&HostCall::Ui).unwrap_err();
        assert_eq!(err.code(), "PlatformUnavailable", "{err}");
        assert!(err.to_string().contains("platform permission"), "{err}");
    }

    // composed live path ----------------------------------------------------

    #[test]
    fn composed_context_allows_when_all_trusted_sources_permit() {
        let mut e = engine_with(
            Box::new(composed_all_granted()),
            caps(&["app/*"], &[], &[], &[], true),
        );
        assert!(e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .is_ok());
    }

    #[test]
    fn composed_workspace_policy_deny_blocks_live_command() {
        // Prove a real workspace-policy deny actually blocks a live command.
        let mut ctx = composed_all_granted();
        ctx.workspace_policy = WorkspacePolicy::new([], [Category::Storage]);
        let mut e = engine_with(Box::new(ctx), caps(&["app/*"], &[], &[], &[], true));
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("workspace policy"), "{err}");
        assert_eq!(e.host_calls(), 0, "a gate denial is not counted");
    }

    #[test]
    fn composed_run_profile_deny_blocks_live_command() {
        let mut ctx = composed_all_granted();
        ctx.run_profile = RunProfile::new("review-safety", [Category::Ui]); // no db
        let mut e = engine_with(Box::new(ctx), caps(&[], &[], &["tasks"], &["tasks"], true));
        let err = e
            .check(&HostCall::Db { op: Access::Write, collection: "tasks".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("run profile"), "{err}");
    }

    #[test]
    fn composed_first_failing_gate_wins_workspace_before_manifest() {
        // ORDER (SC-10): the workspace-policy gate (gate 2) is evaluated before
        // the manifest+resource subcheck (gate 3). When BOTH would deny — the
        // workspace forbids storage AND the manifest does not grant the key —
        // the workspace-policy denial is the one surfaced (first failing wins).
        let mut ctx = composed_all_granted();
        ctx.workspace_policy = WorkspacePolicy::new([], [Category::Storage]);
        // Manifest grants only `app/*`; the call targets `secret/x` (outside it),
        // so the capability subcheck would ALSO deny — but it never runs.
        let mut e = engine_with(Box::new(ctx), caps(&["app/*"], &[], &[], &[], true));
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "secret/x".into() })
            .unwrap_err();
        assert!(
            err.to_string().contains("workspace policy"),
            "first failing gate (workspace policy) must win over the manifest subcheck: {err}"
        );
    }

    #[test]
    fn composed_context_clone_preserves_trusted_denials() {
        // A clone of a ComposedDecisionContext-scoped engine must make identical
        // decisions (review 023 P2): no fail-open fallback to AllowAll.
        let mut ctx = composed_all_granted();
        ctx.workspace_policy = WorkspacePolicy::new([], [Category::Ui]);
        let original = engine_with(Box::new(ctx), caps(&[], &[], &[], &[], true));
        let mut clone = original.clone();
        let err = clone.check(&HostCall::Ui).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("workspace policy"), "{err}");
    }

    // --- Clone preserves the decision context (review 023 P2) ---------------

    #[test]
    fn clone_of_context_scoped_engine_preserves_denials() {
        // A clone of a context-scoped engine MUST make identical decisions: if
        // the original denies via a non-AllowAll DecisionContext, so must the
        // clone. Cloning must NOT silently fall back to AllowAll (fail-open).
        let ctx = Box::new(DenyGate { which: "workspace_policy", category: Category::Storage });
        let original = engine_with(ctx, caps(&["app/*"], &[], &[], &[], true));

        let mut clone = original.clone();
        let call = HostCall::Storage { op: Access::Read, key: "app/x".into() };

        // The original denies this call; the clone must deny it the SAME way.
        let err = clone.check(&call).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(
            err.to_string().contains("workspace policy"),
            "clone must surface the context denial, not AllowAll-permit: {err}"
        );

        // And a call the context allows still passes through the rest of the
        // gates on the clone (the context survived, it didn't widen access).
        assert!(clone
            .check(&HostCall::Storage { op: Access::Read, key: "secret/x".into() })
            .is_err()); // outside grant → still denied, never AllowAll-broadened
    }

    #[test]
    fn clone_matches_original_decision_on_every_call() {
        // Pin down "a cloned engine must make identical decisions to the
        // original" across allow + deny outcomes.
        let ctx = Box::new(DenyGate { which: "run_profile", category: Category::Db });
        let mut original = engine_with(ctx, caps(&[], &[], &["tasks"], &["tasks"], true));
        let mut clone = original.clone();

        let denied = HostCall::Db { op: Access::Write, collection: "tasks".into() };
        let allowed = HostCall::Storage { op: Access::Read, key: "app/x".into() };

        // run_profile denies the Db category for both engines.
        assert_eq!(
            original.check(&denied).unwrap_err().code(),
            clone.check(&denied).unwrap_err().code()
        );
        // Storage (no manifest grant here) is denied identically by both, and
        // never AllowAll-permitted on the clone.
        assert_eq!(
            original.check(&allowed).unwrap_err().code(),
            clone.check(&allowed).unwrap_err().code()
        );
    }

    // --- Context-gate denial is recordable like a manifest denial (review 023 P1)

    #[test]
    fn check_context_gates_isolates_the_context_decision() {
        // The runtime needs the context-gate outcome in isolation so it can
        // record a context-only denial through the same channel as a
        // manifest-scope denial. `check_context_gates` exposes exactly that:
        // it runs ONLY the DecisionContext gates, no role/budget/capability.
        let ctx = Box::new(DenyGate { which: "platform_permission", category: Category::Ui });
        let e = engine_with(ctx, caps(&[], &[], &[], &[], true));

        // Ui is denied by the platform-permission gate...
        let err = e.check_context_gates(&HostCall::Ui).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("platform permission"), "{err}");

        // ...while a different category (Storage) passes the context gates,
        // even though its manifest/resource subcheck would later deny it. This
        // proves the method reports the CONTEXT decision only.
        assert!(e
            .check_context_gates(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .is_ok());
    }

    #[test]
    fn check_context_gates_allows_under_allow_all() {
        // Under the M0a permissive context every category passes the context
        // gates, so the recordable-denial seam is inert until a real context is
        // wired (no spurious denials recorded on the AllowAll path).
        let e = engine(caps(&["app/*"], &[], &[], &[], true), owner());
        for call in [
            HostCall::Storage { op: Access::Read, key: "app/x".into() },
            HostCall::Db { op: Access::Read, collection: "tasks".into() },
            HostCall::Ui,
            HostCall::Time,
            HostCall::Random,
        ] {
            assert!(e.check_context_gates(&call).is_ok(), "AllowAll passes {call:?}");
        }
    }

    #[test]
    fn check_context_gates_matches_inline_gate_decision_in_check() {
        // `check` runs the same context gates inline; the isolated method must
        // agree with the inline path so the runtime can record the exact denial
        // that `check` would otherwise produce.
        let ctx = Box::new(DenyGate { which: "workspace_policy", category: Category::Storage });
        let mut e = engine_with(ctx, caps(&["app/*"], &[], &[], &[], true));
        let call = HostCall::Storage { op: Access::Read, key: "app/x".into() };

        let isolated = e.check_context_gates(&call).unwrap_err();
        let inline = e.check(&call).unwrap_err();
        assert_eq!(isolated.code(), inline.code());
        assert_eq!(isolated.to_string(), inline.to_string());
    }

    // --- Roles (SC-10 / SC-11) ----------------------------------------------

    #[test]
    fn runnable_roles_may_run() {
        for role in [Role::Owner, Role::Maintainer, Role::Editor, Role::Runner] {
            let actor = ActorContext { actor: "u".into(), role };
            let mut e = engine(caps(&["app/*"], &[], &[], &[], true), actor);
            assert!(
                e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_ok(),
                "{role:?} should be allowed to run"
            );
        }
    }

    #[test]
    fn viewer_role_cannot_run() {
        let actor = ActorContext { actor: "u".into(), role: Role::Viewer };
        let mut e = engine(caps(&["app/*"], &[], &[], &[], true), actor);
        // Even a fully-granted call is denied because the role can't run.
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("role"), "{err}");
    }

    #[test]
    fn auditor_and_reviewer_cannot_run() {
        for role in [Role::Auditor, Role::Reviewer] {
            let actor = ActorContext { actor: "u".into(), role };
            let mut e = engine(caps(&[], &[], &[], &[], true), actor);
            // Even the deterministic Time seam is gated by the role check.
            let err = e.check(&HostCall::Time).unwrap_err();
            assert_eq!(err.code(), "PermissionDenied", "{role:?}");
        }
    }

    #[test]
    fn role_gate_does_not_consume_budget() {
        let actor = ActorContext { actor: "u".into(), role: Role::Viewer };
        let mut e =
            PolicyEngine::new(&manifest_with(caps(&["app/*"], &[], &[], &[], true), 1), &actor)
                .unwrap();
        // A denied call must not advance the host-call counter.
        assert!(e.check(&HostCall::Time).is_err());
        assert_eq!(e.host_calls(), 0);
    }

    // --- Revocation (CR-4) --------------------------------------------------

    #[test]
    fn revoke_storage_takes_effect_immediately() {
        let mut e = engine(caps(&["app/*"], &["app/*"], &[], &[], true), owner());
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_ok());
        e.revoke(Category::Storage);
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(err.to_string().contains("revoked"), "{err}");
    }

    #[test]
    fn revoke_db_and_ui_and_seams() {
        let mut e = engine(caps(&[], &[], &["tasks"], &["tasks"], true), owner());
        assert!(e.check(&HostCall::Db { op: Access::Write, collection: "tasks".into() }).is_ok());
        e.revoke(Category::Db);
        assert_eq!(
            e.check(&HostCall::Db { op: Access::Write, collection: "tasks".into() })
                .unwrap_err()
                .code(),
            "PermissionDenied"
        );

        assert!(e.check(&HostCall::Ui).is_ok());
        e.revoke(Category::Ui);
        assert_eq!(e.check(&HostCall::Ui).unwrap_err().code(), "PermissionDenied");

        assert!(e.check(&HostCall::Time).is_ok());
        e.revoke(Category::Time);
        assert_eq!(e.check(&HostCall::Time).unwrap_err().code(), "PermissionDenied");

        assert!(e.check(&HostCall::Random).is_ok());
        e.revoke(Category::Random);
        assert_eq!(e.check(&HostCall::Random).unwrap_err().code(), "PermissionDenied");
    }

    #[test]
    fn revocation_is_scoped_to_one_category() {
        let mut e = engine(caps(&["app/*"], &[], &["tasks"], &[], true), owner());
        e.revoke(Category::Storage);
        // Storage denied, but db still works.
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_err());
        assert!(e.check(&HostCall::Db { op: Access::Read, collection: "tasks".into() }).is_ok());
    }

    #[test]
    fn revoking_one_seam_leaves_the_other() {
        // Revoking Time must not revoke Random — seam allowances are per-category.
        let mut e = engine(caps(&[], &[], &[], &[], false), owner());
        e.revoke(Category::Time);
        assert!(e.check(&HostCall::Time).is_err());
        assert!(e.check(&HostCall::Random).is_ok());
    }

    #[test]
    fn revoked_call_does_not_consume_budget() {
        let mut e =
            PolicyEngine::new(&manifest_with(caps(&["app/*"], &[], &[], &[], true), 5), &owner())
                .unwrap();
        e.revoke(Category::Storage);
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_err());
        assert_eq!(e.host_calls(), 0, "denied calls must not be counted");
    }

    // --- Host-call budget (SC-2) --------------------------------------------

    #[test]
    fn host_call_count_over_max_is_resource_limit_exceeded() {
        let mut e =
            PolicyEngine::new(&manifest_with(caps(&["app/*"], &[], &[], &[], true), 3), &owner())
                .unwrap();
        for i in 0..3 {
            assert!(
                e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_ok(),
                "call {i} should be allowed"
            );
        }
        assert_eq!(e.host_calls(), 3);
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
        assert!(err.to_string().contains('3'), "message names the limit: {err}");
        // Counter does not advance past the cap on a rejected call.
        assert_eq!(e.host_calls(), 3);
    }

    #[test]
    fn budget_counts_seam_calls_too() {
        // Time/Random are explicit manifest capabilities and still count toward
        // the flood guard.
        let mut e = PolicyEngine::new(&manifest_with(caps(&[], &[], &[], &[], true), 2), &owner())
            .unwrap();
        assert!(e.check(&HostCall::Time).is_ok());
        assert!(e.check(&HostCall::Random).is_ok());
        assert_eq!(e.check(&HostCall::Time).unwrap_err().code(), "ResourceLimitExceeded");
    }

    #[test]
    fn budget_checked_before_capability() {
        // Once the budget is spent, even a normally-denied call surfaces the
        // limit error (the budget gate runs first). This keeps a hostile loop
        // from being able to distinguish denials by error code after flooding.
        let mut e = PolicyEngine::new(&manifest_with(caps(&[], &[], &[], &[], true), 1), &owner())
            .unwrap();
        assert!(e.check(&HostCall::Time).is_ok());
        let err = e
            .check(&HostCall::Storage { op: Access::Read, key: "denied".into() })
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
    }

    // --- Helpers / serde ----------------------------------------------------

    #[test]
    fn category_of_classifies_every_variant() {
        assert_eq!(
            PolicyEngine::category_of(&HostCall::Storage { op: Access::Read, key: "k".into() }),
            Category::Storage
        );
        assert_eq!(
            PolicyEngine::category_of(&HostCall::Db { op: Access::Write, collection: "c".into() }),
            Category::Db
        );
        assert_eq!(PolicyEngine::category_of(&HostCall::Ui), Category::Ui);
        assert_eq!(PolicyEngine::category_of(&HostCall::Time), Category::Time);
        assert_eq!(PolicyEngine::category_of(&HostCall::Random), Category::Random);
    }

    #[test]
    fn host_call_serializes_with_tag() {
        let c = HostCall::Storage { op: Access::Write, key: "app/x".into() };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"call\":\"storage\""), "{s}");
        assert!(s.contains("\"op\":\"write\""), "{s}");
        let back: HostCall = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    // --- Permission snapshot (review 009 P1 CR-9) ---------------------------

    #[test]
    fn snapshot_captures_evaluated_permission_state() {
        let e = engine(caps(&["app/*"], &["app/*"], &["tasks"], &[], true), owner());
        let snap = e.snapshot();
        assert!(snap.can_run);
        assert_eq!(snap.max_host_calls, 10_000);
        assert_eq!(snap.capabilities.storage.read, vec!["app/*".to_string()]);
    }

    #[test]
    fn from_snapshot_reproduces_the_recorded_decision() {
        // Record-time engine grants app/* and permits running.
        let recorded = engine(caps(&["app/*"], &["app/*"], &[], &[], true), owner()).snapshot();
        // Replay engine built from the snapshot honors the SAME grants, even
        // though no manifest/actor is consulted.
        let mut replay = PolicyEngine::from_snapshot(&recorded).unwrap();
        assert!(replay
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .is_ok());
        assert_eq!(
            replay
                .check(&HostCall::Storage { op: Access::Read, key: "secret/x".into() })
                .unwrap_err()
                .code(),
            "PermissionDenied"
        );
    }

    #[test]
    fn from_snapshot_uses_recorded_grants_not_live_ones() {
        // A run recorded under a DENY snapshot (no storage grant) replays as a
        // denial regardless of how generous a current manifest would be.
        let recorded =
            engine(caps(&[], &[], &["tasks"], &["tasks"], true), owner()).snapshot();
        let mut replay = PolicyEngine::from_snapshot(&recorded).unwrap();
        let err = replay
            .check(&HostCall::Storage { op: Access::Read, key: "app/x".into() })
            .unwrap_err();
        // Storage was never declared in the snapshot → CapabilityRequired.
        assert_eq!(err.code(), "CapabilityRequired");
    }

    #[test]
    fn snapshot_roundtrips_through_from_snapshot() {
        let original = engine(caps(&["app/*"], &[], &["tasks"], &["tasks"], false), owner());
        let snap = original.snapshot();
        let rebuilt = PolicyEngine::from_snapshot(&snap).unwrap();
        assert_eq!(rebuilt.snapshot(), snap);
    }

    #[test]
    fn prefix_match_handles_bare_star_and_empty() {
        // `prefix_matches` is the low-level matcher; it still matches greedily
        // for a bare "*" — which is exactly why `validate_storage_grant` rejects
        // such a grant before it can ever be installed (review 006 P2).
        assert!(prefix_matches("*", "anything"));
        assert!(prefix_matches("*", ""));
        // Empty grant matches only the empty key.
        assert!(prefix_matches("", ""));
        assert!(!prefix_matches("", "x"));
    }

    #[test]
    fn validate_storage_grant_accepts_and_rejects() {
        // Direct unit coverage of the validator's branches.
        assert!(validate_storage_grant("config").is_ok());
        assert!(validate_storage_grant("app/*").is_ok());
        assert!(validate_storage_grant("*").is_err());
        assert!(validate_storage_grant("").is_err());
        assert!(validate_storage_grant("  ").is_err());
        assert!(validate_storage_grant("a*b").is_err());
        assert!(validate_storage_grant("**").is_err());
        assert!(validate_storage_grant("*x").is_err());
    }
}
