//! Auto-quarantine policy for budget-driven webapp package rollback (D12, Q2).

use serde::{Deserialize, Serialize};

/// Core-owned auto-quarantine knobs (Q2 default: 3 errors / 60s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoQuarantinePolicy {
    pub error_threshold: u32,
    pub window_seconds: u32,
}

impl Default for AutoQuarantinePolicy {
    fn default() -> Self {
        AutoQuarantinePolicy {
            error_threshold: 3,
            window_seconds: 60,
        }
    }
}

/// Input observed by the shell and supplied to `quota.auto_quarantine`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoQuarantineRequest {
    pub app_id: String,
    pub install_id: String,
    #[serde(default)]
    pub error_code: Option<String>,
    pub budget_error_count_60s: u32,
    pub is_active_install: bool,
    #[serde(default)]
    pub policy: AutoQuarantinePolicy,
}

/// Pure decision returned before the shell applies `package.set_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoQuarantineDecision {
    pub should_quarantine: bool,
    pub quarantine_eligible: bool,
    pub budget_error_count_60s: u32,
    pub reason: Option<String>,
}

/// Evaluate whether a budget error should trigger auto-quarantine.
pub fn evaluate_auto_quarantine(input: &AutoQuarantineRequest) -> AutoQuarantineDecision {
    let eligible = input.error_code.as_deref() == Some("resource_budget_exceeded")
        && input.is_active_install;
    let should = eligible
        && input.budget_error_count_60s >= input.policy.error_threshold;
    AutoQuarantineDecision {
        should_quarantine: should,
        quarantine_eligible: eligible,
        budget_error_count_60s: input.budget_error_count_60s,
        reason: if should {
            Some("resource_budget_exceeded".into())
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_budget_error_triggers_quarantine() {
        let decision = evaluate_auto_quarantine(&AutoQuarantineRequest {
            app_id: "notes-lite".into(),
            install_id: "install-v2".into(),
            error_code: Some("resource_budget_exceeded".into()),
            budget_error_count_60s: 3,
            is_active_install: true,
            policy: AutoQuarantinePolicy::default(),
        });
        assert!(decision.should_quarantine);
        assert!(decision.quarantine_eligible);
    }

    #[test]
    fn inactive_install_is_not_eligible() {
        let decision = evaluate_auto_quarantine(&AutoQuarantineRequest {
            app_id: "notes-lite".into(),
            install_id: "install-v2".into(),
            error_code: Some("resource_budget_exceeded".into()),
            budget_error_count_60s: 3,
            is_active_install: false,
            policy: AutoQuarantinePolicy::default(),
        });
        assert!(!decision.should_quarantine);
        assert!(!decision.quarantine_eligible);
    }
}