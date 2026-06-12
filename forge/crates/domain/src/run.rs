//! Deterministic run record + replay contract.
//!
//! prd-merged/01 CR-8 (deterministic mode), CR-9 (run records), CR-11
//! (injectable clock/RNG seams); prd-merged/02 System-Architecture determinism
//! model. A run records every nondeterministic input the sandbox consumed
//! (seeded time/random values and host-call responses) so `runtime.replay`
//! reproduces it byte-identically on any platform.
//!
//! This is the jewel's last link: "... → deterministic replay, all offline."

use crate::ids::{AppletId, RunId};
use crate::{AppResult, CoreError};
use serde::{Deserialize, Serialize};

/// One recorded host interaction during a run, in call order.
///
/// On replay the engine re-issues the same calls and the recorder *serves*
/// these responses instead of touching live subsystems, so the run cannot
/// diverge. A mismatch between the live call sequence and the recording is a
/// determinism violation (surfaced as `RuntimeError`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedCall {
    /// Monotone index of this host call within the run (0-based).
    pub seq: u64,
    /// Host API method, e.g. `storage.get`, `db.insert`, `time.now`,
    /// `random.next`, `ui.render`.
    pub method: String,
    /// Arguments as seen by the host bridge (canonical JSON).
    pub args: serde_json::Value,
    /// The response the host returned (canonical JSON). For effectful calls
    /// (db.insert, ui.render) this is the ack/result; for seams (time/random)
    /// it is the seeded value.
    pub response: serde_json::Value,
}

/// A complete deterministic execution record. prd-merged/01 CR-9.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: RunId,
    pub applet_id: AppletId,
    /// Hash of the transpiled JS actually executed (provenance + replay key).
    pub code_hash: String,
    /// The `input` passed to `main(ctx, input)`.
    #[serde(default)]
    pub input: serde_json::Value,
    /// Seed for the deterministic RNG seam (`ctx.random`). prd-merged/01 CR-11.
    pub random_seed: u64,
    /// Logical clock start for the deterministic time seam (`ctx.time.now`).
    pub time_start: u64,
    /// Every host call, in order.
    #[serde(default)]
    pub calls: Vec<RecordedCall>,
    /// Captured log lines (bounded by `Limits::log_bytes`).
    #[serde(default)]
    pub logs: Vec<String>,
    /// The run's result, or the error that suspended it.
    pub outcome: RunOutcome,
}

/// Terminal state of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunOutcome {
    /// `main` returned normally.
    Completed { result: AppResult },
    /// The run was suspended (limit hit) or errored.
    Failed { error: CoreError },
}

impl RunRecord {
    pub fn is_completed(&self) -> bool {
        matches!(self.outcome, RunOutcome::Completed { .. })
    }

    /// A stable fingerprint of the *observable* run: code, inputs, seeds, the
    /// ordered call/response trace, and outcome. Two runs with equal
    /// fingerprints are replay-identical. Used by the conformance/replay tests
    /// (prd-merged/09 §2 "deterministic replay identity").
    ///
    /// Note: this is a structural equality digest, not a cryptographic hash —
    /// it deliberately serializes the fields that must match on replay and
    /// excludes the `run_id` (which differs per invocation).
    pub fn replay_fingerprint(&self) -> String {
        let canonical = serde_json::json!({
            "code_hash": self.code_hash,
            "input": self.input,
            "random_seed": self.random_seed,
            "time_start": self.time_start,
            "calls": self.calls,
            "outcome": self.outcome,
        });
        // serde_json::Value serializes object keys in sorted order, so this is
        // canonical for our purposes (all keys are statically known here).
        canonical.to_string()
    }

    /// True iff `other` is a replay-identical run of `self`.
    pub fn replays_identically(&self, other: &RunRecord) -> bool {
        self.replay_fingerprint() == other.replay_fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: RunId::new(run_id),
            applet_id: AppletId::new("app_notes"),
            code_hash: "sha256:abc".into(),
            input: serde_json::json!({"name": "world"}),
            random_seed: 42,
            time_start: 1000,
            calls: vec![
                RecordedCall {
                    seq: 0,
                    method: "time.now".into(),
                    args: serde_json::json!(null),
                    response: serde_json::json!(1000),
                },
                RecordedCall {
                    seq: 1,
                    method: "storage.set".into(),
                    args: serde_json::json!(["name", "world"]),
                    response: serde_json::json!(null),
                },
            ],
            logs: vec!["hello".into()],
            outcome: RunOutcome::Completed {
                result: AppResult { ok: true, value: serde_json::json!("Hello world") },
            },
        }
    }

    #[test]
    fn run_record_roundtrips() {
        let r = sample("run_1");
        let s = serde_json::to_string(&r).unwrap();
        let back: RunRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn identical_runs_with_different_ids_replay_identically() {
        // The whole point: a replay produces a new run_id but the same trace.
        let original = sample("run_1");
        let replay = sample("run_2");
        assert_ne!(original.run_id, replay.run_id);
        assert!(
            original.replays_identically(&replay),
            "same code/seeds/trace must be replay-identical regardless of run_id"
        );
    }

    #[test]
    fn divergent_trace_is_detected() {
        let original = sample("run_1");
        let mut diverged = sample("run_2");
        diverged.calls[0].response = serde_json::json!(9999); // clock differs
        assert!(!original.replays_identically(&diverged));
    }

    #[test]
    fn outcome_status_serializes_by_tag() {
        let r = sample("run_1");
        let s = serde_json::to_string(&r.outcome).unwrap();
        assert!(s.contains("\"status\":\"completed\""), "{s}");
    }
}
