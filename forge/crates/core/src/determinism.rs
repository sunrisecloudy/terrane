//! Replay-keying determinism helpers: the seed derivation and run-id minting that
//! make a `runtime.run` reproducible byte-for-byte (review 031 finding 2 / review
//! 032 finding 1 / CR-9). The replay seeds are a deterministic function of
//! `(code_hash, input)` (or an explicit payload override), and each execution
//! mints a unique, inspectable `run_id` from the monotone invocation counter — so
//! two executions of the same applet+input persist as two distinct records that
//! each still replay identically to themselves. These are kept dependency-free
//! (a fixed FNV-style fold) because only determinism matters here, not security.

use forge_domain::{CoreCommand, CoreError, Result, RunId};

/// Read the optional explicit `(random_seed, time_start)` seam override from a
/// `runtime.run` payload (review 032 finding 1).
///
/// Returns `Ok(None)` when neither field is present (the default
/// `(code_hash, input)`-derived seeds apply). If *either* is present, *both*
/// must be: a half-specified override is a malformed command (a scenario that
/// pins one seam but lets the other drift is not reproducible), rejected with
/// `ValidationError` rather than silently defaulting the missing seam. Each
/// field must be a non-negative integer that fits `u64`.
pub(crate) fn run_seed_override(cmd: &CoreCommand) -> Result<Option<(u64, u64)>> {
    let random_seed = cmd.payload.get("random_seed");
    let time_start = cmd.payload.get("time_start");
    match (random_seed, time_start) {
        (None, None) => Ok(None),
        (Some(r), Some(t)) => {
            let random_seed = seed_field("random_seed", r)?;
            let time_start = seed_field("time_start", t)?;
            // The logical clock represents time as `i64` (LogicalClock::new casts
            // `time_start as i64`), so a value above `i64::MAX` would wrap to a
            // negative start that `ctx.time.now()` could never have produced
            // honestly. Reject it rather than record an unrepresentable seam
            // (review 037 finding 2).
            if time_start > i64::MAX as u64 {
                return Err(CoreError::ValidationError(format!(
                    "runtime.run `time_start` must fit i64 (<= {}), got {time_start}",
                    i64::MAX
                )));
            }
            Ok(Some((random_seed, time_start)))
        }
        (Some(_), None) | (None, Some(_)) => Err(CoreError::ValidationError(
            "runtime.run seed override must set BOTH `random_seed` and `time_start` or neither"
                .into(),
        )),
    }
}

/// Parse a `u64` seed field from the command payload, rejecting non-integer /
/// out-of-range values with a precise `ValidationError`.
pub(crate) fn seed_field(name: &str, value: &serde_json::Value) -> Result<u64> {
    value.as_u64().ok_or_else(|| {
        CoreError::ValidationError(format!(
            "runtime.run `{name}` must be a non-negative integer that fits u64, got {value}"
        ))
    })
}

/// Derive the deterministic replay seeds `(random_seed, time_start)` from the
/// run's `(code_hash, input)` (review 031 finding 2). The same code + input
/// always produces the same seeds, so re-runs replay byte-identically and the
/// "deterministic across independent runs" property holds; a different input
/// produces different seeds (a genuinely different deterministic run).
///
/// This is a stable, non-cryptographic split of the canonical `code_hash`
/// (which already digests the program) mixed with a digest of the input. It is
/// not security-sensitive — only determinism matters — so a fixed FNV-style
/// fold over the canonical inputs is sufficient and dependency-free.
pub(crate) fn derive_seeds(code_hash: &str, input: &serde_json::Value) -> (u64, u64) {
    // Canonical JSON for the input (serde_json sorts object keys), so equal
    // inputs fold to the same digest regardless of construction order.
    let input_repr = input.to_string();
    let random_seed = fnv1a64(code_hash.as_bytes()) ^ fnv1a64(input_repr.as_bytes());
    // A second, independent fold (salted) for the time seam so the two seeds are
    // not trivially correlated. Mask the sign bit so the value always fits `i64`:
    // the logical clock stores time as `i64` (LogicalClock::new casts), so a
    // derived seed above `i64::MAX` would wrap negative and disagree with the
    // recorded seam (review 037/039 finding 2 — same bound as the explicit
    // `time_start` override, applied to the derived path too).
    let time_start = (fnv1a64(input_repr.as_bytes()).wrapping_mul(0x100000001b3)
        ^ fnv1a64(code_hash.as_bytes()))
        & (i64::MAX as u64);
    (random_seed, time_start)
}

/// A small FNV-1a 64-bit fold. Deterministic and dependency-free; used only to
/// derive replay seeds (not for security or collision resistance).
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Mint a unique, inspectable per-execution `run_id` from the run's `code_hash`
/// and the workspace's monotone invocation counter (review 031 finding 2). The
/// counter guarantees uniqueness even when two executions share code+input (and
/// therefore seeds); the short hash prefix keeps the id self-describing.
pub(crate) fn unique_run_id(code_hash: &str, invocation: u64) -> RunId {
    // Strip the `alg:` tag, then take a short prefix of the digest body.
    let digest = code_hash.split_once(':').map(|(_, body)| body).unwrap_or(code_hash);
    let short = &digest[..8.min(digest.len())];
    RunId::new(format!("run_{short}_{invocation:06}"))
}

#[cfg(test)]
mod seed_override_tests {
    use super::*;
    use forge_domain::{ActorContext, RequestId, WorkspaceId};

    fn run_cmd(payload: serde_json::Value) -> CoreCommand {
        CoreCommand {
            request_id: RequestId::new("r1"),
            actor: ActorContext::owner("dev"),
            workspace_id: WorkspaceId::new("ws1"),
            applet_id: None,
            name: "runtime.run".into(),
            payload,
        }
    }

    #[test]
    fn no_override_when_neither_seed_present() {
        assert_eq!(run_seed_override(&run_cmd(serde_json::json!({}))).unwrap(), None);
    }

    #[test]
    fn both_seeds_in_range_are_accepted() {
        let got = run_seed_override(&run_cmd(serde_json::json!({
            "random_seed": 7u64, "time_start": 1000u64
        })))
        .unwrap();
        assert_eq!(got, Some((7, 1000)));
    }

    #[test]
    fn half_specified_override_is_rejected() {
        assert_eq!(
            run_seed_override(&run_cmd(serde_json::json!({ "random_seed": 7u64 })))
                .unwrap_err()
                .code(),
            "ValidationError"
        );
    }

    #[test]
    fn time_start_above_i64_max_is_rejected() {
        // review 037 finding 2: the logical clock is i64, so a u64 time_start
        // beyond i64::MAX would wrap negative — reject it instead of recording an
        // unrepresentable seam. random_seed may still use the full u64 range.
        let over = (i64::MAX as u64) + 1;
        let err = run_seed_override(&run_cmd(serde_json::json!({
            "random_seed": u64::MAX, "time_start": over
        })))
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        // boundary: exactly i64::MAX is allowed.
        let ok = run_seed_override(&run_cmd(serde_json::json!({
            "random_seed": 1u64, "time_start": i64::MAX as u64
        })))
        .unwrap();
        assert_eq!(ok, Some((1, i64::MAX as u64)));
    }
}
