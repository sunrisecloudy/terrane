//! The KV-schema persistence layer for [`WorkspaceCore`](super::WorkspaceCore).
//!
//! These are the small read/write helpers that back the workspace's `__forge/
//! meta` namespace: the monotone run counter, the per-applet last-known UI tree
//! (the interactive diff base, UI-4/CR-6), and the per-applet dispatch lifecycle
//! flag. They are extracted here verbatim from `workspace.rs` (a pure move,
//! /simplify #5) so the facade reads as orchestration while the KV key schema +
//! (de)serialization lives in one focused module.
//!
//! ATOMICITY ORDERING is load-bearing and preserved exactly:
//!   - [`next_run_counter`] delegates to [`Store::next_counter`] (read+bump+write
//!     in ONE SQLite transaction) — never a read-bump-write in core.
//!   - the lifecycle read is not cached/deferred (each [`get_applet_lifecycle`]
//!     hits the store).

use forge_domain::{CoreError, Result};
use forge_storage::Store;

use super::AppletLifecycle;

/// Reserved KV namespace prefix for core-owned metadata (applet manifests +
/// compiled programs + workspace meta). Applet `ctx.storage` namespaces are
/// `applet/<id>` (see [`StorageHostBridge`](crate::StorageHostBridge)), which
/// never collide with this `__forge/...` prefix.
pub(super) const META_NS: &str = "__forge/meta";

/// The KV key (within [`META_NS`]) holding the workspace's monotone run counter.
/// Bumped once per `runtime.run` to mint a unique per-execution `run_id` while
/// the replay *seeds* stay a deterministic function of `(code_hash, input)`
/// (review 031 finding 2 / CR-9 "every execution persists").
pub(super) const RUN_COUNTER_KEY: &str = "run_counter";

/// KV key prefix (within [`META_NS`]) for an applet's **last-known UI tree** — the
/// most recent tree the applet rendered through this facade (`runtime.run`'s last
/// `ui.render`, then each accepted `ui.dispatch_event`). This is the DIFF BASE for
/// the next event: `ui.dispatch_event` re-enters the applet's handler in a fresh
/// one-shot realm, captures the handler's new tree, and diffs it against THIS
/// stored tree to produce the next UI patch (UI-4/CR-6). Keyed per applet so two
/// applets' interactive sessions never share a diff base; persisted so the loop
/// survives reopening the workspace. The full key is `ui_tree/<applet_id>`.
pub(super) const UI_TREE_KEY_PREFIX: &str = "ui_tree/";

/// KV key prefix (within [`META_NS`]) for an applet's **dispatch lifecycle** — the
/// receiver-side flag that decides whether an applet may be re-entered by a UI
/// event. An applet is `active` by default; a workspace can SUSPEND it through the
/// trusted [`set_applet_lifecycle`](super::WorkspaceCore::set_applet_lifecycle)
/// seam, after which `ui.dispatch_event` rejects every event with a typed
/// `ui.applet_not_dispatchable` error BEFORE any handler runs and with no state
/// change (the T034 `suspended_applet_rejected` vector). Set only through the
/// trusted seam (never a request payload), mirroring the `db.read` grant table;
/// persisted so a suspended applet stays suspended after reopen. The full key is
/// `lifecycle/<applet_id>`.
pub(super) const APPLET_LIFECYCLE_KEY_PREFIX: &str = "lifecycle/";

/// KV key for an applet's last-known UI tree (the interactive diff base) within
/// [`META_NS`]. See [`UI_TREE_KEY_PREFIX`].
fn ui_tree_key(applet_id: &str) -> String {
    format!("{UI_TREE_KEY_PREFIX}{applet_id}")
}

/// KV key for an applet's dispatch lifecycle flag within [`META_NS`]. See
/// [`APPLET_LIFECYCLE_KEY_PREFIX`].
fn applet_lifecycle_key(applet_id: &str) -> String {
    format!("{APPLET_LIFECYCLE_KEY_PREFIX}{applet_id}")
}

/// Persist `tree` as the applet's last-known UI tree (the diff base for the
/// next UI event), keyed by applet id within [`META_NS`]. Written after every
/// accepted render through this facade — a `runtime.run`'s last render and each
/// accepted `ui.dispatch_event` — so the interactive loop's diff base survives
/// reopening the workspace (UI-4/CR-6).
pub(super) fn store_ui_tree(
    store: &mut Store,
    applet_id: &str,
    tree: &serde_json::Value,
) -> Result<()> {
    let bytes = serde_json::to_vec(tree)
        .map_err(|e| CoreError::StorageError(format!("ui tree serialize failed: {e}")))?;
    store.kv_set(META_NS, &ui_tree_key(applet_id), &bytes, "application/json")
}

/// Load the applet's last-known UI tree (the diff base) as a [`forge_ui::Node`],
/// if one was recorded. `None` ⇒ the applet has not rendered through this facade
/// yet, so the next render's diff is a single root replace (UI-1).
pub(super) fn load_ui_tree(store: &Store, applet_id: &str) -> Result<Option<forge_ui::Node>> {
    match store.kv_get(META_NS, &ui_tree_key(applet_id))? {
        Some(bytes) => {
            let node = forge_ui::from_str(std::str::from_utf8(&bytes).map_err(|e| {
                CoreError::StorageError(format!("ui tree is not utf-8: {e}"))
            })?)?;
            Ok(Some(node))
        }
        None => Ok(None),
    }
}

/// Set an applet's TRUSTED dispatch lifecycle (UI-4/CR-6): `Active` (the
/// default, re-entrant) or `Suspended` (a UI event is rejected before any
/// handler runs). The flag is **persisted** to the workspace file, so a
/// suspended applet stays suspended after `open(...)`.
pub(super) fn set_applet_lifecycle(
    store: &mut Store,
    applet_id: impl AsRef<str>,
    lifecycle: AppletLifecycle,
) -> Result<()> {
    let key = applet_lifecycle_key(applet_id.as_ref());
    let bytes = serde_json::to_vec(&lifecycle)
        .map_err(|e| CoreError::StorageError(format!("serialize applet lifecycle: {e}")))?;
    store.kv_set(META_NS, &key, &bytes, "application/json")
}

/// An applet's dispatch lifecycle, defaulting to [`AppletLifecycle::Active`] for
/// an applet that was never explicitly suspended. Read-only access for tests /
/// the `ui.dispatch_event` gate.
pub(super) fn get_applet_lifecycle(store: &Store, applet_id: &str) -> Result<AppletLifecycle> {
    match store.kv_get(META_NS, &applet_lifecycle_key(applet_id))? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::StorageError(format!("deserialize applet lifecycle: {e}"))
        }),
        None => Ok(AppletLifecycle::Active),
    }
}

/// Atomically read-bump-write the persisted workspace run counter, returning
/// the value assigned to this invocation. Monotone across the workspace's
/// lifetime (persisted in meta), so each `runtime.run` mints a distinct
/// `run_id` even for an identical applet+input pair (review 031 finding 2).
///
/// Review 036 finding 3: the read+bump+write run inside ONE SQLite transaction
/// ([`Store::next_counter`]), so the reservation is atomic. Two `WorkspaceCore`
/// instances over the same file can no longer reserve the same invocation
/// number — the second transaction observes the first's committed value — so no
/// audit record is silently replaced via a `run_id` collision.
pub(super) fn next_run_counter(store: &mut Store) -> Result<u64> {
    store.next_counter(META_NS, RUN_COUNTER_KEY)
}
