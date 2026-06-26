//! UI host calls for [`HostContext`]: `ctx.ui.render` plus the recorded UI
//! event-dispatch envelope.
//!
//! `ctx.ui.render` is a policy-gated, recorded host call (prd-merged/05 UI). The
//! dispatch envelope records the `(action_ref, payload)` that addressed a handler
//! plus the `result` it produced (prd-merged/05 UI-4, prd-merged/01 CR-6) so a
//! session replays the same event sequence byte-identically.

use super::HostContext;
use forge_domain::Result;
use forge_policy::HostCall;

impl HostContext<'_> {
    // --- UI event dispatch (recorded, replay-bound) ---------------------

    /// Record (or replay) a **dispatched UI event** (prd-merged/05 UI-4,
    /// prd-merged/01 CR-6): the `(action_ref, payload)` that addressed a handler,
    /// plus the `result` the dispatch produced (the handler's final UI tree /
    /// returned value). The individual `ctx.ui.render` calls a handler makes are
    /// already captured as `ui.render` effects; this records the *dispatch
    /// envelope* so a session replays the same event sequence byte-identically.
    ///
    /// On replay the recorder serves the recorded result and asserts the
    /// `action_ref`+`payload` match the recording (a diverging event/payload/order
    /// is a determinism `RuntimeError`). This is *not* a policy-gated host call —
    /// the dispatch itself touches no user data; the effects inside the handler
    /// are gated as usual. It is, however, counted toward the trace order so the
    /// `replay_fingerprint` covers every dispatched event.
    pub fn dispatch_event(
        &mut self,
        action_ref: &str,
        payload: serde_json::Value,
        result: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.recorder.dispatch_event(action_ref, payload, result)
    }

    // --- UI (capability-checked, recorded) ------------------------------

    pub fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        let args = serde_json::json!([tree]);
        self.check_or_record_denial(&HostCall::Ui, "ui.render", &args)?;
        let bridge = &mut *self.bridge;
        let t = tree.clone();
        self.recorder.host_call("ui.render", args, || {
            bridge.ui_render(t).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }
}
