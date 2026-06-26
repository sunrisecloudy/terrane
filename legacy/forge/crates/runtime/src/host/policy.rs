//! The single policy/denial-recording chokepoint for [`HostContext`].
//!
//! Every policy-gated `ctx.*` host call funnels through
//! [`HostContext::check_or_record_denial`]: it runs the forge-policy
//! [`PolicyEngine`](forge_policy::PolicyEngine) check and, on a denial, records
//! the denied attempt into the trace (so it survives into the `RunRecord`,
//! review 009 P1 CR-9) in **one** deterministic order before propagating the
//! error. Keeping this in a single place is what makes the denial-recording
//! sequence replay-stable — an out-of-order denial breaks replay.

use super::HostContext;
use forge_domain::Result;
use forge_policy::HostCall;

impl HostContext<'_> {
    /// Run the policy check for `call`; on a denial, record the denied attempt
    /// into the trace (so it survives into the [`RunRecord`], review 009 P1 CR-9)
    /// and then propagate the error. `method`/`args` describe the call as the
    /// recorder logs it.
    ///
    /// Recording the denial can itself fail in replay mode (a method/args
    /// mismatch against the recorded denial) — that divergence takes precedence
    /// and is surfaced instead of the original policy error.
    pub(super) fn check_or_record_denial(
        &mut self,
        call: &HostCall,
        method: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        match self.policy.check(call) {
            Ok(()) => Ok(()),
            Err(policy_err) => {
                self.recorder
                    .record_denial(method, args.clone(), &policy_err)?;
                Err(policy_err)
            }
        }
    }
}
