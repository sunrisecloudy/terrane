//! Deterministic run record + replay contract.
//!
//! prd-merged/01 CR-8 (deterministic mode), CR-9 (run records), CR-11
//! (injectable clock/RNG seams); prd-merged/02 System-Architecture determinism
//! model. A run records every nondeterministic input the sandbox consumed
//! (seeded time/random values and host-call responses) so `runtime.replay`
//! reproduces it byte-identically on any platform.
//!
//! This is the jewel's last link: "... → deterministic replay, all offline."

use crate::hash::is_canonical_code_hash;
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

    /// Build a [`RunRecord`] whose `code_hash` is **validated at construction**,
    /// so a record that exists is a record whose provenance is canonical
    /// (prd-merged/01 CR-9, CR-14; review 010 P1, review 013 P1).
    ///
    /// This is the contract's *teeth*. Until now `validate_code_hash` was an
    /// opt-in method a recorder could simply forget to call (review 013 P1), and
    /// the only way to make a record was the struct literal — which happily
    /// accepts the runtime's old `fnv1a64:…` string. Routing record creation
    /// through this constructor makes the canonical-hash check non-bypassable:
    /// the recorder hands in the pieces and gets back either a valid record or a
    /// `ValidationError`, never a record carrying a divergent provenance hash.
    ///
    /// Callers that build a record field-by-field (e.g. a recorder that fills
    /// `calls`/`logs` incrementally) should finish by calling
    /// [`validate_code_hash`](Self::validate_code_hash) before trusting it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: RunId,
        applet_id: AppletId,
        code_hash: String,
        input: serde_json::Value,
        random_seed: u64,
        time_start: u64,
        calls: Vec<RecordedCall>,
        logs: Vec<String>,
        outcome: RunOutcome,
    ) -> crate::Result<Self> {
        let record = RunRecord {
            run_id,
            applet_id,
            code_hash,
            input,
            random_seed,
            time_start,
            calls,
            logs,
            outcome,
        };
        record.validate_code_hash()?;
        Ok(record)
    }

    /// Reject a record whose `code_hash` is not the canonical `sha256:` form
    /// (prd-merged/01 CR-9, CR-14; review 010 P1, review 013 P1/P2).
    ///
    /// The `code_hash` is the run's provenance + replay key, so it must be the
    /// single algorithm every crate agrees on
    /// ([`code_hash`](crate::hash::code_hash)). This guard gives that contract
    /// teeth at the record boundary: a recorder that stores a divergent string
    /// (the runtime's old `fnv1a64:…`, an uppercase digest, a truncated body)
    /// fails here instead of silently shipping a hash the pipeline can never
    /// reproduce.
    ///
    /// This is the building block for that enforcement, not the enforcement
    /// itself: it only takes effect where a caller actually invokes it.
    /// [`RunRecord::new`](Self::new) calls it for callers that build a record in
    /// one shot; recording/replay/storage boundaries in other crates must adopt
    /// `new` (or call this method) for a non-canonical record to be rejected.
    pub fn validate_code_hash(&self) -> crate::Result<()> {
        if is_canonical_code_hash(&self.code_hash) {
            Ok(())
        } else {
            Err(CoreError::ValidationError(format!(
                "run record code_hash is not canonical sha256: {:?}",
                self.code_hash
            )))
        }
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

    /// Strict replay check: both records must carry a *canonical* `code_hash`
    /// and be replay-identical (review 010 P1, review 013 P1).
    ///
    /// `replays_identically` only compares fingerprints, so two records that
    /// happen to share a divergent provenance string (e.g. both `fnv1a64:…`)
    /// would "match" while neither is reproducible by the pipeline. This helper
    /// closes that hole: it refuses to bless a replay whose provenance is not
    /// the single canonical algorithm, then asserts byte-identical traces.
    pub fn assert_replay_of(&self, other: &RunRecord) -> crate::Result<()> {
        self.validate_code_hash()?;
        other.validate_code_hash()?;
        if self.replays_identically(other) {
            Ok(())
        } else {
            Err(CoreError::RuntimeError(
                "replay diverged from recorded run (trace fingerprints differ)".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: RunId::new(run_id),
            applet_id: AppletId::new("app_notes"),
            // Canonical sha256: of a stand-in body, so the sample is a
            // contract-valid record (its code_hash passes validate_code_hash).
            code_hash: crate::hash::code_hash("sample-body"),
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

    /// A record carrying a canonical `sha256:` provenance hash validates.
    #[test]
    fn canonical_code_hash_validates() {
        let r = sample("run_1");
        assert!(r.validate_code_hash().is_ok(), "sample uses a canonical sha256: hash");
    }

    /// Regression for review 010 P1: a record carrying the runtime's old
    /// `fnv1a64:` provenance string (the exact divergence the review flagged)
    /// is rejected at the record boundary instead of replaying as if valid.
    #[test]
    fn fnv1a64_code_hash_is_rejected_by_record() {
        let mut r = sample("run_1");
        r.code_hash = "fnv1a64:0123456789abcdef".into();
        let err = r.validate_code_hash().expect_err("non-canonical hash must be rejected");
        assert_eq!(err.code(), "ValidationError");
    }

    /// A short/garbage `code_hash` (e.g. the prior placeholder `sha256:abc`)
    /// also fails: the body must be exactly 64 lowercase-hex chars.
    #[test]
    fn malformed_code_hash_is_rejected_by_record() {
        let mut r = sample("run_1");
        r.code_hash = "sha256:abc".into();
        assert!(r.validate_code_hash().is_err());
    }

    /// Helper: the field tuple a constructor call needs, derived from `sample`
    /// so the two stay in lockstep.
    fn new_args(
        run_id: &str,
        code_hash: String,
    ) -> (
        RunId,
        AppletId,
        String,
        serde_json::Value,
        u64,
        u64,
        Vec<RecordedCall>,
        Vec<String>,
        RunOutcome,
    ) {
        let s = sample(run_id);
        (
            s.run_id,
            s.applet_id,
            code_hash,
            s.input,
            s.random_seed,
            s.time_start,
            s.calls,
            s.logs,
            s.outcome,
        )
    }

    /// Review 013 P1: `RunRecord::new` validates the `code_hash` at
    /// construction, so a canonical hash yields a record and that record is
    /// already contract-valid.
    #[test]
    fn new_accepts_canonical_code_hash() {
        let (rid, aid, ch, input, seed, t0, calls, logs, outcome) =
            new_args("run_1", crate::hash::code_hash("body"));
        let r = RunRecord::new(rid, aid, ch, input, seed, t0, calls, logs, outcome)
            .expect("canonical hash must build a record");
        assert!(r.validate_code_hash().is_ok());
    }

    /// Review 013 P1 (the teeth): `RunRecord::new` refuses to build a record
    /// from the runtime's old `fnv1a64:` provenance string. This is the
    /// non-bypassable path — a recorder cannot mint a divergent record by
    /// "forgetting" to validate, because construction itself fails.
    #[test]
    fn new_rejects_fnv1a64_code_hash() {
        let (rid, aid, ch, input, seed, t0, calls, logs, outcome) =
            new_args("run_1", "fnv1a64:0123456789abcdef".into());
        let err = RunRecord::new(rid, aid, ch, input, seed, t0, calls, logs, outcome)
            .expect_err("non-canonical hash must not construct a record");
        assert_eq!(err.code(), "ValidationError");
    }

    /// A record built by `new` is byte-identical to the equivalent struct
    /// literal, so adopting the constructor changes provenance enforcement
    /// only, never the recorded data.
    #[test]
    fn new_matches_struct_literal() {
        let literal = sample("run_1");
        let (rid, aid, ch, input, seed, t0, calls, logs, outcome) =
            new_args("run_1", literal.code_hash.clone());
        let built = RunRecord::new(rid, aid, ch, input, seed, t0, calls, logs, outcome).unwrap();
        assert_eq!(literal, built);
    }

    /// `assert_replay_of` blesses a replay only when both records carry a
    /// canonical hash and the traces match.
    #[test]
    fn assert_replay_of_accepts_canonical_identical_runs() {
        let original = sample("run_1");
        let replay = sample("run_2");
        assert!(original.assert_replay_of(&replay).is_ok());
    }

    /// Review 010 P1 / 013 P1: `assert_replay_of` refuses to bless a replay
    /// whose provenance is the divergent `fnv1a64:` form even when the two
    /// fingerprints match — `replays_identically` alone would wrongly say yes.
    #[test]
    fn assert_replay_of_rejects_matching_but_non_canonical_provenance() {
        let mut original = sample("run_1");
        let mut replay = sample("run_2");
        original.code_hash = "fnv1a64:0123456789abcdef".into();
        replay.code_hash = "fnv1a64:0123456789abcdef".into();
        // The escape hatch the strict check closes: bare fingerprint equality.
        assert!(
            original.replays_identically(&replay),
            "precondition: divergent-but-equal hashes fool replays_identically"
        );
        let err = original
            .assert_replay_of(&replay)
            .expect_err("non-canonical provenance must not be blessed as a replay");
        assert_eq!(err.code(), "ValidationError");
    }

    /// `assert_replay_of` surfaces a genuine trace divergence as a
    /// `RuntimeError` (not a silent `false`).
    #[test]
    fn assert_replay_of_rejects_divergent_trace() {
        let original = sample("run_1");
        let mut diverged = sample("run_2");
        diverged.calls[0].response = serde_json::json!(9999);
        let err = original
            .assert_replay_of(&diverged)
            .expect_err("a divergent trace must be a replay error");
        assert_eq!(err.code(), "RuntimeError");
    }
}
