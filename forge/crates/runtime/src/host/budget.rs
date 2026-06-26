//! The per-run resource budgets for [`HostContext`](super::HostContext).
//!
//! This is the single source of truth for the CR-5 byte budgets and the SC-2
//! host-call flood gate. Each capability handler routes its accounting through
//! one of the `check_*` methods here instead of inlining the
//! `saturating_add` + limit comparison, so the budget arithmetic — and the exact
//! `ResourceLimitExceeded` message it raises — lives in exactly one place.
//!
//! Accounting note (preserved as-is, NOT a single shared counter): `ctx.log`,
//! `ctx.net.fetch`, and `ctx.files.*` are each gated by their own grant rather
//! than the [`PolicyEngine`](forge_policy::PolicyEngine) `HostCall` category
//! counter, so each keeps its **own** call counter against the *same*
//! `max_host_calls` limit. They are independent tallies, not one shared flood
//! gate — the policy-gated calls (`storage`/`db`/`time`/`random`/`ui`) are
//! counted separately inside the `PolicyEngine`. Unifying these into a single
//! shared counter would CHANGE accounting and is intentionally avoided here (see
//! the dedup report's flagged review item).

use forge_domain::{CoreError, Limits, Result};

/// The mutable per-run budget tallies plus the immutable [`Limits`] they are
/// checked against. Owned by [`HostContext`](super::HostContext); every effect
/// that consumes a budget does so through a `check_*` method below.
pub(super) struct HostBudgets {
    /// The run's resource limits (`max_host_calls`, `storage_bytes`, `log_bytes`).
    limits: Limits,
    /// Bytes appended to the log so far (against `Limits::log_bytes`).
    log_bytes_used: u64,
    /// Bytes written to storage so far (against `Limits::storage_bytes`).
    storage_bytes_used: u64,
    /// `ctx.log` calls so far (against `Limits::max_host_calls`, review 009 P2):
    /// a flood of empty-string logs costs zero bytes, so the byte budget alone
    /// can't stop it — count the *calls* against the host-call cap too.
    log_calls_used: u64,
    /// `ctx.net.fetch` calls so far (against `Limits::max_host_calls`). `net` is
    /// gated by the [`NetPolicy`](forge_policy::NetPolicy) decision rather than the
    /// [`PolicyEngine`](forge_policy::PolicyEngine) `HostCall` categories, so —
    /// like `ctx.log` — it counts its own calls against the host-call flood cap
    /// (SC-2) here.
    net_calls_used: u64,
    /// `ctx.files.read`/`ctx.files.write` calls so far (against
    /// `Limits::max_host_calls`). Like `net`, files is gated by its own grant
    /// (not the [`PolicyEngine`](forge_policy::PolicyEngine) `HostCall`
    /// categories), so it counts its own calls against the host-call flood cap
    /// (SC-2) here.
    files_calls_used: u64,
    /// `ctx.resource.*` calls so far (against `Limits::max_host_calls`).
    resource_calls_used: u64,
}

impl HostBudgets {
    /// All tallies start at zero for the run's [`Limits`].
    pub(super) fn new(limits: Limits) -> Self {
        Self {
            limits,
            log_bytes_used: 0,
            storage_bytes_used: 0,
            log_calls_used: 0,
            net_calls_used: 0,
            files_calls_used: 0,
            resource_calls_used: 0,
        }
    }

