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
use crate::manifest::Capabilities;
use crate::{AppResult, CoreError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The capability/permission state that was *in effect* when a run was recorded
/// (prd-merged/01 CR-9, prd-merged/07 SC-8/SC-9; review 009 P1 CR-9 completeness).
///
/// A run record must capture the evaluated permission decision, not just the
/// effects that happened to succeed: a replay has to re-derive the *same*
/// allow/deny outcome for every host call even if the workspace's grants have
/// since changed (a grant was revoked, a role downgraded, a budget lowered).
/// Replaying against today's manifest instead of the recorded snapshot would let
/// a run that was *denied* at record time silently *succeed* on replay (or vice
/// versa), which is a determinism + provenance hole. The runtime builds its
/// replay-mode policy engine from this snapshot so the recorded decision is the
/// authoritative one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionSnapshot {
    /// The capability grants (storage/db/ui scopes) evaluated for this run.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Whether the actor's role was permitted to run code at all (SC-10).
    #[serde(default)]
    pub can_run: bool,
    /// The host-call budget (`max_host_calls`) in effect for this run (SC-2).
    #[serde(default)]
    pub max_host_calls: u64,
}

/// A captured platform-resource blob keyed by `asset_id` in the run record.
///
/// Blobs are stored here (base64 in the serialized record) rather than inline in
/// `RecordedCall.response` for `resource.invoke`, so camera captures stay off the
/// QuickJS hot path while remaining replayable (prd-merged/01 CR-8/CR-9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceAssetBlob {
    /// Raw bytes, base64-encoded for canonical JSON storage.
    pub bytes_base64: String,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

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
    /// The capability/permission state evaluated for this run (review 009 P1
    /// CR-9). Replay rebuilds its policy decision from this snapshot rather than
    /// the live manifest, so a run replays with the permissions it was recorded
    /// under even if the workspace's grants have since changed. Defaulted on
    /// deserialize so older records (no snapshot) still load.
    #[serde(default)]
    pub permissions: PermissionSnapshot,
    /// Run-scoped resource blobs keyed by `asset_id` (`resource.invoke` stores
    /// metadata in `calls`; bytes live here for replay).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resource_assets: BTreeMap<String, ResourceAssetBlob>,
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
            permissions: PermissionSnapshot::default(),
            resource_assets: BTreeMap::new(),
            outcome,
        };
        record.validate_code_hash()?;
        Ok(record)
    }

    /// Attach the evaluated [`PermissionSnapshot`] to a record (review 009 P1
    /// CR-9). A builder rather than a `new` argument so the validating
    /// constructor's signature stays stable: a caller builds the record via
    /// [`new`](Self::new) (which validates the `code_hash`) and then records the
    /// permission state that was in effect.
    #[must_use]
    pub fn with_permissions(mut self, permissions: PermissionSnapshot) -> Self {
        self.permissions = permissions;
        self
    }

    /// Attach run-scoped resource blobs captured during `resource.invoke`.
    #[must_use]
    pub fn with_resource_assets(
        mut self,
        resource_assets: BTreeMap<String, ResourceAssetBlob>,
    ) -> Self {
        self.resource_assets = resource_assets;
        self
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
            "permissions": self.permissions,
            "resource_assets": self.resource_assets,
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

    /// A composite fingerprint of an ordered **event session** — an initial
    /// `runtime.run` record followed by N `ui.dispatch_event` records, in dispatch
    /// order (prd-merged/05 UI-4, prd-merged/01 CR-6, CR-8). This is the
    /// session-level analogue of [`replay_fingerprint`](Self::replay_fingerprint):
    /// two sessions with equal composite fingerprints replayed the same ordered
    /// event sequence to byte-identical per-run traces.
    ///
    /// The composite is built from the per-run [`replay_fingerprint`] of each record
    /// in order, so it is sensitive to (a) any per-run trace divergence and (b) the
    /// ORDER the events were applied: swapping two events in the sequence changes the
    /// composite even when each individual run still fingerprints the same. A session
    /// replays byte-identically iff its replayed records reproduce this exact value.
    ///
    /// `records` is borrowed in dispatch order; an empty session fingerprints to the
    /// empty-list canonical form (a degenerate but well-defined identity).
    pub fn session_fingerprint(records: &[&RunRecord]) -> String {
        let per_run: Vec<String> = records.iter().map(|r| r.replay_fingerprint()).collect();
        serde_json::json!({ "session": per_run }).to_string()
    }

    /// True iff two ordered event sessions replay identically: same length and each
    /// record replay-identical to its counterpart in order. The composite
    /// [`session_fingerprint`](Self::session_fingerprint) equality is the single
    /// check (it folds in both per-run identity and ordering).
    pub fn session_replays_identically(original: &[&RunRecord], replayed: &[&RunRecord]) -> bool {
        Self::session_fingerprint(original) == Self::session_fingerprint(replayed)
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
            permissions: PermissionSnapshot::default(),
            resource_assets: BTreeMap::new(),
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

    /// Review 009 P1 (CR-9): the permission snapshot round-trips through serde
    /// and `with_permissions` records it without disturbing the validated
    /// `code_hash`.
    #[test]
    fn permission_snapshot_roundtrips_and_attaches() {
        let snap = PermissionSnapshot {
            capabilities: Capabilities::default(),
            can_run: true,
            max_host_calls: 1234,
        };
        let (rid, aid, ch, input, seed, t0, calls, logs, outcome) =
            new_args("run_1", crate::hash::code_hash("body"));
        let r = RunRecord::new(rid, aid, ch, input, seed, t0, calls, logs, outcome)
            .unwrap()
            .with_permissions(snap.clone());
        assert_eq!(r.permissions, snap);
        assert!(r.validate_code_hash().is_ok());
        let back: RunRecord = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.permissions, snap);
    }

    /// A record with no recorded snapshot still deserializes (the field defaults),
    /// so older records remain loadable.
    #[test]
    fn missing_permission_snapshot_defaults_on_deserialize() {
        let mut json = serde_json::to_value(sample("run_1")).unwrap();
        json.as_object_mut().unwrap().remove("permissions");
        let back: RunRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back.permissions, PermissionSnapshot::default());
    }

    /// The permission snapshot is part of the replay fingerprint: two otherwise
    /// identical runs recorded under different permissions are NOT
    /// replay-identical (review 009 P1).
    #[test]
    fn differing_permission_snapshot_changes_fingerprint() {
        let a = sample("run_1");
        let mut b = sample("run_2");
        b.permissions.max_host_calls = a.permissions.max_host_calls + 1;
        assert!(!a.replays_identically(&b));
    }

    /// A session of [initial run + N events] fingerprints byte-identically to a
    /// replay of the same ordered records (UI-4/CR-6 session replay). The composite
    /// folds in each record's per-run fingerprint in order.
    #[test]
    fn session_fingerprint_matches_identical_replay() {
        let initial_a = sample("run_1");
        let event_a = sample("run_2");
        // The replay produces fresh run_ids but byte-identical traces.
        let initial_b = sample("run_3");
        let event_b = sample("run_4");
        let original = [&initial_a, &event_a];
        let replayed = [&initial_b, &event_b];
        assert!(RunRecord::session_replays_identically(&original, &replayed));
        assert_eq!(
            RunRecord::session_fingerprint(&original),
            RunRecord::session_fingerprint(&replayed)
        );
    }

    /// The session fingerprint is ORDER-sensitive: swapping two otherwise-identical
    /// records changes the composite even though each record fingerprints the same.
    /// This is what proves "two events apply in recorded order deterministically".
    #[test]
    fn session_fingerprint_is_order_sensitive() {
        // Two records with DISTINCT traces so order is observable.
        let first = sample("run_1");
        let mut second = sample("run_2");
        second.calls[0].response = serde_json::json!(2000);
        let in_order = [&first, &second];
        let swapped = [&second, &first];
        assert_ne!(
            RunRecord::session_fingerprint(&in_order),
            RunRecord::session_fingerprint(&swapped),
            "swapping event order must change the composite session fingerprint"
        );
        assert!(!RunRecord::session_replays_identically(&in_order, &swapped));
    }

    /// A divergence in ANY record of the session breaks the composite identity (the
    /// session-level analogue of a single-run trace divergence).
    #[test]
    fn session_fingerprint_detects_a_diverged_member() {
        let a0 = sample("run_1");
        let a1 = sample("run_2");
        let b0 = sample("run_3");
        let mut b1 = sample("run_4");
        b1.calls[0].response = serde_json::json!(9999); // one event diverges
        assert!(!RunRecord::session_replays_identically(&[&a0, &a1], &[&b0, &b1]));
    }

    /// An empty session has a well-defined (degenerate) composite identity.
    #[test]
    fn empty_session_fingerprint_is_stable() {
        let empty: [&RunRecord; 0] = [];
        assert_eq!(
            RunRecord::session_fingerprint(&empty),
            RunRecord::session_fingerprint(&empty)
        );
        assert!(RunRecord::session_replays_identically(&empty, &empty));
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
