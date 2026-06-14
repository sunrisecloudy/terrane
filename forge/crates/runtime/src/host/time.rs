//! Deterministic time/random seams for [`HostContext`].
//!
//! `ctx.time.now()` and `ctx.random.next()` are policy-checked (against the
//! always-granted Time/Random categories), counted against `max_host_calls`, and
//! served by the recorder's seeded logical clock / RNG (prd-merged/01 CR-11). They
//! funnel through the same [`HostContext::check_or_record_denial`] chokepoint as
//! every other host call.

use super::HostContext;
use forge_domain::Result;
use forge_policy::HostCall;

impl HostContext<'_> {
    // --- Deterministic seams (policy-checked, recorded) ------------------

    /// `ctx.time.now()` — checked against the (always-granted) Time category,
    /// counted against `max_host_calls`, served by the seeded logical clock.
    pub fn now(&mut self) -> Result<i64> {
        self.check_or_record_denial(&HostCall::Time, "time.now", &serde_json::Value::Null)?;
        self.recorder.now()
    }

    /// `ctx.random.next()` — checked against Random, counted, served by the
    /// seeded RNG.
    pub fn random_next(&mut self) -> Result<f64> {
        self.check_or_record_denial(&HostCall::Random, "random.next", &serde_json::Value::Null)?;
        self.recorder.random_next()
    }
}
