//! forge-core: the command/event facade that wires the entire M0a spine.
//!
//! prd-merged/01 CR-A1..A5 (Command / Event / Response; an actor passes policy
//! before state) + prd-merged/04 P-04 command catalog
//! (`forge/spec/commands.md`).
//!
//! This crate is the seam where every lower layer meets:
//!
//! ```text
//!   TS  ──forge-pipeline──▶  SWC transpile + static policy scan  (applet.install)
//!       ──forge-runtime───▶  QuickJS realm, zero ambient capability
//!       ──forge-policy────▶  per-call capability check on ctx.*
//!       ──forge-storage───▶  SQLite record/KV write          (ctx.db / ctx.storage)
//!       ──forge-ui────────▶  tree diff → UI patch events      (ctx.ui.render)
//!       ──forge-runtime───▶  deterministic RunRecord + replay (runtime.run/replay)
//! ```
//!
//! [`WorkspaceCore`] holds the [`forge_storage::Store`], a
//! [`forge_schema::SchemaRegistry`], and an [`EventSink`]; its
//! [`handle`](WorkspaceCore::handle) method dispatches a [`forge_domain::CoreCommand`]
//! to a [`forge_domain::CoreResponse`] and emits [`forge_domain::CoreEvent`]s for
//! observability (prd-merged/02). [`StorageHostBridge`] is the
//! [`forge_runtime::HostBridge`] that backs `ctx.*` with the real `Store`.

mod bridge;
mod event;
mod sync_rbac;
mod workspace;

pub use bridge::{NoNetworkClient, StorageHostBridge, UiRender};
pub use event::EventSink;
pub use sync_rbac::{
    authorize_remote_op, IncomingClaim, RemoteOp, RemoteOpEnvelope, ResourceType, SyncAuditRecord,
    SyncAuthDecision, TrustedMembership,
};
pub use workspace::{source_id_for, AppletLifecycle, WorkspaceCore};
