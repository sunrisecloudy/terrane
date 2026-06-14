//! The `ctx.log` sink for [`HostContext`].
//!
//! Logging has no capability gate (it is an observability sink, not an effect on
//! user data). It is recorded so replay stays in parity, and bounded by **two**
//! budgets (review 009 P2): the `log_bytes` byte budget (CR-5) caps total log
//! *volume*, and the `max_host_calls` budget caps the *number* of log calls so an
//! empty-string log flood — which costs zero bytes — still trips a limit.

use super::HostContext;
use forge_domain::Result;

impl HostContext<'_> {
    /// `ctx.log(line)` — there is no capability gate for logging (it is an
    /// observability sink, not an effect on user data). It is recorded so replay
    /// stays in parity, and bounded by **two** budgets (review 009 P2):
    ///   * the `log_bytes` budget (CR-5) caps total log *volume*; and
    ///   * the `max_host_calls` budget caps the *number* of log calls — an
    ///     empty-string log flood costs zero bytes, so the byte budget alone can
    ///     never stop it, and ctx.log is otherwise outside the policy host-call
    ///     counter. Counting log calls here closes that flood hole.
    pub fn log(&mut self, line: &str) -> Result<()> {
        // Call-count budget first: a flood of empty logs must trip even though it
        // adds no bytes (review 009 P2). Then the byte budget. Both route through
        // the single `HostBudgets` source of truth.
        self.budgets.check_log_call()?;
        self.budgets.check_log_bytes(line.len() as u64)?;
        let args = serde_json::json!([line]);
        let bridge = &mut *self.bridge;
        let l = line.to_string();
        self.recorder.host_call("log", args, || {
            bridge.log(&l).map(|()| serde_json::Value::Null)
        })?;
        self.logs.push(line.to_string());
        Ok(())
    }
}
