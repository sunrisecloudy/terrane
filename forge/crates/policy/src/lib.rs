//! forge-policy: the capability + minimal-RBAC engine that gates every
//! `ctx.*` host call an applet attempts at runtime.
//!
//! Normative spec: prd-merged/07-security-prd.md.
//!   - **SC-8** capability = action + resource + constraints. A [`HostCall`]
//!     models the concrete call the runtime is about to make; the manifest
//!     [`Capabilities`] grants are the action+resource scopes.
//!   - **SC-9** grants are per-workspace-member: the [`ActorContext`] role is
//!     evaluated together with the manifest grants on every call.
//!   - **SC-10** a run is allowed only if *all* pass: the actor role permits
//!     running ∧ the manifest requests the capability ∧ the resource matches
//!     the allowlist ∧ a resource (host-call count) budget remains.
//!   - **SC-2** the host-call counter is a flood guard mapped to
//!     `manifest.limits.max_host_calls` → [`CoreError::ResourceLimitExceeded`].
//!   - **CR-4** revocation takes effect immediately: [`PolicyEngine::revoke`]
//!     denies a category on the very next [`PolicyEngine::check`].
//!
//! This crate is pure logic with no I/O; it stays `wasm32-unknown-unknown`
//! clean (no `std::time`/`std::fs`). The runtime crate owns real CPU/memory
//! limits; policy owns role/capability decisions and the call-count limit.

use forge_domain::{
    ActorContext, Capabilities, CoreError, Manifest, PermissionSnapshot, Result, Role,
};
use serde::{Deserialize, Serialize};

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
    /// `ctx.time.now` — deterministic clock seam (always allowed).
    Time,
    /// `ctx.random.next` — deterministic RNG seam (always allowed).
    Random,
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
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Category::Storage => "storage",
            Category::Db => "db",
            Category::Ui => "ui",
            Category::Time => "time",
            Category::Random => "random",
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

/// The capability + RBAC decision engine for a single run.
///
/// Built from a `&Manifest` (the requested grants) plus an [`ActorContext`]
/// (who is running, with what role). [`PolicyEngine::check`] is called once per
/// host call the runtime makes, in order, and enforces all of SC-10 plus the
/// SC-2 host-call flood guard.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    /// Requested grants, cloned so revocation can mutate without touching the
    /// caller's manifest (revocation is per-engine/per-member, prd-merged/07
    /// SC-9/CR-4).
    capabilities: Capabilities,
    /// Whether the actor's role may run code at all (SC-10).
    can_run: bool,
    /// Categories revoked at runtime; denied immediately regardless of grant.
    revoked: [bool; 5],
    /// Max host calls permitted this run (`manifest.limits.max_host_calls`).
    max_host_calls: u64,
    /// Host calls counted so far this run.
    host_calls: u64,
}

impl PolicyEngine {
    /// Build an engine for `actor` running under `manifest`.
    pub fn new(manifest: &Manifest, actor: &ActorContext) -> Self {
        PolicyEngine {
            capabilities: manifest.capabilities.clone(),
            can_run: role_can_run(actor.role),
            revoked: [false; 5],
            max_host_calls: manifest.limits.max_host_calls,
            host_calls: 0,
        }
    }

    /// Build an engine from a recorded [`PermissionSnapshot`] (review 009 P1
    /// CR-9). Replay must re-derive the permission decision the run was recorded
    /// under — *not* whatever the live manifest grants now — so a denied call
    /// stays denied (and an allowed call stays allowed) even if grants/role/budget
    /// have since changed. The runtime builds its replay-mode engine from the
    /// record's snapshot, making the recorded decision authoritative.
    pub fn from_snapshot(snapshot: &PermissionSnapshot) -> Self {
        PolicyEngine {
            capabilities: snapshot.capabilities.clone(),
            can_run: snapshot.can_run,
            revoked: [false; 5],
            max_host_calls: snapshot.max_host_calls,
            host_calls: 0,
        }
    }

