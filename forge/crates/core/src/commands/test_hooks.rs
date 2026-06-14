//! TEST-ONLY fault-injection seam (`simulate_failure_stage`) for the atomicity /
//! fail-closed regressions.
//!
//! Several command handlers expose a deterministic fault-injection point so the
//! atomicity tests can prove a decision transaction rolls back as a unit (the
//! lifecycle upgrade-`commit`, the purge-`uninstall.tombstone`, the run-egress
//! `run.save`, the schema `registry_persist`) and that a required self-audit append
//! fails closed (the `audit.query` `self_audit_append`). The desired failure is named
//! in the command payload under `simulate_failure_stage`.
//!
//! Review 157 (P2 — production test backdoor): reading that field DIRECTLY from
//! `cmd.payload` in production-compiled code made the hook reachable from UNTRUSTED
//! command input — any caller able to issue `runtime.run` / `applet.upgrade` /
//! `applet.uninstall` / `schema.apply_change` could inject `simulate_failure_stage`
//! and force the decision transaction to roll back (a forced-failure / denial-of-
//! service vector). This module is the single chokepoint that closes that hole: the
//! read is gated behind the non-default `test-hooks` cargo feature, so in the
//! `forge-cli` RELEASE binary [`simulate_failure_stage`] is a const `None` and the
//! dispatch path CANNOT honor the field no matter what a caller puts in the payload.
//!
//! The crate's own integration tests enable the feature via the dev-dependency
//! self-reference in `Cargo.toml`, so they still drive every hook through the
//! test-gated seam — the atomicity coverage is unchanged, only its reachability from
//! production input is removed.
//!
//! Scope of the gate: `test-hooks` is activated ONLY through that dev-dependency, so
//! it is present in the `cargo test`/dev graph but NOT in any crate's NORMAL build.
//! The shipped `forge-cli` binary (a `cargo build` of the `forge` bin) does not pull
//! `forge-core`'s dev-dependencies, so it links `forge-core` with only its `default`
//! feature — `cargo tree -p forge-cli -i forge-core -e features` confirms `test-hooks`
//! is absent there. The unit tests below pin the gate to the feature flag in BOTH
//! compilations, so a regression that reads the payload field unconditionally is
//! caught regardless of how the crate is built.

use forge_domain::CoreCommand;

/// Read the TEST-ONLY `simulate_failure_stage` fault-injection stage from `cmd`'s
/// payload — the name of the failure to inject (e.g. `"commit"`, `"run.save"`,
/// `"uninstall.tombstone"`, `"registry_persist"`, `"self_audit_append"`), or `None`
/// when no failure is requested.
///
/// Gated on the `test-hooks` feature (review 157): with the feature OFF (the default,
/// and the release build) this is a const `None` — the production dispatch path never
/// observes the payload field, so the hook is unreachable from untrusted command
/// input. With the feature ON (the crate's own integration tests) it reads the field
/// so the atomicity / fail-closed regressions can drive the seam.
#[inline]
pub(in crate::workspace) fn simulate_failure_stage(_cmd: &CoreCommand) -> Option<&str> {
    #[cfg(feature = "test-hooks")]
    {
        _cmd.payload
            .get("simulate_failure_stage")
            .and_then(|v| v.as_str())
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        None
    }
}

/// `true` when the payload requests the named fault-injection `stage` (a convenience
/// over [`simulate_failure_stage`] for the common exact-match sites). Always `false`
/// in the release build (feature off), so the gated branch compiles out.
#[inline]
pub(in crate::workspace) fn simulate_failure_at(cmd: &CoreCommand, stage: &str) -> bool {
    simulate_failure_stage(cmd) == Some(stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{ActorContext, CoreCommand, RequestId, WorkspaceId};

    /// A command carrying a `simulate_failure_stage` field in its payload — the exact
    /// shape an untrusted caller would inject to try to force a rollback.
    fn cmd_with_stage(stage: &str) -> CoreCommand {
        CoreCommand {
            request_id: RequestId::new("req"),
            name: "runtime.run".into(),
            applet_id: None,
            actor: ActorContext::owner("attacker"),
            workspace_id: WorkspaceId::new("ws"),
            payload: serde_json::json!({ "simulate_failure_stage": stage }),
        }
    }

    /// Review 157: the `simulate_failure_stage` read is gated by the `test-hooks`
    /// feature, so it is honored ONLY in a test-hooks build and IGNORED (const `None`)
    /// in the release build. This assertion is correct in BOTH compilations — it pins
    /// the gate to the feature flag, so a future change that reads the payload field
    /// unconditionally (re-opening the production backdoor) fails here. Paired with the
    /// `cargo tree` invariant that the shipped `forge-cli` binary activates only
    /// `forge-core`'s `default` feature (NOT `test-hooks`), this proves the dispatch
    /// path cannot honor an injected `simulate_failure_stage` in production.
    #[test]
    fn simulate_failure_stage_is_honored_only_under_the_test_hooks_feature() {
        let cmd = cmd_with_stage("run.save");
        if cfg!(feature = "test-hooks") {
            assert_eq!(
                simulate_failure_stage(&cmd),
                Some("run.save"),
                "with test-hooks ON the gated seam reads the field (drives the regressions)"
            );
            assert!(simulate_failure_at(&cmd, "run.save"));
        } else {
            assert_eq!(
                simulate_failure_stage(&cmd),
                None,
                "the RELEASE build (no test-hooks) never reads simulate_failure_stage — \
                 the forced-rollback backdoor is unreachable from an untrusted payload"
            );
            assert!(!simulate_failure_at(&cmd, "run.save"));
        }
    }

    /// An absent field is `None` regardless of the feature (no hook requested).
    #[test]
    fn an_absent_simulate_failure_stage_is_none() {
        let cmd = CoreCommand {
            request_id: RequestId::new("req"),
            name: "runtime.run".into(),
            applet_id: None,
            actor: ActorContext::owner("user"),
            workspace_id: WorkspaceId::new("ws"),
            payload: serde_json::json!({ "input": {} }),
        };
        assert_eq!(simulate_failure_stage(&cmd), None);
        assert!(!simulate_failure_at(&cmd, "run.save"));
    }
}