    /// Charge one `ctx.log` call against the host-call flood cap (SC-2 / review
    /// 009 P2). A flood of empty logs must trip even though it adds no bytes, so
    /// the *call* is counted before the byte budget. Errors with the verbatim
    /// `(ctx.log flood)` message.
    pub(super) fn check_log_call(&mut self) -> Result<()> {
        self.log_calls_used = self.log_calls_used.saturating_add(1);
        if self.log_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.log flood)",
                self.limits.max_host_calls
            )));
        }
        Ok(())
    }

    /// Charge `len` log bytes against the `log_bytes` byte budget (CR-5).
    pub(super) fn check_log_bytes(&mut self, len: u64) -> Result<()> {
        self.log_bytes_used = self.log_bytes_used.saturating_add(len);
        if self.log_bytes_used > self.limits.log_bytes {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "log byte budget exceeded: log_bytes = {} reached",
                self.limits.log_bytes
            )));
        }
        Ok(())
    }

    /// Charge `value_bytes` against the `storage_bytes` byte budget (CR-5).
    pub(super) fn check_storage_bytes(&mut self, value_bytes: u64) -> Result<()> {
        self.storage_bytes_used = self.storage_bytes_used.saturating_add(value_bytes);
        if self.storage_bytes_used > self.limits.storage_bytes {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "storage byte budget exceeded: storage_bytes = {} reached",
                self.limits.storage_bytes
            )));
        }
        Ok(())
    }

    /// Charge one `ctx.net.fetch` call against the host-call flood cap (SC-2).
    /// Errors with the verbatim `(ctx.net.fetch flood)` message.
    pub(super) fn check_net_call(&mut self) -> Result<()> {
        self.net_calls_used = self.net_calls_used.saturating_add(1);
        if self.net_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.net.fetch flood)",
                self.limits.max_host_calls
            )));
        }
        Ok(())
    }

    /// Charge one `ctx.files.read`/`ctx.files.write` call against the host-call
    /// flood cap (SC-2). Errors with the verbatim `(ctx.files flood)` message.
    pub(super) fn check_files_call(&mut self) -> Result<()> {
        self.files_calls_used = self.files_calls_used.saturating_add(1);
        if self.files_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.files flood)",
                self.limits.max_host_calls
            )));
        }
        Ok(())
    }

    /// Charge one `ctx.resource.*` call against the host-call flood cap (SC-2).
    pub(super) fn check_resource_call(&mut self) -> Result<()> {
        self.resource_calls_used = self.resource_calls_used.saturating_add(1);
        if self.resource_calls_used > self.limits.max_host_calls {
            return Err(CoreError::ResourceLimitExceeded(format!(
                "host-call limit exceeded: max_host_calls = {} reached (ctx.resource flood)",
                self.limits.max_host_calls
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small `Limits` where the byte/call caps are the only relevant fields.
    fn limits(max_host_calls: u64, storage_bytes: u64, log_bytes: u64) -> Limits {
        Limits { max_host_calls, storage_bytes, log_bytes, ..Limits::default() }
    }

    /// The log-call counter trips on the `(max_host_calls + 1)`th call with the
    /// exact `(ctx.log flood)` message — the byte budget is irrelevant here.
    #[test]
    fn log_call_flood_trips_at_max_host_calls() {
        let mut b = HostBudgets::new(limits(2, u64::MAX, u64::MAX));
        assert!(b.check_log_call().is_ok());
        assert!(b.check_log_call().is_ok());
        let err = b.check_log_call().unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
        assert!(err.to_string().contains("ctx.log flood"), "{err}");
    }

    /// The log-byte budget accumulates and trips when total bytes EXCEED
    /// `log_bytes` (strictly greater), independent of the call count.
    #[test]
    fn log_bytes_budget_trips_when_exceeded() {
        let mut b = HostBudgets::new(limits(u64::MAX, u64::MAX, 4));
        assert!(b.check_log_bytes(4).is_ok(), "exactly at the cap is allowed");
        let err = b.check_log_bytes(1).unwrap_err();
        assert!(err.to_string().contains("log byte budget"), "{err}");
    }

    /// The storage-byte budget accumulates across calls and trips with the
    /// `storage byte` message when the running total exceeds `storage_bytes`.
    #[test]
    fn storage_bytes_budget_accumulates_and_trips() {
        let mut b = HostBudgets::new(limits(u64::MAX, 10, u64::MAX));
        assert!(b.check_storage_bytes(6).is_ok());
        assert!(b.check_storage_bytes(4).is_ok(), "exactly at the cap is allowed");
        let err = b.check_storage_bytes(1).unwrap_err();
        assert!(err.to_string().contains("storage byte budget"), "{err}");
    }

    /// `net` and `files` keep INDEPENDENT call counters against the same
    /// `max_host_calls` cap — exhausting one does not consume the other's slots.
    /// This pins the preserved (non-shared) accounting.
    #[test]
    fn net_and_files_counters_are_independent() {
        let mut b = HostBudgets::new(limits(1, u64::MAX, u64::MAX));
        // One net + one files call both fit even though max_host_calls == 1,
        // because each namespace counts against its own tally.
        assert!(b.check_net_call().is_ok());
        assert!(b.check_files_call().is_ok());
        // The SECOND call in each namespace is what trips its own counter.
        assert!(b.check_net_call().unwrap_err().to_string().contains("ctx.net.fetch flood"));
        assert!(b.check_files_call().unwrap_err().to_string().contains("ctx.files flood"));
    }

    /// Saturating arithmetic: an enormous byte charge saturates `u64` instead of
    /// wrapping, so it still trips the budget rather than silently overflowing.
    #[test]
    fn byte_charge_saturates_rather_than_wrapping() {
        let mut b = HostBudgets::new(limits(u64::MAX, 100, u64::MAX));
        let err = b.check_storage_bytes(u64::MAX).unwrap_err();
        assert!(err.to_string().contains("storage byte budget"), "{err}");
        // A follow-up charge stays saturated (no wrap back under the cap).
        assert!(b.check_storage_bytes(1).is_err());
    }
}