    /// Capture the engine's evaluated permission state as a [`PermissionSnapshot`]
    /// for the run record (review 009 P1 CR-9). Reflects the *current* grants,
    /// role-can-run, and host-call budget (revocations applied so far are not part
    /// of the static snapshot; M0a has no mid-run revocation on the spine path).
    pub fn snapshot(&self) -> PermissionSnapshot {
        PermissionSnapshot {
            capabilities: self.capabilities.clone(),
            can_run: self.can_run,
            max_host_calls: self.max_host_calls,
        }
    }

    /// Index of a category into the `revoked` bitset.
    fn revoke_index(cat: Category) -> usize {
        match cat {
            Category::Storage => 0,
            Category::Db => 1,
            Category::Ui => 2,
            Category::Time => 3,
            Category::Random => 4,
        }
    }

    fn is_revoked(&self, cat: Category) -> bool {
        self.revoked[Self::revoke_index(cat)]
    }

    /// Revoke a capability category for the rest of this run.
    ///
    /// prd-merged/07 CR-4: revocation takes effect immediately — the very next
    /// [`check`](Self::check) of a call in `cat` is denied with
    /// `PermissionDenied`, even though the manifest still nominally grants it.
    pub fn revoke(&mut self, cat: Category) {
        self.revoked[Self::revoke_index(cat)] = true;
    }

    /// Number of host calls counted so far this run.
    pub fn host_calls(&self) -> u64 {
        self.host_calls
    }

    /// The capability category a `call` belongs to.
    pub fn category_of(call: &HostCall) -> Category {
        match call {
            HostCall::Storage { .. } => Category::Storage,
            HostCall::Db { .. } => Category::Db,
            HostCall::Ui => Category::Ui,
            HostCall::Time => Category::Time,
            HostCall::Random => Category::Random,
        }
    }

    /// Decide whether `call` is permitted, and on success count it against the
    /// host-call budget.
    ///
    /// Order of checks (prd-merged/07 SC-10 — *all* must pass):
    /// 1. the actor's role permits running at all;
    /// 2. the budget (`max_host_calls`) has not been exhausted (SC-2);
    /// 3. the category has not been revoked (CR-4);
    /// 4. the manifest grants the capability category, and the specific
    ///    resource matches the allowlist (SC-8).
    ///
    /// On any failure no call is counted; the budget is only consumed by calls
    /// that are actually allowed to proceed.
    pub fn check(&mut self, call: &HostCall) -> Result<()> {
        // 1. Role gate (SC-10): read-only roles cannot run code, so no host
        //    call they would issue is ever permitted.
        if !self.can_run {
            return Err(CoreError::PermissionDenied(format!(
                "actor role is not permitted to run applets (required: Owner/Maintainer/Editor/Runner) for {} call",
                Self::category_of(call).as_str()
            )));
        }

        // 2. Budget gate (SC-2): the (n+1)th call once n == max is refused.
        if self.host_calls >= self.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached",
                self.max_host_calls
            )));
        }

        // 3 + 4. Capability gate (SC-8/CR-4).
        self.check_capability(call)?;

        // Only count calls that pass every gate.
        self.host_calls += 1;
        Ok(())
    }

    /// The capability/allowlist portion of [`check`](Self::check), separated so
    /// the role and budget gates read cleanly. Does not touch the counter.
    fn check_capability(&self, call: &HostCall) -> Result<()> {
        match call {
            HostCall::Storage { op, key } => {
                if self.is_revoked(Category::Storage) {
                    return Err(revoked_error(Category::Storage, *op, key));
                }
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
                if self.is_revoked(Category::Db) {
                    return Err(revoked_error(Category::Db, *op, collection));
                }
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
                if self.is_revoked(Category::Ui) {
                    return Err(CoreError::PermissionDenied(
                        "ui capability has been revoked".to_string(),
                    ));
                }
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
            // Deterministic seams are always available (prd-merged/01 CR-11),
            // but still honor an explicit runtime revocation (CR-4).
            HostCall::Time => {
                if self.is_revoked(Category::Time) {
                    return Err(CoreError::PermissionDenied(
                        "time capability has been revoked".to_string(),
                    ));
                }
                Ok(())
            }
            HostCall::Random => {
                if self.is_revoked(Category::Random) {
                    return Err(CoreError::PermissionDenied(
                        "random capability has been revoked".to_string(),
                    ));
                }
                Ok(())
            }
        }
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

/// Prefix/glob match for storage keys. A grant of `app/*` matches any key
/// under `app/`; a bare grant (`config`) matches exactly that key. The `*`
/// suffix is the only glob form M0a supports (prd-merged/07 SC-8 `path/*`).
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
        }
    }

    fn owner() -> ActorContext {
        ActorContext::owner("dev")
    }

    fn engine(caps: Capabilities, actor: ActorContext) -> PolicyEngine {
        PolicyEngine::new(&manifest_with(caps, 10_000), &actor)
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

    // --- Deterministic seams ------------------------------------------------

    #[test]
    fn time_and_random_are_always_allowed() {
        // Even with an utterly empty capability set, the deterministic seams
        // are available (prd-merged/01 CR-11).
        let mut e = engine(caps(&[], &[], &[], &[], false), owner());
        assert!(e.check(&HostCall::Time).is_ok());
        assert!(e.check(&HostCall::Random).is_ok());
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
            // Even the always-allowed Time seam is gated by the role check.
            let err = e.check(&HostCall::Time).unwrap_err();
            assert_eq!(err.code(), "PermissionDenied", "{role:?}");
        }
    }

    #[test]
    fn role_gate_does_not_consume_budget() {
        let actor = ActorContext { actor: "u".into(), role: Role::Viewer };
        let mut e =
            PolicyEngine::new(&manifest_with(caps(&["app/*"], &[], &[], &[], true), 1), &actor);
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
    fn revoked_call_does_not_consume_budget() {
        let mut e =
            PolicyEngine::new(&manifest_with(caps(&["app/*"], &[], &[], &[], true), 5), &owner());
        e.revoke(Category::Storage);
        assert!(e.check(&HostCall::Storage { op: Access::Read, key: "app/x".into() }).is_err());
        assert_eq!(e.host_calls(), 0, "denied calls must not be counted");
    }

    // --- Host-call budget (SC-2) --------------------------------------------

    #[test]
    fn host_call_count_over_max_is_resource_limit_exceeded() {
        let mut e =
            PolicyEngine::new(&manifest_with(caps(&["app/*"], &[], &[], &[], true), 3), &owner());
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
        // Time/Random are "always allowed" capability-wise but still count
        // toward the flood guard.
        let mut e = PolicyEngine::new(&manifest_with(caps(&[], &[], &[], &[], true), 2), &owner());
        assert!(e.check(&HostCall::Time).is_ok());
        assert!(e.check(&HostCall::Random).is_ok());
        assert_eq!(e.check(&HostCall::Time).unwrap_err().code(), "ResourceLimitExceeded");
    }

    #[test]
    fn budget_checked_before_capability() {
        // Once the budget is spent, even a normally-denied call surfaces the
        // limit error (the budget gate runs first). This keeps a hostile loop
        // from being able to distinguish denials by error code after flooding.
        let mut e = PolicyEngine::new(&manifest_with(caps(&[], &[], &[], &[], true), 1), &owner());
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
        let mut replay = PolicyEngine::from_snapshot(&recorded);
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
        let mut replay = PolicyEngine::from_snapshot(&recorded);
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
        let rebuilt = PolicyEngine::from_snapshot(&snap);
        assert_eq!(rebuilt.snapshot(), snap);
    }

    #[test]
    fn prefix_match_handles_bare_star_and_empty() {
        // A lone "*" grant matches everything (prefix is empty).
        assert!(prefix_matches("*", "anything"));
        assert!(prefix_matches("*", ""));
        // Empty grant matches only the empty key.
        assert!(prefix_matches("", ""));
        assert!(!prefix_matches("", "x"));
    }
}
